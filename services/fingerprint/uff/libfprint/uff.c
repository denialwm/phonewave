/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Experimental native driver for sensors behind a root UFF1 helper: the
 * Aston's G7s UFF and the NX809J's Goodix GUM5228. The helper's worker
 * isolates blocking Qualcomm secure calls and owns their complete resource
 * lifetime. UFF1 is a private, versioned worker interface, not a public
 * libfprint API.
 */
#define FP_COMPONENT "uff"
#include "drivers_api.h"
#include "fp-device-private.h"
#include <gio/gunixoutputstream.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef UFF_HELPER
#define UFF_HELPER "/usr/local/libexec/uff-fprint-helper"
#endif
#ifndef GOODIX_HELPER
#define GOODIX_HELPER "/usr/local/libexec/goodix-fprint-helper"
#endif

/* Each phone's sensor: what fprintd calls it and how many templates its
 * secure side keeps. The id table's driver_data indexes it. */
typedef struct {
  const char *driver, *name;
  guint prints;
} UffSensor;

static const UffSensor sensors[] = {
  { "aston-goodix-g7s-uff", "UFF G7s UFF (native, experimental)", 5 },
  { "nx809j-goodix-gum5228", "Goodix GUM5228 ultrasonic (native, experimental)", 10 },
};

typedef struct {
  FpDevice parent;
  GSubprocess *worker;
  GDataInputStream *output;
  GOutputStream *control;
  GPtrArray *prints;
  GSource *deadline;
  GSource *sleep_deadline;
  gboolean cancelled, failed, result_seen, reported;
  gboolean suspending, suspended, resuming;
  guint result, identifier, stages;
  guint power_phase; /* 0 legacy, 1 idle/quiescent, 2 capture/on, 3 cleaned/quiescent */
} FpiDeviceUff;
typedef FpDeviceClass FpiDeviceUffClass;
GType fpi_device_uff_get_type (void);
G_DEFINE_TYPE (FpiDeviceUff, fpi_device_uff, FP_TYPE_DEVICE)

static gboolean matching (FpDevice *dev);

static void
sleep_complete (FpDevice *dev, GError *error)
{
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  if (self->sleep_deadline)
    { g_source_destroy (self->sleep_deadline); self->sleep_deadline = NULL; }
  if (self->suspending)
    {
      self->suspending = FALSE;
      self->suspended = error == NULL;
      fpi_device_suspend_complete (dev, error);
    }
  else if (self->resuming)
    {
      gboolean failed = error != NULL;
      self->resuming = self->suspended = FALSE;
      fpi_device_resume_complete (dev, error);
      if (failed && self->worker)
        {
          self->failed = TRUE;
          g_subprocess_send_signal (self->worker, SIGTERM);
        }
    }
  else
    g_clear_error (&error);
}

static void
sleep_timeout (FpDevice *dev, gpointer unused)
{
  (void) unused;
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  self->sleep_deadline = NULL;
  sleep_complete (dev, fpi_device_error_new_msg (FP_DEVICE_ERROR_NOT_SUPPORTED,
                                               "Fingerprint suspend handshake timed out"));
}

static void
sleep_command (FpDevice *dev, gboolean suspend)
{
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  g_autoptr(GError) error = NULL;
  const char *request = suspend ? "suspend\n" : "resume\n";
  self->suspending = suspend;
  self->resuming = !suspend;
  if (!matching (dev) ||
      !self->worker || self->power_phase != 1 ||
      self->failed || self->cancelled || self->result_seen ||
      (!suspend && !self->suspended))
    {
      sleep_complete (dev, fpi_device_error_new (FP_DEVICE_ERROR_NOT_SUPPORTED));
      return;
    }
  self->sleep_deadline = fpi_device_add_timeout (dev, 2000, sleep_timeout, NULL, NULL);
  /* At most one short command is outstanding, so this cannot fill the pipe. */
  if (!g_output_stream_write_all (self->control,
                                  request, strlen (request), NULL, NULL, &error))
    sleep_complete (dev, g_steal_pointer (&error));
}

