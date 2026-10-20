// SPDX-License-Identifier: MIT
// Stock-kernel USB rescue PID 1. No reboot, power-off, slot or storage writes.
#define _GNU_SOURCE
#include <dirent.h>
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>
#ifdef RESCUE_MAINLINE
#include <sys/utsname.h>
#include "pad4_kernel_pair.h"
#endif

static void logmsg(const char *format, ...)
{
    char buf[1024];
    va_list ap;
    va_start(ap, format);
    vsnprintf(buf, sizeof(buf), format, ap);
    va_end(ap);
    const char *paths[] = {"/dev/kmsg", "/rescue.log"};
    for (unsigned i = 0; i < 2; i++) {
        int fd = open(paths[i], O_WRONLY | O_CLOEXEC |
                      (i ? O_CREAT | O_APPEND : 0), 0600);
        if (fd >= 0) {
            dprintf(fd, "usb-rescue: %s\n", buf);
            close(fd);
        }
    }
}

static int mkdir_ok(const char *path)
{
    return mkdir(path, 0755) == 0 || errno == EEXIST ? 0 : -1;
}

static int write_text(const char *path, const char *value)
{
    int fd = open(path, O_WRONLY | O_CLOEXEC);
    if (fd < 0) return -1;
    size_t size = strlen(value);
    ssize_t n = write(fd, value, size);
    int saved = errno;
    close(fd);
    errno = saved;
    return n == (ssize_t)size ? 0 : -1;
}

static void node(const char *path, unsigned major_no, unsigned minor_no)
{
    if (mknod(path, S_IFCHR | 0600, makedev(major_no, minor_no)) && errno != EEXIST)
        logmsg("mknod %s: %s", path, strerror(errno));
}

static void mount_wait(const char *type, const char *path)
{
    for (;;) {
        mkdir_ok(path);
        if (!mount(type, path, type, MS_NOSUID, NULL) || errno == EBUSY) return;
        logmsg("mount %s at %s: %s; holding and retrying", type, path, strerror(errno));
        sleep(5);
    }
}

#ifndef RESCUE_MAINLINE
static int module_options(const char *path, const char *cmdline, char *out, size_t size)
{
    const char *base = strrchr(path, '/');
    base = base ? base+1 : path;
    size_t name_size = strlen(base);
    if (name_size > 3 && !strcmp(base+name_size-3, ".ko")) name_size -= 3;
    size_t used = 0;
    out[0] = 0;
    for (const char *p = cmdline; *p;) {
        while (isspace((unsigned char)*p)) p++;
        const char *start = p;
        int quoted = 0;
        while (*p && (quoted || !isspace((unsigned char)*p))) {
            if (*p == '"') quoted = !quoted;
            p++;
        }
        size_t length = (size_t)(p-start);
        if (length <= name_size+1 || start[name_size] != '.') continue;
        size_t i;
        for (i = 0; i < name_size; i++) {
            char a = base[i] == '-' ? '_' : base[i];
            char b = start[i] == '-' ? '_' : start[i];
            if (a != b) break;
        }
        if (i != name_size) continue;
        const char *option = start+name_size+1;
        size_t n = length-name_size-1;
        if (used+n+2 > size) return -1;
        if (used) out[used++] = ' ';
        memcpy(out+used, option, n);
        used += n;
        out[used] = 0;
    }
    return 0;
}

static void load_modules(void)
{
    // Independent child: a blocked probe must not stop PID 1 or USB supervision.
    char cmdline[65536] = {0};
    FILE *cmd = fopen("/proc/cmdline", "r");
    if (cmd) {
        size_t n = fread(cmdline, 1, sizeof(cmdline)-1, cmd);
        cmdline[n] = 0;
        fclose(cmd);
    }
    for (;;) {
        FILE *list = fopen("/rescue/modules.plan", "r");
        if (!list) {
            logmsg("cannot open module plan: %s", strerror(errno));
            sleep(15);
            continue;
        }
        char path[512];
        unsigned failed = 0;
        while (fgets(path, sizeof(path), list)) {
            path[strcspn(path, "\r\n")] = 0;
            if (!*path) continue;
            char options[65536];
            if (module_options(path, cmdline, options, sizeof(options))) {
                logmsg("module options too long for %s; holding", path);
                failed++;
                continue;
            }
            int fd = open(path, O_RDONLY | O_CLOEXEC);
            if (fd < 0) {
                logmsg("open module %s: %s", path, strerror(errno));
                failed++;
                continue;
            }
            logmsg("module attempt %s", path);
            if (syscall(SYS_finit_module, fd, options, 0) && errno != EEXIST) {
                logmsg("module %s: %s", path, strerror(errno));
                failed++;
            }
            close(fd);
        }
        fclose(list);
        logmsg("module pass complete; %u failures", failed);
        if (!failed) _exit(0);
        sleep(30);
    }
}

