// SPDX-License-Identifier: MIT
#define _GNU_SOURCE

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define UDC_CLASS_PATH "/sys/class/udc"
#define USB_ROLE_CLASS_PATH "/sys/class/usb_role"
#define CONFIGFS_PATH "/config"
#define GADGET_PATH "/config/usb_gadget/g1"
#define RETRY_MILLISECONDS 100
#define ROOT_RETRY_MILLISECONDS 2000
#define CHARGE_STATUS_MILLISECONDS 60000
#define USB_CONFIG_ATTEMPTS 20
#define USB_UDC_ATTEMPTS 50
#define MODULE_LOAD_TIMEOUT_MILLISECONDS 5000
#define MODULE_LOAD_POLL_MILLISECONDS 50
#define EARLY_MODULE_LIST_PATH "/lib/modules/aston-early.modules"
#define EARLY_LOG_PATH "/newroot/var/lib/aston-release/early-boot.log"
#define EARLY_LOG_SIZE 32768

static void log_status(const char *format, ...);
static char early_log[EARLY_LOG_SIZE];
static size_t early_log_length;
static bool usb_log_ready;
static bool prepare_usb_transport(void);

static void sleep_milliseconds(long milliseconds)
{
	struct timespec requested = {
		.tv_sec = milliseconds / 1000,
		.tv_nsec = (milliseconds % 1000) * 1000000L,
	};

	while (nanosleep(&requested, &requested) < 0 && errno == EINTR)
		;
}

static bool read_small_file(const char *path, char *buffer, size_t size)
{
	ssize_t count;
	int fd;

	if (size < 2)
		return false;
	fd = open(path, O_RDONLY | O_CLOEXEC);
	if (fd < 0)
		return false;
	count = read(fd, buffer, size - 1);
	close(fd);
	if (count < 0)
		return false;
	buffer[count] = '\0';
	return true;
}

static bool text_has_word(const char *text, const char *word)
{
	size_t length = strlen(word);
	const char *cursor = text;

	while (*cursor != '\0') {
		const char *begin;

		while (*cursor == ' ' || *cursor == '\t' || *cursor == '\r' ||
		       *cursor == '\n')
			cursor++;
		begin = cursor;
		while (*cursor != '\0' && *cursor != ' ' && *cursor != '\t' &&
		       *cursor != '\r' && *cursor != '\n')
			cursor++;
		if ((size_t)(cursor - begin) == length &&
		    memcmp(begin, word, length) == 0)
			return true;
	}
	return false;
}

static bool charge_only_requested(void)
{
	char command_line[4096];

	return read_small_file("/proc/cmdline", command_line,
			       sizeof(command_line)) &&
	       text_has_word(command_line, "aston.charge_only=1");
}

static const char *read_supply_value(const char *path, char *buffer, size_t size)
{
	size_t length;

	if (!read_small_file(path, buffer, size)) {
		strncpy(buffer, "unavailable", size);
		buffer[size - 1] = '\0';
		return buffer;
	}
	length = strlen(buffer);
	while (length > 0 && (buffer[length - 1] == '\n' ||
			      buffer[length - 1] == '\r' ||
			      buffer[length - 1] == ' ' ||
			      buffer[length - 1] == '\t'))
		buffer[--length] = '\0';
	return buffer;
}

static void log_charge_status(void)
{
	char status[32], capacity[32], voltage[32], current[32];
	char usb_online[32], usb_type[32], usb_current_max[32];

	log_status(
		"<6>ASTON CHARGE: status=%s capacity=%s%% "
		"battery_uv=%s battery_ua=%s usb_online=%s "
		"usb_type=%s usb_current_max_ua=%s\n",
		read_supply_value("/sys/class/power_supply/qcom-battmgr-bat/status",
				  status, sizeof(status)),
		read_supply_value("/sys/class/power_supply/qcom-battmgr-bat/capacity",
				  capacity, sizeof(capacity)),
		read_supply_value("/sys/class/power_supply/qcom-battmgr-bat/voltage_now",
				  voltage, sizeof(voltage)),
		read_supply_value("/sys/class/power_supply/qcom-battmgr-bat/current_now",
				  current, sizeof(current)),
		read_supply_value("/sys/class/power_supply/qcom-battmgr-usb/online",
				  usb_online, sizeof(usb_online)),
		read_supply_value("/sys/class/power_supply/qcom-battmgr-usb/usb_type",
				  usb_type, sizeof(usb_type)),
		read_supply_value("/sys/class/power_supply/qcom-battmgr-usb/current_max",
				  usb_current_max, sizeof(usb_current_max)));
}