static void suspend_device (FpDevice *dev) { sleep_command (dev, TRUE); }
static void resume_device (FpDevice *dev) { sleep_command (dev, FALSE); }

static const FpIdEntry id_table[] = {
  { .udev_types = FPI_DEVICE_UDEV_SUBTYPE_PLATFORM,
    .platform_driver = "oplus-uff-fp", .platform_compatible = "oneplus,aston-fingerprint",
    .driver_data = 0 },
  { .udev_types = FPI_DEVICE_UDEV_SUBTYPE_PLATFORM,
    .platform_driver = "goodix-tee-fp", .platform_compatible = "goodix,gum5228",
    .driver_data = 1 },
  { .udev_types = 0 }
};

/* The secure side matches a touch against every template and says which:
 * verify checks it is the expected print, identify finds it in the gallery. */
static const UffSensor *
sensor (FpDevice *dev)
{
  return &sensors[fpi_device_get_driver_data (dev)];
}

static const char *
helper (FpDevice *dev)
{
  return fpi_device_get_driver_data (dev) == 1 ? GOODIX_HELPER : UFF_HELPER;
}

static gboolean
matching (FpDevice *dev)
{
  FpiDeviceAction action = fpi_device_get_current_action (dev);
  return action == FPI_DEVICE_ACTION_VERIFY || action == FPI_DEVICE_ACTION_IDENTIFY;
}

static FpPrint *
make_print (FpDevice *dev, guint identifier)
{
  FpPrint *print = fp_print_new (dev);
  fpi_print_set_type (print, FPI_PRINT_RAW);
  fpi_print_set_device_stored (print, TRUE);
  g_object_set (print, "fpi-data", g_variant_new_uint32 (identifier), NULL);
  return print;
}

static gboolean
print_identifier (FpPrint *print, guint *identifier)
{
  g_autoptr(GVariant) data = NULL;
  g_object_get (print, "fpi-data", &data, NULL);
  if (!data || !g_variant_is_of_type (data, G_VARIANT_TYPE_UINT32))
    return FALSE;
  *identifier = g_variant_get_uint32 (data);
  return *identifier != 0;
}

/* A verification's outcome goes to fprintd as soon as the helper has it;
 * fprintd tells its client at once. The helper's cleanup follows, and only
 * decides how the action completes. */
static void
report (FpDevice *dev)
{
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  guint identifier = 0;
  self->reported = TRUE;
  if (fpi_device_get_current_action (dev) == FPI_DEVICE_ACTION_VERIFY)
    {
      FpPrint *expected;
      if (self->result == 3)
        {
          fpi_device_verify_report (dev, FPI_MATCH_ERROR, NULL,
                                   fpi_device_retry_new (FP_DEVICE_RETRY_TOO_SHORT));
          return;
        }
      fpi_device_get_verify_data (dev, &expected);
      gboolean match = !self->result && self->identifier &&
                       print_identifier (expected, &identifier) && identifier == self->identifier;
      fpi_device_verify_report (dev, match ? FPI_MATCH_SUCCESS : FPI_MATCH_FAIL, NULL, NULL);
      return;
    }
  GPtrArray *gallery;
  FpPrint *found = NULL;
  if (self->result == 3)
    {
      fpi_device_identify_report (dev, NULL, NULL,
                                 fpi_device_retry_new (FP_DEVICE_RETRY_TOO_SHORT));
      return;
    }
  fpi_device_get_identify_data (dev, &gallery);
  for (guint i = 0; !self->result && self->identifier && !found && i < gallery->len; i++)
    if (print_identifier (g_ptr_array_index (gallery, i), &identifier) &&
        identifier == self->identifier)
      found = g_ptr_array_index (gallery, i);
  fpi_device_identify_report (dev, found, NULL, NULL);
}

