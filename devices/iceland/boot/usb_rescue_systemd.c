// SPDX-License-Identifier: MIT
// Same kernel and USB gadget as the proven rescue; hand off verified root to systemd.
#define RESCUE_MAINLINE 1
#ifndef RESCUE_MAINLINE_RELEASE
#error "Pin the exact matching kernel release at build time"
#endif
static int systemd_boot(void);
#define RESCUE_TRY_BOOT systemd_boot
#include "usb_rescue_persistent.c"

static int systemd_runtime_mount(const char *path)
{
    // The initramfs itself may be tmpfs. Filesystem type alone cannot tell
    // whether this directory is a mountpoint that MS_MOVE can transfer.
    return mkdir_ok(path) || mount("tmpfs", path, "tmpfs",
                                  MS_NOSUID | MS_NODEV, "mode=0755");
}

static int systemd_boot(void)
{
    static int attempted;
    if (attempted) return 0;
    attempted = 1;
    int children_stopped = 0;
    const char *stage = "initramfs identity";
    struct statfs fs;
    struct stat st;
    if (getpid() != 1 || statfs("/", &fs) ||
        (fs.f_type != RAMFS_MAGIC && fs.f_type != TMPFS_MAGIC) ||
        stat("/init", &st) || !S_ISREG(st.st_mode)) goto failed;
    // UFS enumeration is asynchronous; do not fall into rescue just because
    // storage was slower than the built-in USB controller on a cold boot.
    stage = "userdata identity";
    int storage_ready = 0;
    for (unsigned attempt = 0; attempt < 15; attempt++) {
        if (!prepare_userdata()) { storage_ready = 1; break; }
        sleep(1);
    }
    if (!storage_ready) goto failed;
    stage = "root mount";
    if (!is_arch_mount() && (mkdir_ok(ARCH_ROOT) ||
        mount(USERDATA, ARCH_ROOT, "ext4", MS_NOATIME, "nodiscard,errors=remount-ro"))) goto failed;
    stage = "root readiness";
    if (!is_arch_mount() ||
        !text_matches(ARCH_ROOT "/etc/pad4-root-id", "59b8e7b3:archroot-v1\n") ||
        !text_matches(ARCH_ROOT "/etc/pad4-systemd-ready", "pad4-systemd-20261013-v1\n") ||
        access(ARCH_ROOT "/usr/lib/systemd/systemd", X_OK) ||
        access(ARCH_ROOT "/usr/lib/ld-linux-aarch64.so.1", X_OK)) goto failed;

    // Validate the actual systemd executable and its dynamic dependencies first.
    stage = "systemd executable preflight";
    pid_t check = fork();
    if (!check) {
        if (chdir(ARCH_ROOT) || chroot(".") || chdir("/")) _exit(125);
        execl("/usr/lib/systemd/systemd", "systemd", "--version", (char *)NULL);
        _exit(126);
    }
    int status;
    if (check < 0 || waitpid(check, &status, 0) != check ||
        !WIFEXITED(status) || WEXITSTATUS(status)) goto failed;
    stage = "private mount tree and runtime tmpfs";
    if (mount(NULL, "/", NULL, MS_REC | MS_PRIVATE, NULL) ||
        systemd_runtime_mount("/run")) goto failed;

    // No service may retain the old initramfs across the handoff. Only this
    // supervisor's own children exist here (module loader, serial shell, network).
    stage = "reap initramfs children";
    FILE *children = fopen("/proc/self/task/1/children", "r");
    if (!children) goto failed;
    long child;
    while (fscanf(children, "%ld", &child) == 1)
        if (child > 1) kill((pid_t)child, SIGKILL);
    fclose(children);
    while (waitpid(-1, NULL, 0) > 0 || errno == EINTR) {}
    children_stopped = 1;
    arch_pid = network_pid = -1;
    network_ready = 0;

    const char *api[] = {"/run", "/sys", "/proc", "/dev"};
    unsigned moved = 0;
    for (; moved < sizeof(api)/sizeof(api[0]); moved++) {
        char destination[128];
        snprintf(destination, sizeof(destination), ARCH_ROOT "%s", api[moved]);
        stage = api[moved];
        if (mkdir_ok(destination) || mount(api[moved], destination, NULL, MS_MOVE, NULL))
            goto restore;
    }
    // Kernel gadget configuration survives; systemd-networkd takes over its IP.
    // BusyBox checks the ramfs boundary again and frees only the old initramfs.
    clearenv();
    setenv("PATH", "/usr/bin:/usr/sbin", 1);
    setenv("HOME", "/root", 1);
    setenv("TERM", "linux", 1);
    signal(SIGPIPE, SIG_DFL);
    stage = "exec switch_root";
    execl("/rescue/busybox", "busybox", "switch_root", "-c", "/dev/console",
          ARCH_ROOT, "/usr/lib/systemd/systemd", (char *)NULL);
restore:
    while (moved) {
        char source[128];
        --moved;
        snprintf(source, sizeof(source), ARCH_ROOT "%s", api[moved]);
        if (mount(source, api[moved], NULL, MS_MOVE, NULL)) {
            // Do not exit PID 1 even if a mount cannot be restored.
            for (;;) sleep(60);
        }
    }
failed:
    signal(SIGPIPE, SIG_IGN);
    logmsg("systemd handoff failed at %s: %s; retaining USB rescue without reboot", stage, strerror(errno));
    return children_stopped;
}