static void run_charge_only(void)
{
	log_status("<5>ASTON CHARGE: low-power charge-only mode; "
		   "desktop and USB gadget suppressed\n");
	sleep_milliseconds(5000);
	for (;;) {
		log_charge_status();
		sleep_milliseconds(CHARGE_STATUS_MILLISECONDS);
	}
}

static bool write_text_file(const char *path, const char *text)
{
	const char *cursor = text;
	size_t remaining = strlen(text);
	int fd = open(path, O_WRONLY | O_CLOEXEC);

	if (fd < 0)
		return false;
	while (remaining > 0) {
		ssize_t written = write(fd, cursor, remaining);

		if (written > 0) {
			cursor += written;
			remaining -= (size_t)written;
			continue;
		}
		if (written < 0 && errno == EINTR)
			continue;
		close(fd);
		return false;
	}
	close(fd);
	return true;
}

static bool make_directory(const char *path)
{
	return mkdir(path, 0755) == 0 || errno == EEXIST;
}

static bool make_link(const char *target, const char *path)
{
	return symlink(target, path) == 0 || errno == EEXIST;
}

static void log_status(const char *format, ...)
{
	char buffer[512];
	va_list arguments;
	int length;
	int fd;
	size_t append_length;

	va_start(arguments, format);
	length = vsnprintf(buffer, sizeof(buffer), format, arguments);
	va_end(arguments);
	if (length < 0)
		return;
	if ((size_t)length >= sizeof(buffer))
		length = (int)sizeof(buffer) - 1;
	append_length = (size_t)length;
	if (append_length > sizeof(early_log) - early_log_length)
		append_length = sizeof(early_log) - early_log_length;
	memcpy(early_log + early_log_length, buffer, append_length);
	early_log_length += append_length;
	fd = open("/dev/kmsg", O_WRONLY | O_CLOEXEC);
	if (fd >= 0) {
		(void)write(fd, buffer, (size_t)length);
		close(fd);
	}
	if (usb_log_ready) {
		fd = open("/dev/ttyGS0", O_WRONLY | O_CLOEXEC | O_NONBLOCK);
		if (fd >= 0) {
			(void)write(fd, buffer, (size_t)length);
			close(fd);
		}
	}
}

static void start_usb_log(void)
{
	int fd = open("/dev/ttyGS0", O_WRONLY | O_CLOEXEC | O_NONBLOCK);

	if (fd < 0)
		return;
	(void)write(fd, early_log, early_log_length);
	close(fd);
	usb_log_ready = true;
}

static void persist_early_log(void)
{
	size_t offset = 0;
	int fd = open(EARLY_LOG_PATH,
		      O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);

	if (fd < 0)
		return;
	while (offset < early_log_length) {
		ssize_t written = write(fd, early_log + offset,
					 early_log_length - offset);

		if (written > 0) {
			offset += (size_t)written;
			continue;
		}
		if (written < 0 && errno == EINTR)
			continue;
		break;
	}
	(void)fsync(fd);
	close(fd);
}

static bool mount_if_needed(const char *source, const char *target,
			    const char *filesystem, unsigned long flags,
			    const char *data)
{
	return mount(source, target, filesystem, flags, data) == 0 || errno == EBUSY;
}