static void
finished (GObject *object, GAsyncResult *result, gpointer user_data)
{
  FpDevice *dev = user_data;
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  g_autoptr(GError) error = NULL;
  gboolean exited = g_subprocess_wait_check_finish (G_SUBPROCESS (object), result, &error);
  g_autoptr(GPtrArray) prints = g_steal_pointer (&self->prints);

  if (self->deadline)
    { g_source_destroy (self->deadline); self->deadline = NULL; }
  g_clear_object (&self->output);
  g_clear_object (&self->control);
  g_clear_object (&self->worker);
  if (self->suspending || self->resuming)
    sleep_complete (dev, fpi_device_error_new_msg (FP_DEVICE_ERROR_NOT_SUPPORTED,
                                                 "Fingerprint worker exited during suspend handshake"));
  if (self->cancelled)
    {
      g_clear_error (&error);
      error = g_error_new_literal (G_IO_ERROR, G_IO_ERROR_CANCELLED, "Fingerprint operation cancelled");
    }
  else if (self->failed || !exited || !self->result_seen || (self->result > 1 && self->result != 3) ||
           (self->power_phase != 0 && self->power_phase != 3) ||
           (self->result && !matching (dev)))
    {
      g_clear_error (&error);
      error = fpi_device_error_new_msg (FP_DEVICE_ERROR_GENERAL, "Native fingerprint worker failed");
    }
  if (error)
    {
      fpi_device_action_error (dev, g_steal_pointer (&error));
      return;
    }
  switch (fpi_device_get_current_action (dev))
    {
    case FPI_DEVICE_ACTION_PROBE:
      fpi_device_probe_complete (dev, sensor (dev)->driver, sensor (dev)->name, NULL);
      break;
    case FPI_DEVICE_ACTION_OPEN:
      fpi_device_open_complete (dev, NULL);
      break;
    case FPI_DEVICE_ACTION_ENROLL:
      {
        FpPrint *print;
        if (!self->identifier || self->result)
          { fpi_device_action_error (dev, fpi_device_error_new (FP_DEVICE_ERROR_GENERAL)); break; }
        fpi_device_get_enroll_data (dev, &print);
        fpi_print_set_type (print, FPI_PRINT_RAW);
        fpi_print_set_device_stored (print, TRUE);
        g_object_set (print, "fpi-data", g_variant_new_uint32 (self->identifier), NULL);
        fpi_device_enroll_complete (dev, g_object_ref (print), NULL);
        break;
      }
    case FPI_DEVICE_ACTION_VERIFY:
      if (!self->reported)
        report (dev);
      fpi_device_verify_complete (dev, NULL);
      break;
    case FPI_DEVICE_ACTION_IDENTIFY:
      if (!self->reported)
        report (dev);
      fpi_device_identify_complete (dev, NULL);
      break;
    case FPI_DEVICE_ACTION_LIST:
      fpi_device_list_complete (dev, g_steal_pointer (&prints), NULL);
      break;
    case FPI_DEVICE_ACTION_DELETE:
      fpi_device_delete_complete (dev, NULL);
      break;
    case FPI_DEVICE_ACTION_NONE:
    case FPI_DEVICE_ACTION_CLOSE:
    case FPI_DEVICE_ACTION_CAPTURE:
    case FPI_DEVICE_ACTION_CLEAR_STORAGE:
      fpi_device_action_error (dev, fpi_device_error_new (FP_DEVICE_ERROR_GENERAL));
    }
}

static void read_line (GObject *object, GAsyncResult *result, gpointer user_data);
static void timeout (FpDevice *dev, gpointer unused);

static void
read_next (FpDevice *dev)
{
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  g_data_input_stream_read_line_async (self->output, G_PRIORITY_DEFAULT, NULL, read_line, dev);
}

