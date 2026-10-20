/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Real UFF callbacks with a pipe worker; no device, input or display access. */
#include <glib.h>
#include <glib/gstdio.h>
static const char *test_helper;
#define UFF_HELPER test_helper
#include "../libfprint/drivers/uff.c"

typedef struct { gboolean done, match; GError *error; } VerifyResult;

static void
verified (GObject *object, GAsyncResult *result, gpointer data)
{
  VerifyResult *out = data;
  fp_device_verify_finish (FP_DEVICE (object), result, &out->match, NULL, &out->error);
  out->done = TRUE;
}

static void
exercise (gconstpointer scenario)
{
  g_autoptr(GError) error = NULL;
  g_autoptr(FpDevice) device = NULL;
  g_autoptr(FpPrint) print = NULL;
  g_autoptr(GCancellable) cancellation = g_cancellable_new ();
  g_autofree gchar *filename = NULL;
  g_autofree gchar *program = NULL;
  VerifyResult result = {0};
  int saved_stdin = -1;
  int fd = g_file_open_tmp ("uff-suspend-test-XXXXXX", &filename, &error);
  g_assert_no_error (error);
  close (fd);
  program = g_strdup_printf (
    "#!/usr/bin/python3\nimport sys,time\nscenario='%s'\n"
    "if sys.argv[1]!='verify':\n print('UFF1 result 0 0',flush=True)\n sys.exit(0)\n"
    "print('UFF1 power 2' if scenario=='retained-sleep' else 'UFF1 power 0',flush=True)\n"
    "for line in sys.stdin:\n"
    " if line=='suspend\\n':\n"
    "  if scenario=='timeout': continue\n"
    "  print('UFF1 suspend-busy' if scenario=='busy' else 'UFF1 invalid' if scenario=='malformed' else 'UFF1 suspended',flush=True)\n"
    " elif line=='resume\\n':\n"
    "  print('UFF1 invalid' if scenario=='resume-error' else 'UFF1 resumed',flush=True)\n",
    (const char *) scenario);
  g_assert_true (g_file_set_contents (filename, program, -1, &error));
  g_assert_no_error (error);
  g_assert_cmpint (g_chmod (filename, 0700), ==, 0);
  test_helper = filename;
  device = g_object_new (fpi_device_uff_get_type (), NULL);
  g_assert_true (fp_device_open_sync (device, NULL, &error));
  g_assert_no_error (error);
  if (!strcmp (scenario, "closed-stdin"))
    { saved_stdin = dup (0); close (0); }
  print = make_print (device, 42);
  fp_device_verify (device, print, cancellation, NULL, NULL, NULL, verified, &result);
  FpiDeviceUff *self = (FpiDeviceUff *) device;
  while (self->power_phase != 1 && !result.done)
    g_main_context_iteration (NULL, TRUE);
  g_assert_false (result.done);
  g_autofree gchar *original_pid = g_strdup (g_subprocess_get_identifier (self->worker));
  gboolean normal = (!strcmp (scenario, "normal") || !strcmp (scenario, "closed-stdin") || !strcmp (scenario, "retained-sleep"));
  gboolean resume_error = !strcmp (scenario, "resume-error");
  for (guint cycle = 0; cycle < (normal ? 3 : 1); cycle++)
    {
      gboolean suspended_ok = fp_device_suspend_sync (device, NULL, &error);
      g_assert_cmpint (suspended_ok, ==, normal || resume_error);
      if (normal || resume_error)
        {
          g_assert_no_error (error);
          g_assert_false (result.done);
          g_assert_true (self->suspended);
          g_assert_cmpstr (g_subprocess_get_identifier (self->worker), ==, original_pid);
        }
      else
        {
          g_assert_error (error, FP_DEVICE_ERROR, FP_DEVICE_ERROR_NOT_SUPPORTED);
          g_clear_error (&error);
        }
      gboolean resumed_ok = fp_device_resume_sync (device, NULL, &error);
      g_assert_cmpint (resumed_ok, ==, !resume_error);
      if (resume_error)
        { g_assert_nonnull (error); g_clear_error (&error); }
      else
        g_assert_no_error (error);
      if (normal)
        {
          g_assert_false (result.done);
          g_assert_false (self->suspended);
          g_assert_cmpstr (g_subprocess_get_identifier (self->worker), ==, original_pid);
        }
    }
  g_cancellable_cancel (cancellation);
  while (!result.done)
    g_main_context_iteration (NULL, TRUE);
  g_assert_nonnull (result.error);
  g_clear_error (&result.error);
  g_assert_null (self->worker);
  g_assert_true (fp_device_close_sync (device, NULL, &error));
  g_assert_no_error (error);
  if (saved_stdin >= 0)
    { g_assert_cmpint (dup2 (saved_stdin, 0), ==, 0); close (saved_stdin); }
  g_unlink (filename);
}