static void prepare_virtual_filesystems(void)
{
	make_directory("/dev");
	make_directory("/proc");
	make_directory("/sys");
	make_directory("/run");
	make_directory("/newroot");
	make_directory("/oplusreserve");
	(void)mount_if_needed("devtmpfs", "/dev", "devtmpfs", MS_NOSUID,
			      "mode=0755");
	(void)mount_if_needed("proc", "/proc", "proc", MS_NOSUID | MS_NOEXEC,
			      NULL);
	(void)mount_if_needed("sysfs", "/sys", "sysfs", MS_NOSUID | MS_NOEXEC,
			      NULL);
	(void)mount_if_needed("tmpfs", "/run", "tmpfs", MS_NOSUID | MS_NODEV,
			      "mode=0755");
}

static bool module_name_valid(const char *name)
{
	const unsigned char *cursor = (const unsigned char *)name;

	if (*cursor == '\0')
		return false;
	for (; *cursor != '\0'; cursor++) {
		if ((*cursor >= 'a' && *cursor <= 'z') ||
		    (*cursor >= 'A' && *cursor <= 'Z') ||
		    (*cursor >= '0' && *cursor <= '9') || *cursor == '_' ||
		    *cursor == '-' || *cursor == '.')
			continue;
		return false;
	}
	return true;
}

static bool load_module_with_timeout(const char *module, const char *path)
{
	int fd;
	pid_t child;
	int status = 0;
	int attempt;

	fd = open(path, O_RDONLY | O_CLOEXEC);
	if (fd < 0) {
		log_status("<3>ASTON TRACE: cannot open module %s: %s\n",
			   module, strerror(errno));
		return false;
	}
	log_status("<6>ASTON TRACE: finit_module begin: %s\n", module);
	child = fork();
	if (child < 0) {
		log_status("<3>ASTON TRACE: cannot fork module worker %s: %s\n",
			   module, strerror(errno));
		close(fd);
		return false;
	}
	if (child == 0) {
		long result;
		int error;

		errno = 0;
		result = syscall(SYS_finit_module, fd, "", 0);
		error = errno;
		close(fd);
		if (result == 0 || error == EEXIST)
			_exit(0);
		_exit(error > 0 && error < 255 ? error : 1);
	}
	close(fd);
	for (attempt = 0;
	     attempt < MODULE_LOAD_TIMEOUT_MILLISECONDS /
		       MODULE_LOAD_POLL_MILLISECONDS;
	     attempt++) {
		pid_t result = waitpid(child, &status, WNOHANG);

		if (result == child) {
			if (WIFEXITED(status) && WEXITSTATUS(status) == 0) {
				log_status("<6>ASTON TRACE: finit_module ready: %s\n",
					   module);
				return true;
			}
			if (WIFEXITED(status)) {
				int error = WEXITSTATUS(status);

				log_status("<3>ASTON TRACE: finit_module failed: %s: "
					   "%s (%d)\n", module, strerror(error),
					   error);
			} else {
				log_status("<3>ASTON TRACE: module worker ended "
					   "abnormally: %s\n", module);
			}
			return false;
		}
		if (result < 0 && errno != EINTR) {
			log_status("<3>ASTON TRACE: waitpid failed: %s: %s\n",
				   module, strerror(errno));
			return false;
		}
		sleep_milliseconds(MODULE_LOAD_POLL_MILLISECONDS);
	}
	log_status("<2>ASTON TRACE: finit_module TIMEOUT: %s after %d ms\n",
		   module, MODULE_LOAD_TIMEOUT_MILLISECONDS);
	(void)kill(child, SIGKILL);
	(void)waitpid(child, &status, WNOHANG);
	return false;
}