static void
read_line (GObject *object, GAsyncResult *result, gpointer user_data)
{
  FpDevice *dev = user_data;
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  g_autoptr(GError) error = NULL;
  gsize length;
  g_autofree gchar *line = g_data_input_stream_read_line_finish (G_DATA_INPUT_STREAM (object), result, &length, &error);
  guint value = 0, identifier = 0;
  int consumed = 0;

  if (!line)
    {
      if (error) self->result_seen = FALSE;
      g_subprocess_wait_check_async (self->worker, NULL, finished, dev);
      return;
    }
  if (!strcmp (line, "UFF1 suspended") && self->suspending && self->power_phase == 1)
    {
      g_message ("UFF verification retained across suspend; sensor is quiescent");
      sleep_complete (dev, NULL);
    }
  else if (!strcmp (line, "UFF1 resumed") && self->resuming && self->suspended && self->power_phase == 1)
    {
      g_message ("UFF verification resumed with original touch listener");
      sleep_complete (dev, NULL);
    }
  else if (!strcmp (line, "UFF1 suspend-busy") && self->suspending)
    sleep_complete (dev, fpi_device_error_new (FP_DEVICE_ERROR_NOT_SUPPORTED));
  /* After a reported result, only the cleanup's quiescent record. */
  else if (length <= 80 && sscanf (line, "UFF1 power %u%n", &value, &consumed) == 1 &&
      consumed == (int) length && matching (dev) &&
      (!self->result_seen || (self->reported && value != 1 && self->power_phase == 2)) &&
      ((value == 0 && (self->power_phase == 0 || self->power_phase == 1 || self->power_phase == 2)) ||
       (value == 1 && self->power_phase == 1) ||
       (value == 2 && (self->power_phase == 0 || self->power_phase == 2))))
    {
      self->power_phase = value == 2 ? (self->power_phase == 0 ? 1 : 3) :
                          value == 1 ? 2 : (self->power_phase == 0 ? 1 : 3);
      /* Idle is either rail-off (0) or signed-firmware sleep (2).
       * Only active capture (1) contributes to the thermal model. */
      fpi_device_update_temp (dev, value == 1);
      /* A power-on record follows a physical contact; expose that contact
       * to fprintd while capture is active, then clear it on cleanup. */
      fpi_device_report_finger_status (dev, value == 1 ? FP_FINGER_STATUS_PRESENT :
                                       self->power_phase == 1 ? FP_FINGER_STATUS_NEEDED : FP_FINGER_STATUS_NONE);
      if (self->deadline)
        { g_source_destroy (self->deadline); self->deadline = NULL; }
      if (self->power_phase != 1)
        self->deadline = fpi_device_add_timeout (dev, 30000, timeout, NULL, NULL);
    }
  else if (!self->result_seen && length <= 80 && sscanf (line, "UFF1 progress %u%n", &value, &consumed) == 1 &&
      consumed == (int) length && value <= 20 && value > self->stages &&
      fpi_device_get_current_action (dev) == FPI_DEVICE_ACTION_ENROLL)
    {
      self->stages = value;
      fpi_device_enroll_progress (dev, value, NULL, NULL);
    }
  else if (!self->result_seen && g_str_equal (line, "UFF1 retry") &&
           fpi_device_get_current_action (dev) == FPI_DEVICE_ACTION_ENROLL)
    fpi_device_enroll_progress (dev, self->stages, NULL,
                                fpi_device_retry_new (FP_DEVICE_RETRY_CENTER_FINGER));
  else if (!self->result_seen && length <= 80 && sscanf (line, "UFF1 item %u%n", &identifier, &consumed) == 1 &&
           consumed == (int) length && identifier && self->prints &&
           self->prints->len < sensor (dev)->prints &&
           fpi_device_get_current_action (dev) == FPI_DEVICE_ACTION_LIST)
    {
      gboolean duplicate = FALSE;
      for (guint i = 0; i < self->prints->len; i++)
        {
          guint existing = 0;
          duplicate |= print_identifier (g_ptr_array_index (self->prints, i), &existing) && existing == identifier;
        }
      if (duplicate)
        { self->failed = TRUE; g_subprocess_send_signal (self->worker, SIGTERM); }
      else
        g_ptr_array_add (self->prints, make_print (dev, identifier));
    }
  else if (length <= 80 && sscanf (line, "UFF1 result %u %u%n", &value, &identifier, &consumed) == 2 &&
           consumed == (int) length && !self->result_seen)
    {
      self->result = value; self->identifier = identifier; self->result_seen = TRUE;
      if (matching (dev) && !self->cancelled && !self->failed &&
          (value == 0 || value == 1 || value == 3) && (value != 0 || identifier))
        report (dev);
    }
  else
    {
      self->failed = TRUE;
      g_subprocess_send_signal (self->worker, SIGTERM);
    }
  read_next (dev);
}