#else
static void load_modules(void)
{
    logmsg("mainline rescue: storage and USB dependencies are built in; stock module plan skipped");
    // Let the standard watchdog core service a watchdog left running by ABL.
    // This exact matching module has no loadable dependencies. Never start a
    // userspace watchdog or request a reboot; retain serial rescue on failure.
    struct utsname uts;
    const char *path = !uname(&uts) ? pad4_watchdog_path(uts.release) : NULL;
    if (!path) {
        logmsg("mainline watchdog: unregistered kernel; retaining rescue");
        _exit(1);
    }
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) logmsg("mainline watchdog module missing: %s",strerror(errno));
    else {
        if (syscall(SYS_finit_module,fd,"",0) && errno != EEXIST)
            logmsg("mainline watchdog module: %s",strerror(errno));
        close(fd);
    }
    _exit(0);
}
#endif

static int first_udc(char *name, size_t size)
{
    DIR *dir = opendir("/sys/class/udc");
    if (!dir) return -1;
    struct dirent *entry;
    int found = -1;
    while ((entry = readdir(dir))) {
        if (entry->d_name[0] == '.') continue;
        int n = snprintf(name, size, "%s", entry->d_name);
        if (n > 0 && n < (int)size) found = 0;
        break;
    }
    closedir(dir);
    return found;
}

static int request_peripheral(const char *device, int report)
{
#ifdef RESCUE_MAINLINE
    // The audited mainline FDT forces peripheral/high-speed before probing.
    (void)device;
    (void)report;
    return 0;
#else
    // Scope the request to the stock a600000 controller and its bound driver.
    // This is a volatile driver attribute, never a DTB or partition write.
    char path[512], compatible[128] = {0}, driver[512], mode[64];
    snprintf(path, sizeof(path), "%s/of_node/compatible", device);
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return -1;
    ssize_t n = read(fd, compatible, sizeof(compatible)-1);
    close(fd);
    if (n <= 0 || strcmp(compatible, "qcom,dwc-usb3-msm")) return -1;
    snprintf(path, sizeof(path), "%s/driver", device);
    n = readlink(path, driver, sizeof(driver)-1);
    if (n <= 0 || n >= (ssize_t)sizeof(driver)-1) return -1;
    driver[n] = 0;
    const char *name = strrchr(driver, '/');
    name = name ? name+1 : driver;
    if (strcmp(name, "msm-dwc3")) return -1;
    snprintf(path, sizeof(path), "%s/mode", device);
    fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return -1;
    n = read(fd, mode, sizeof(mode)-1);
    close(fd);
    if (n <= 0) return -1;
    mode[n] = 0;
    mode[strcspn(mode, "\r\n")] = 0;
    if (!strcmp(mode, "peripheral")) return 0;
    if (strcmp(mode, "none") && strcmp(mode, "host")) {
        if (report) logmsg("unexpected controller mode '%s'; not changing it", mode);
        return -1;
    }
    if (write_text(path, "peripheral\n")) {
        if (report) logmsg("request peripheral at %s: %s", path, strerror(errno));
        return -1;
    }
    return 1;
#endif
}

static int gadget_error(const char *operation, const char *path)
{
    int error = errno;
    logmsg("gadget %s %s: %s", operation, path, strerror(error));
    errno = error;
    return -1;
}