static guint
fd_count (void)
{
  g_autoptr(GDir) dir = g_dir_open ("/proc/self/fd", 0, NULL);
  guint count = 0;
  while (g_dir_read_name (dir)) count++;
  return count;
}

static void
retry_sequence (void)
{
  g_autoptr(GError) error = NULL;
  g_autofree gchar *filename = NULL;
  int fd = g_file_open_tmp ("uff-retry-test-XXXXXX", &filename, &error);
  g_assert_no_error (error);
  close (fd);
  const char *program =
    "#!/usr/bin/python3\nimport sys\n"
    "if sys.argv[1]!='verify':\n print('UFF1 result 0 0',flush=True)\n sys.exit(0)\n"
    "print('UFF1 power 0',flush=True)\n"
    "for line in sys.stdin:\n"
    " if line=='suspend\\n': print('UFF1 suspended',flush=True)\n"
    " elif line=='resume\\n': print('UFF1 resumed',flush=True)\n"
    " elif line.startswith('finish '):\n"
    "  code=int(line.split()[1])\n"
    "  print('UFF1 power 1\\nUFF1 power 0',flush=True)\n"
    "  print(f'UFF1 result {code} {42 if code==0 else 0}',flush=True)\n"
    "  sys.exit(0)\n"
    "sys.exit(9)\n";
  g_assert_true (g_file_set_contents (filename, program, -1, &error));
  g_assert_no_error (error);
  g_assert_cmpint (g_chmod (filename, 0700), ==, 0);
  test_helper = filename;
  g_autoptr(FpDevice) device = g_object_new (fpi_device_uff_get_type (), NULL);
  g_assert_true (fp_device_open_sync (device, NULL, &error));
  g_assert_no_error (error);
  g_autoptr(FpPrint) print = make_print (device, 42);
  FpiDeviceUff *self = (FpiDeviceUff *) device;
  int saved_stdin = dup (0);
  close (0);
  guint before_fds = fd_count ();
  const guint codes[] = {1, 3, 0, 1, 3, 0};
  for (guint attempt = 0; attempt < G_N_ELEMENTS (codes); attempt++)
    {
      VerifyResult result = {0};
      fp_device_verify (device, print, NULL, NULL, NULL, NULL, verified, &result);
      while (self->power_phase != 1 && !result.done)
        g_main_context_iteration (NULL, TRUE);
      g_assert_false (result.done);
      g_assert_true (fp_device_suspend_sync (device, NULL, &error));
      g_assert_no_error (error);
      g_assert_true (fp_device_resume_sync (device, NULL, &error));
      g_assert_no_error (error);
      g_autofree gchar *finish = g_strdup_printf ("finish %u\n", codes[attempt]);
      g_assert_true (g_output_stream_write_all (self->control, finish,
                                                strlen (finish), NULL, NULL, &error));
      g_assert_no_error (error);
      while (!result.done) g_main_context_iteration (NULL, TRUE);
      if (codes[attempt] == 3)
        {
          g_assert_error (result.error, FP_DEVICE_RETRY, FP_DEVICE_RETRY_TOO_SHORT);
          g_clear_error (&result.error);
        }
      else
        {
          g_assert_no_error (result.error);
          g_assert_cmpint (result.match, ==, codes[attempt] == 0);
        }
      g_assert_null (self->worker);
      g_assert_null (self->control);
      /* fprintd retries an early lift in-place; Denial releases and claims
       * again following a terminal match or rejection. Exercise both. */
      if (codes[attempt] != 3)
        {
          g_assert_true (fp_device_close_sync (device, NULL, &error));
          g_assert_no_error (error);
          g_assert_true (fp_device_open_sync (device, NULL, &error));
          g_assert_no_error (error);
        }
    }
  g_assert_true (fp_device_close_sync (device, NULL, &error));
  g_assert_no_error (error);
  while (g_main_context_iteration (NULL, FALSE));
  g_assert_cmpuint (fd_count (), ==, before_fds);
  if (saved_stdin >= 0)
    { g_assert_cmpint (dup2 (saved_stdin, 0), ==, 0); close (saved_stdin); }
  g_unlink (filename);
}

int main (int argc, char **argv)
{
  g_test_init (&argc, &argv, NULL);
  g_test_add_data_func ("/uff/suspend/retained-session", "normal", exercise);
  g_test_add_data_func ("/uff/suspend/retained-sensor-sleep", "retained-sleep", exercise);
  g_test_add_data_func ("/uff/suspend/active-contact", "busy", exercise);
  g_test_add_data_func ("/uff/suspend/malformed-ack", "malformed", exercise);
  g_test_add_data_func ("/uff/suspend/missing-ack", "timeout", exercise);
  g_test_add_data_func ("/uff/suspend/failed-resume", "resume-error", exercise);
  g_test_add_data_func ("/uff/suspend/closed-stdin", "closed-stdin", exercise);
  g_test_add_func ("/uff/suspend/retry-sequence", retry_sequence);
  return g_test_run ();
}