static bool load_early_modules(void)
{
	char module[NAME_MAX + 1];
	char path[PATH_MAX];
	FILE *list;
	bool loaded_all = true;
	size_t loaded = 0;
	size_t index = 0;

	list = fopen(EARLY_MODULE_LIST_PATH, "re");
	if (!list) {
		log_status("<3>ASTON DEV: early module list missing: %s\n",
			   strerror(errno));
		return false;
	}
	while (fgets(module, sizeof(module), list) != NULL) {
		size_t length = strcspn(module, "\r\n");

		module[length] = '\0';
		if (module[0] == '\0' || module[0] == '#')
			continue;
		index++;
		if (!module_name_valid(module) ||
		    snprintf(path, sizeof(path), "/lib/modules/%s.ko", module) >=
			    (int)sizeof(path)) {
			log_status("<3>ASTON DEV: invalid early module name: %s\n",
				   module);
			loaded_all = false;
			continue;
		}
		log_status("<6>ASTON TRACE: module %zu: %s\n", index, module);
		if (!load_module_with_timeout(module, path)) {
			loaded_all = false;
		} else {
			loaded++;
		}
		if (!usb_log_ready &&
		    strcmp(module, "phy_qcom_eusb2_repeater") == 0) {
			log_status("<6>ASTON TRACE: USB checkpoint after module %zu\n",
				   index);
			(void)prepare_usb_transport();
		}
	}
	if (ferror(list))
		loaded_all = false;
	fclose(list);
	log_status("<6>ASTON DEV: early module load complete: %zu modules\n",
		   loaded);
	return loaded_all && loaded > 0;
}

static bool configure_composite_gadget(void)
{
	make_directory(CONFIGFS_PATH);
	if (!mount_if_needed("configfs", CONFIGFS_PATH, "configfs", 0, NULL))
		return false;
	if (!make_directory("/config/usb_gadget") ||
	    !make_directory(GADGET_PATH) ||
	    !make_directory(GADGET_PATH "/strings") ||
	    !make_directory(GADGET_PATH "/strings/0x409") ||
	    !make_directory(GADGET_PATH "/configs") ||
	    !make_directory(GADGET_PATH "/configs/b.1") ||
	    !make_directory(GADGET_PATH "/configs/b.1/strings") ||
	    !make_directory(GADGET_PATH "/configs/b.1/strings/0x409") ||
	    !make_directory(GADGET_PATH "/functions") ||
	    !make_directory(GADGET_PATH "/functions/acm.GS0") ||
	    !make_directory(GADGET_PATH "/functions/ncm.usb0"))
		return false;
	if (!write_text_file(GADGET_PATH "/idVendor", "0x18d1\n") ||
	    !write_text_file(GADGET_PATH "/idProduct", "0x4ee7\n") ||
	    !write_text_file(GADGET_PATH "/strings/0x409/manufacturer", "OnePlus\n") ||
	    !write_text_file(GADGET_PATH "/strings/0x409/product",
			     "Aston Linux first boot\n") ||
	    !write_text_file(GADGET_PATH "/strings/0x409/serialnumber",
			     "f683d9c9\n") ||
	    !write_text_file(GADGET_PATH "/configs/b.1/strings/0x409/configuration",
			     "acm-ncm\n") ||
	    !write_text_file(GADGET_PATH "/configs/b.1/MaxPower", "500\n") ||
	    !write_text_file(GADGET_PATH "/functions/ncm.usb0/dev_addr",
			     "02:18:d1:4e:e7:02\n") ||
	    !write_text_file(GADGET_PATH "/functions/ncm.usb0/host_addr",
			     "02:18:d1:4e:e7:01\n"))
		return false;
	return make_link(GADGET_PATH "/functions/acm.GS0",
			 GADGET_PATH "/configs/b.1/acm.GS0") &&
	       make_link(GADGET_PATH "/functions/ncm.usb0",
			 GADGET_PATH "/configs/b.1/ncm.usb0");
}

static void select_device_roles(void)
{
	char path[512];
	struct dirent *entry;
	DIR *directory = opendir(USB_ROLE_CLASS_PATH);

	if (!directory)
		return;
	while ((entry = readdir(directory)) != NULL) {
		if (entry->d_name[0] == '.')
			continue;
		if (snprintf(path, sizeof(path), "%s/%s/role", USB_ROLE_CLASS_PATH,
			     entry->d_name) >= (int)sizeof(path))
			continue;
		(void)write_text_file(path, "device\n");
	}
	closedir(directory);
}