static int setup_gadget_at(const char *base, const char *udc)
{
    char path[512];
#define PATH(suffix) snprintf(path, sizeof(path), "%s/%s", base, suffix)
#define DIR(suffix) do { PATH(suffix); if (mkdir_ok(path)) return gadget_error("mkdir", path); } while (0)
#define PUT(suffix, value) do { PATH(suffix); if (write_text(path, value)) return gadget_error("write", path); } while (0)
    if (mkdir_ok(base)) return gadget_error("mkdir", base);
    // Local development gadget identifiers; no production USB identity claimed.
    PUT("idVendor", "0x1d6b");
    PUT("idProduct", "0x0104");
    PUT("bcdUSB", "0x0200");
    PUT("bcdDevice", "0x0100");
    DIR("strings/0x409");
    PUT("strings/0x409/serialnumber", "OPD2514-RESCUE");
    PUT("strings/0x409/manufacturer", "Local rescue");
    PUT("strings/0x409/product", "OPD2514 USB rescue shell");
    DIR("configs/c.1");
    PUT("configs/c.1/MaxPower", "250");
    DIR("configs/c.1/strings/0x409");
    PUT("configs/c.1/strings/0x409/configuration", "Rescue serial");
    DIR("functions/acm.usb0");
    PATH("configs/c.1/acm.usb0");
    // ConfigFS resolves the target at syscall time relative to cwd, unlike
    // ordinary filesystem symlinks. PID1's cwd is /, so use an absolute target.
    char target[512];
    snprintf(target, sizeof(target), "%s/functions/acm.usb0", base);
    if (symlink(target, path) && errno != EEXIST) return gadget_error("symlink", path);
#ifdef RESCUE_EXTRA_GADGET
    RESCUE_EXTRA_GADGET(base);
#endif
    PUT("UDC", udc);
#undef PUT
#undef DIR
#undef PATH
    return 0;
}

static int setup_gadget(const char *udc)
{
    return setup_gadget_at("/sys/kernel/config/usb_gadget/opd2514_rescue", udc);
}

static int serial_node(void)
{
    unsigned major_no, minor_no;
    FILE *f = fopen("/sys/class/tty/ttyGS0/dev", "r");
    if (!f) return -1;
    int n = fscanf(f, "%u:%u", &major_no, &minor_no);
    fclose(f);
    if (n != 2) return -1;
    node("/dev/ttyGS0", major_no, minor_no);
    return 0;
}

static void serial_shell(void)
{
    if (setsid() < 0) _exit(110);
    int fd = open("/dev/ttyGS0", O_RDWR);
    if (fd < 0) _exit(111);
    if (ioctl(fd, TIOCSCTTY, 0) < 0) _exit(112);
    struct termios tio;
    if (!tcgetattr(fd, &tio)) {
        tio.c_iflag = ICRNL;
        tio.c_oflag = OPOST | ONLCR;
        tio.c_cflag = CS8 | CREAD | CLOCAL | HUPCL;
        tio.c_lflag = ISIG | ICANON | ECHO | ECHOE | ECHOK;
        tio.c_cc[VINTR] = 3; tio.c_cc[VQUIT] = 28;
        tio.c_cc[VERASE] = 127; tio.c_cc[VKILL] = 21;
        tio.c_cc[VEOF] = 4; tio.c_cc[VSUSP] = 26;
        tio.c_cc[VMIN] = 1; tio.c_cc[VTIME] = 0;
        cfsetispeed(&tio, B115200);
        cfsetospeed(&tio, B115200);
        tcsetattr(fd, TCSANOW, &tio);
    }
    for (int i = 0; i < 3; i++) if (dup2(fd, i) < 0) _exit(113);
    if (fd > 2) close(fd);
    signal(SIGPIPE, SIG_DFL);
    setenv("PATH", "/rescue/bin:/system/bin", 1);
    setenv("HOME", "/", 1);
    setenv("TERM", "vt100", 1);
    setenv("PS1", "rescue# ", 1);
    puts("\nOPD2514 stock-kernel rescue. Logs: /rescue.log\n"
         "No automatic reboot/power-off. Exit respawns the shell.\n");
    fflush(stdout);
    execl("/rescue/busybox", "sh", "-i", (char *)NULL);
    _exit(114);
}

static pid_t supervised_fork(void)
{
    pid_t parent = getpid();
    pid_t child = fork();
    if (!child) {
        if (prctl(PR_SET_PDEATHSIG, SIGKILL) || getppid() != parent) _exit(121);
    }
    return child;
}