static void
cancel (FpDevice *dev)
{
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  self->cancelled = TRUE;
  /* Once the result is reported only cleanup is left: it runs to its end,
   * so the sensor's power state stays confirmed. */
  if (self->worker && !self->reported) g_subprocess_send_signal (self->worker, SIGTERM);
}

static void
timeout (FpDevice *dev, gpointer unused)
{
  (void) unused;
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  self->deadline = NULL;
  self->failed = TRUE;
  if (self->worker) g_subprocess_send_signal (self->worker, SIGTERM);
}

static GOutputStream *
open_control (GSubprocessLauncher *launcher, GError **error)
{
  int fds[2];
  if (pipe2 (fds, O_CLOEXEC) < 0)
    goto failed;
  /* fprintd's logind inhibitor can release descriptor 0. GLib 2.88's
   * spawn path substitutes /dev/null when a new stdin pipe already uses
   * that number. Explicitly map a source above the standard descriptors;
   * never reserve or replace an fd belonging to the hosting process. */
  int reader = fcntl (fds[0], F_DUPFD_CLOEXEC, 3);
  if (reader < 0)
    {
      int saved_errno = errno;
      close (fds[0]);
      close (fds[1]);
      errno = saved_errno;
      goto failed;
    }
  close (fds[0]);
  g_subprocess_launcher_take_stdin_fd (launcher, reader);
  return g_unix_output_stream_new (fds[1], TRUE);

failed:
  g_set_error (error, G_IO_ERROR, g_io_error_from_errno (errno),
               "Cannot create fingerprint control pipe: %s", g_strerror (errno));
  return NULL;
}

/* The kernel's sched_attr, up to the utilization clamps. */
struct helper_sched_attr {
  guint32 size, policy;
  guint64 flags;
  gint32 nice;
  guint32 priority;
  guint64 runtime, deadline, period;
  guint32 util_min, util_max;
};

/* The helper's secure calls run on the core that makes them. A full
 * utilization floor puts the helper and its children on the fastest core
 * at its top clock while they run; while they wait it costs nothing.
 * Without clamp support the helper runs as before. */
static void
helper_setup (gpointer unused)
{
  struct helper_sched_attr attr = {
    .size = sizeof (attr),
    /* Keep policy and parameters; set only the minimum clamp. */
    .flags = 0x08 | 0x10 | 0x20,
    .util_min = 1024,
  };
  (void) unused;
  syscall (SYS_sched_setattr, 0, &attr, 0);
}