static bool bind_first_udc(void)
{
	struct dirent *entry;
	DIR *directory = opendir(UDC_CLASS_PATH);
	bool bound = false;

	if (!directory)
		return false;
	while ((entry = readdir(directory)) != NULL) {
		char name[NAME_MAX + 2];

		if (entry->d_name[0] == '.')
			continue;
		if (snprintf(name, sizeof(name), "%s\n", entry->d_name) >=
		    (int)sizeof(name))
			continue;
		bound = write_text_file(GADGET_PATH "/UDC", name);
		break;
	}
	closedir(directory);
	return bound;
}

static bool prepare_usb_transport(void)
{
	int attempt;

	if (usb_log_ready)
		return true;

	for (attempt = 0; attempt < USB_CONFIG_ATTEMPTS; attempt++) {
		if (configure_composite_gadget())
			break;
		sleep_milliseconds(RETRY_MILLISECONDS);
	}
	if (attempt == USB_CONFIG_ATTEMPTS) {
		log_status("<4>ASTON DEV: composite ConfigFS gadget unavailable; "
			   "continuing root discovery\n");
		return false;
	}
	log_status("<6>ASTON DEV: proven single-ACM function ready\n");
	for (attempt = 0; attempt < USB_UDC_ATTEMPTS; attempt++) {
		select_device_roles();
		if (bind_first_udc()) {
			start_usb_log();
			log_status("<6>ASTON DEV: device role selected and UDC bound\n");
			return true;
		}
		sleep_milliseconds(RETRY_MILLISECONDS);
	}
	log_status("<4>ASTON DEV: UDC unavailable; continuing root discovery\n");
	return false;
}

static bool contains_systemd(const char *root)
{
	char path[PATH_MAX];

	if (snprintf(path, sizeof(path), "%s/usr/lib/systemd/systemd", root) <
	    (int)sizeof(path) && access(path, X_OK) == 0)
		return true;
	return snprintf(path, sizeof(path), "%s/sbin/init", root) <
		       (int)sizeof(path) && access(path, X_OK) == 0;
}

static bool mount_arch_root(void)
{
	char device[32];
	char disk;

	/* UFS LUN discovery order changes when other built-in drivers probe. */
	for (disk = 'a'; disk <= 'z'; disk++) {
		if (snprintf(device, sizeof(device), "/dev/sd%c14", disk) >=
		    (int)sizeof(device) || access(device, F_OK) != 0)
			continue;
		if (mount(device, "/newroot", "ext4", MS_NOATIME, NULL) < 0 &&
		    errno != EBUSY)
			continue;
		if (contains_systemd("/newroot")) {
			log_status("<6>ASTON DEV: Arch root found directly on %s\n",
				   device);
			return true;
		}
		(void)umount2("/newroot", MNT_DETACH);
	}

	for (disk = 'a'; disk <= 'z'; disk++) {
		if (snprintf(device, sizeof(device), "/dev/sd%c10", disk) >=
		    (int)sizeof(device) || access(device, F_OK) != 0)
			continue;
		if (mount(device, "/oplusreserve", "ext4", MS_NOATIME, NULL) < 0 &&
		    errno != EBUSY)
			continue;
		if (contains_systemd("/oplusreserve/arch-root")) {
			if (mount("/oplusreserve/arch-root", "/newroot", NULL,
				  MS_BIND | MS_REC, NULL) == 0 || errno == EBUSY) {
				log_status("<6>ASTON DEV: Arch root found at %s:/arch-root\n",
					   device);
				return true;
			}
		}
		(void)umount2("/oplusreserve", MNT_DETACH);
	}
	return false;
}

static bool bind_into_root(const char *source, const char *relative)
{
	char target[PATH_MAX];

	if (snprintf(target, sizeof(target), "/newroot/%s", relative) >=
	    (int)sizeof(target))
		return false;
	if (!make_directory(target))
		return false;
	return mount(source, target, NULL, MS_BIND | MS_REC, NULL) == 0 ||
	       errno == EBUSY;
}