static _Noreturn void run_rescue(void)
{
    umask(022);
    signal(SIGPIPE, SIG_IGN);
    mount_wait("proc", "/proc");
    mount_wait("sysfs", "/sys");
#ifdef RESCUE_MAINLINE
    mount_wait("devtmpfs", "/dev");
#else
    // Stock CONFIG_DEVTMPFS is disabled. Device nodes are explicitly managed.
    mount_wait("tmpfs", "/dev");
#endif
    node("/dev/null", 1, 3); node("/dev/zero", 1, 5);
    node("/dev/random", 1, 8); node("/dev/urandom", 1, 9);
    node("/dev/kmsg", 1, 11); node("/dev/console", 5, 1);
    node("/dev/tty", 5, 0);
    mkdir_ok("/dev/pts");
    mount("devpts", "/dev/pts", "devpts", MS_NOSUID, "mode=0620,ptmxmode=0666");
    symlink("pts/ptmx", "/dev/ptmx");
    symlink("/proc/self/fd", "/dev/fd");
    symlink("/proc/self/fd/0", "/dev/stdin");
    symlink("/proc/self/fd/1", "/dev/stdout");
    symlink("/proc/self/fd/2", "/dev/stderr");
    int nullfd = open("/dev/null", O_RDWR);
    if (nullfd >= 0) {
        for (int i = 0; i < 3; i++) dup2(nullfd, i);
        if (nullfd > 2) close(nullfd);
    }
    mkdir_ok("/tmp"); chmod("/tmp", 01777);
    mount_wait("configfs", "/sys/kernel/config");
#ifdef RESCUE_TRY_BOOT
    logmsg("Mainline systemd bootstrap started; preflight failures hold in rescue");
#elif defined(RESCUE_MAINLINE)
    logmsg("Mainline rescue PID 1 started; no automatic reboot or Android handoff");
#else
    logmsg("PID 1 v2 started; stock panic/watchdog settings untouched");
#endif
    pid_t loader = -1, shell = -1;
    int modules_done = 0, bound = 0;
    for (unsigned tick = 0;; tick++) {
        int status;
        pid_t dead;
        while ((dead = waitpid(-1, &status, WNOHANG)) > 0) {
            logmsg("child %ld ended, wait status %d", (long)dead, status);
            if (dead == loader) {
                modules_done = WIFEXITED(status) && WEXITSTATUS(status) == 0;
                loader = -1;
            }
            if (dead == shell) shell = -1;
#ifdef RESCUE_EXTRA_REAP
            RESCUE_EXTRA_REAP(dead, status);
#endif
        }
        if (loader < 0 && !modules_done) {
            loader = supervised_fork();
            if (!loader) load_modules();
        }
        // The glue driver can wait for role/VBUS notification before creating
        // its DWC3 child/UDC. Request device mode BEFORE waiting for a UDC.
        int role = request_peripheral("/sys/bus/platform/devices/a600000.ssusb", !(tick % 15));
        if (role == 1) logmsg("requested peripheral mode on a600000.ssusb before UDC setup");
        char udc[256];
        if (!bound && !first_udc(udc, sizeof(udc))) {
            if (!setup_gadget(udc)) {
                bound = 1;
                logmsg("ACM gadget bound to %s", udc);
            } else if (!(tick % 15)) logmsg("USB setup: %s; retrying", strerror(errno));
        }
#ifdef RESCUE_TRY_BOOT
        // Boot must not depend on a connected host or a configured USB link.
        if (modules_done && RESCUE_TRY_BOOT()) loader = shell = -1;
#endif
        if (bound && shell < 0 && !serial_node()) {
            shell = supervised_fork();
            if (!shell) serial_shell();
        }
#ifdef RESCUE_EXTRA_TICK
        RESCUE_EXTRA_TICK(tick, bound, modules_done);
#endif
        if (!(tick % 30)) logmsg("alive; role result=%d; USB bound=%d; modules complete=%d",
                                role, bound, modules_done);
        sleep(2);
    }
}

int main(void)
{
    // Prevent accidental execution on the workstation during offline validation.
    if (getpid() != 1) {
        fputs("This executable is a rescue PID 1 only.\n", stderr);
        return 2;
    }
    run_rescue();
}