static void
start (FpDevice *dev)
{
  FpiDeviceUff *self = (FpiDeviceUff *) dev;
  const gchar *operation = NULL;
  g_autofree gchar *argument = NULL;
  g_autoptr(GError) error = NULL;
  switch (fpi_device_get_current_action (dev))
    {
    case FPI_DEVICE_ACTION_PROBE: operation = "probe"; break;
    case FPI_DEVICE_ACTION_OPEN: operation = "open"; break;
    case FPI_DEVICE_ACTION_ENROLL: operation = "enroll"; break;
    case FPI_DEVICE_ACTION_VERIFY:
      {
        FpPrint *print;
        guint identifier;
        fpi_device_get_verify_data (dev, &print);
        if (!print_identifier (print, &identifier))
          { fpi_device_action_error (dev, fpi_device_error_new (FP_DEVICE_ERROR_DATA_INVALID)); return; }
        operation = "verify";
        break;
      }
    case FPI_DEVICE_ACTION_IDENTIFY:
      {
        GPtrArray *gallery;
        fpi_device_get_identify_data (dev, &gallery);
        /* No enrolled print can match an empty gallery. This also lets
         * fprintd proceed with its duplicate check for the first enrollment. */
        if (fpi_device_get_driver_data (dev) == 1 && gallery->len == 0)
          {
            fpi_device_identify_report (dev, NULL, NULL, NULL);
            fpi_device_identify_complete (dev, NULL);
            return;
          }
        operation = "verify";
        break;
      }
    case FPI_DEVICE_ACTION_LIST: operation = "list"; break;
    case FPI_DEVICE_ACTION_DELETE:
      {
        FpPrint *print;
        guint identifier;
        fpi_device_get_delete_data (dev, &print);
        if (!print_identifier (print, &identifier))
          { fpi_device_delete_complete (dev, fpi_device_error_new (FP_DEVICE_ERROR_DATA_INVALID)); return; }
        operation = "delete";
        argument = g_strdup_printf ("%u", identifier);
        break;
      }
    case FPI_DEVICE_ACTION_NONE:
    case FPI_DEVICE_ACTION_CLOSE:
    case FPI_DEVICE_ACTION_CAPTURE:
    case FPI_DEVICE_ACTION_CLEAR_STORAGE:
      g_assert_not_reached ();
    }
  g_assert (operation != NULL);
  self->cancelled = self->failed = self->result_seen = self->reported = FALSE;
  self->suspending = self->resuming = self->suspended = FALSE;
  self->result = self->identifier = self->stages = self->power_phase = 0;
  self->prints = g_ptr_array_new_with_free_func (g_object_unref);
  g_autoptr(GSubprocessLauncher) launcher = g_subprocess_launcher_new (
    G_SUBPROCESS_FLAGS_STDOUT_PIPE);
  g_subprocess_launcher_setenv (launcher, "UFF_SUSPEND_PROTOCOL", "1", TRUE);
  g_subprocess_launcher_set_child_setup (launcher, helper_setup, NULL, NULL);
  self->control = open_control (launcher, &error);
  if (self->control)
    self->worker = g_subprocess_launcher_spawn (launcher, &error,
                                             helper (dev), operation, argument, NULL);
  if (!self->worker)
    {
      g_clear_object (&self->control);
      g_clear_pointer (&self->prints, g_ptr_array_unref);
      fpi_device_action_error (dev, g_steal_pointer (&error));
      return;
    }
  self->output = g_data_input_stream_new (g_subprocess_get_stdout_pipe (self->worker));
  self->deadline = fpi_device_add_timeout (dev, 150000, timeout, NULL, NULL);
  read_next (dev);
}

static void close_device (FpDevice *dev) { fpi_device_close_complete (dev, NULL); }
static void fpi_device_uff_init (FpiDeviceUff *self) { (void) self; }
static void
fpi_device_uff_class_init (FpiDeviceUffClass *klass)
{
  klass->id = "uff";
  klass->full_name = "UFF1 native platform sensor (experimental)";
  klass->type = FP_DEVICE_TYPE_UDEV;
  klass->id_table = id_table;
  klass->nr_enroll_stages = 20;
  klass->scan_type = FP_SCAN_TYPE_PRESS;
  klass->probe = klass->open = klass->enroll = klass->verify = klass->identify = klass->list =
    klass->delete = start;
  klass->close = close_device;
  klass->cancel = cancel;
  klass->suspend = suspend_device;
  klass->resume = resume_device;
  fpi_device_class_auto_initialize_features (klass);
}