static bool pivot_to_arch_root(void)
{
	char old_root[] = "/newroot/.aston-oldroot-XXXXXX";
	const char *detached_root = old_root + strlen("/newroot");
	/*
	 * pivot_root() requires a mount point for the new root and a directory
	 * underneath it for the old root.  The latter is deliberately detached
	 * after the pivot: leaving the initramfs mounted at / makes
	 * current_chrooted() reject otherwise valid user-namespace creation.
	 */
	/* Never reuse a pre-existing directory or follow a put_old symlink. */
	if (!mkdtemp(old_root))
		return false;
	if (mount(NULL, "/", NULL, MS_REC | MS_PRIVATE, NULL) < 0) {
		log_status("<3>ASTON DEV: root mount privatization failed: %s\n",
			   strerror(errno));
		return false;
	}
	if (syscall(SYS_pivot_root, "/newroot", old_root) < 0) {
		log_status("<3>ASTON DEV: pivot_root failed: %s\n", strerror(errno));
		return false;
	}
	if (chdir("/") < 0) {
		log_status("<3>ASTON DEV: post-pivot chdir failed: %s\n",
			   strerror(errno));
		return false;
	}
	if (umount2(detached_root, MNT_DETACH) < 0) {
		log_status("<3>ASTON DEV: old-root detach failed: %s\n",
			   strerror(errno));
		return false;
	}
	if (rmdir(detached_root) < 0) {
		log_status("<4>ASTON DEV: old-root directory removal failed: %s\n",
			   strerror(errno));
	}
	return true;
}

static void handoff_to_arch(void)
{
	const char *init_path;
	bool usb_ready = usb_log_ready;
	char *const environment[] = {
		"HOME=/root",
		"PATH=/usr/local/sbin:/usr/local/bin:/usr/bin:/usr/sbin:/bin:/sbin",
		"TERM=linux",
		NULL,
	};
	char *arguments[] = { "/sbin/init", NULL };

	while (!mount_arch_root()) {
		if (!usb_ready)
			usb_ready = prepare_usb_transport();
		log_status("<6>ASTON DEV: waiting for Arch root\n");
		sleep_milliseconds(ROOT_RETRY_MILLISECONDS);
	}
	if (!usb_ready)
		(void)prepare_usb_transport();
	if (!bind_into_root("/dev", "dev") || !bind_into_root("/proc", "proc") ||
	    !bind_into_root("/sys", "sys") || !bind_into_root("/run", "run")) {
		log_status("<3>ASTON DEV: virtual filesystem bind failed: %s\n",
			   strerror(errno));
		return;
	}
	/* Retain the gadget's ConfigFS mount when the initramfs is detached. */
	if (access(CONFIGFS_PATH "/usb_gadget", F_OK) == 0 &&
	    !bind_into_root(CONFIGFS_PATH, "config")) {
		log_status("<3>ASTON DEV: ConfigFS handoff failed: %s\n",
			   strerror(errno));
		return;
	}
	init_path = access("/newroot/usr/lib/systemd/systemd", X_OK) == 0 ?
		"/usr/lib/systemd/systemd" : "/sbin/init";
	log_status("<6>ASTON DEV: handing PID 1 to Arch %s\n", init_path);
	persist_early_log();
	if (!pivot_to_arch_root()) {
		log_status("<3>ASTON DEV: root handoff failed\n");
		return;
	}
	log_status("<6>ASTON DEV: root pivot complete; old initramfs detached\n");
	arguments[0] = (char *)init_path;
	execve(init_path, arguments, environment);
	log_status("<3>ASTON DEV: Arch init exec failed: %s\n", strerror(errno));
}

#ifdef ASTON_SELFTEST
int main(void)
{
	if (!text_has_word("quiet aston.charge_only=1 loglevel=6",
			   "aston.charge_only=1") ||
	    text_has_word("aston.charge_only=10", "aston.charge_only=1") ||
	    !module_name_valid("ufs_qcom") ||
	    module_name_valid("../ufs_qcom"))
		return 1;
	puts("firstboot init self-test passed");
	return 0;
}
#else
int main(void)
{
	prepare_virtual_filesystems();
	(void)sethostname("aston", 5);
	if (!load_early_modules())
		log_status("<3>ASTON DEV: early module closure incomplete\n");
	if (charge_only_requested())
		run_charge_only();
	handoff_to_arch();
	for (;;)
		pause();
}
#endif
