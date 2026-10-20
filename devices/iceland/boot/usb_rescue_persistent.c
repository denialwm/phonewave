// SPDX-License-Identifier: MIT
// Persistent Arch services under the proven rescue supervisor; no Android handoff.
#define _GNU_SOURCE
#include <sys/types.h>
static void persistent_gadget(const char *base);
static void persistent_reap(pid_t child, int status);
static void persistent_tick(unsigned tick, int bound, int modules_done);
#define RESCUE_EXTRA_GADGET persistent_gadget
#define RESCUE_EXTRA_REAP persistent_reap
#define RESCUE_EXTRA_TICK persistent_tick
#define main rescue_base_main
#include "usb_rescue_init.c"
#undef main
#include <sys/vfs.h>
#include <linux/magic.h>

#define USERDATA "/dev/pad4-userdata"
#define ARCH_ROOT "/arch"
#ifndef ARCH_SSH_PORT
#define ARCH_SSH_PORT "22"
#endif
static pid_t network_pid = -1, arch_pid = -1;
static int network_ready;
static unsigned network_retry, arch_retry, current_tick;
#ifdef RESCUE_MAINLINE
#include <sys/utsname.h>
static dev_t userdata_dev;
#else
#define userdata_dev makedev(8,15)
#endif

static int text_matches(const char *path, const char *expected)
{
    char buf[4096];
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return 0;
    ssize_t n = read(fd, buf, sizeof(buf)-1);
    close(fd);
    if (n < 0) return 0;
    buf[n] = 0;
    return !strcmp(buf, expected);
}

static int file_has_line(const char *path, const char *expected)
{
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    char line[1024];
    int found = 0;
    while (fgets(line, sizeof(line), f))
        if (!strcmp(line, expected)) { found = 1; break; }
    fclose(f);
    return found;
}

#ifdef RESCUE_MAINLINE
static int userdata_find(const char *block_root)
{
    DIR *dir = opendir(block_root);
    if (!dir) return 0;
    struct dirent *entry;
    int matches = 0;
    while ((entry = readdir(dir))) {
        char path[512];
        if (entry->d_name[0] == '.') continue;
        snprintf(path,sizeof(path),"%s/%s/uevent",block_root,entry->d_name);
        if (!file_has_line(path,"PARTNAME=userdata\n") ||
            !file_has_line(path,"PARTUUID=53706c8e-14e3-4332-9e0b-858c954286f9\n")) continue;
        snprintf(path,sizeof(path),"%s/%s/start",block_root,entry->d_name);
        if (!text_matches(path,"31627104\n")) continue;
        snprintf(path,sizeof(path),"%s/%s/size",block_root,entry->d_name);
        if (!text_matches(path,"463202424\n")) continue;
        snprintf(path,sizeof(path),"%s/%s/dev",block_root,entry->d_name);
        FILE *f = fopen(path,"r");
        unsigned major_no, minor_no;
        if (!f) continue;
        if (fscanf(f,"%u:%u",&major_no,&minor_no) == 2) {
            userdata_dev = makedev(major_no,minor_no);
            matches++;
        }
        fclose(f);
    }
    closedir(dir);
    return matches == 1;
}
#endif

static int userdata_identity(void)
{
#ifdef RESCUE_MAINLINE
    struct utsname uts;
    return !uname(&uts) && pad4_release_supported(uts.release) &&
        // 7.3 kernels mark the handoff generically; the 7.2 kernels used pad4,handoff.
        (!access("/sys/firmware/devicetree/base/chosen/linux,fdt-handoff", F_OK) ||
         text_matches("/sys/firmware/devicetree/base/chosen/pad4,handoff", "stock-abl-v1")) &&
        file_has_line("/proc/bootconfig", "androidboot.serialno = \"59b8e7b3\"\n") &&
        file_has_line("/proc/bootconfig", "androidboot.slot_suffix = \"_b\"\n") &&
        userdata_find("/sys/class/block");
#else
    return file_has_line("/proc/bootconfig", "androidboot.serialno = \"59b8e7b3\"\n") &&
        file_has_line("/proc/bootconfig", "androidboot.slot_suffix = \"_b\"\n") &&
        file_has_line("/sys/class/block/sda15/uevent", "PARTNAME=userdata\n") &&
        file_has_line("/sys/class/block/sda15/uevent", "PARTUUID=53706c8e-14e3-4332-9e0b-858c954286f9\n") &&
        text_matches("/sys/class/block/sda15/start", "31627104\n") &&
        text_matches("/sys/class/block/sda15/size", "463202424\n") &&
        text_matches("/sys/class/block/sda15/dev", "8:15\n");
#endif
}

static int ext4_identity(int fd)
{
    unsigned char sb[1024];
    const unsigned char uuid[] = {0x4a,0x42,0x17,0xbb,0x85,0x5b,0x45,0xb8,
                                 0x88,0x50,0xd6,0xca,0x73,0x88,0x5e,0x8c};
    return pread(fd, sb, sizeof(sb), 1024) == (ssize_t)sizeof(sb) &&
        sb[0x38] == 0x53 && sb[0x39] == 0xef &&
        !memcmp(sb+0x68, uuid, sizeof(uuid));
}

static int prepare_userdata(void)
{
    if (!userdata_identity()) { logmsg("userdata identity mismatch; holding in rescue"); return -1; }
    if (mknod(USERDATA, S_IFBLK | 0600, userdata_dev) && errno != EEXIST) return -1;
    struct stat st;
    if (lstat(USERDATA, &st) || !S_ISBLK(st.st_mode) || st.st_rdev != userdata_dev) return -1;
    int fd = open(USERDATA, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) return -1;
    int valid = ext4_identity(fd);
    close(fd);
    if (!valid) { logmsg("userdata ext4 UUID mismatch; holding in rescue"); return -1; }
    return 0;
}

static void persistent_gadget(const char *base)
{
    char function[512], path[512];
    snprintf(function, sizeof(function), "%s/functions/ecm.usb0", base);
    if (mkdir_ok(function)) goto failed;
    snprintf(path, sizeof(path), "%s/functions/ecm.usb0/dev_addr", base);
    if (write_text(path, "02:50:41:44:34:02")) goto failed;
    snprintf(path, sizeof(path), "%s/functions/ecm.usb0/host_addr", base);
    if (write_text(path, "02:50:41:44:34:01")) goto failed;
    snprintf(path, sizeof(path), "%s/configs/c.1/ecm.usb0", base);
    if (symlink(function, path) && errno != EEXIST) goto failed;
    snprintf(path, sizeof(path), "%s/bcdDevice", base);
    if (write_text(path, "0x0101")) goto failed;
    return;
failed:
    logmsg("ECM setup failed: %s; retaining serial gadget", strerror(errno));
    snprintf(path, sizeof(path), "%s/configs/c.1/ecm.usb0", base);
    unlink(path);
}

static void persistent_reap(pid_t child, int status)
{
    if (child == network_pid) {
        network_pid = -1;
        network_ready = WIFEXITED(status) && WEXITSTATUS(status) == 0;
        network_retry = current_tick + 15;
    }
    if (child == arch_pid) {
        arch_pid = -1;
        arch_retry = current_tick + 15;
        logmsg("Arch SSH ended; serial rescue remains available");
    }
}

static void child_log(const char *name)
{
    // These standard streams intentionally survive exec into the service.
    if (!freopen(name, "a", stdout) || !freopen(name, "a", stderr)) _exit(120);
    chmod(name, 0600);
}

static int is_arch_mount(void)
{
    struct stat st;
    return !stat(ARCH_ROOT, &st) && st.st_dev == userdata_dev;
}

static int bind_dir(const char *source, const char *suffix)
{
    char path[256];
    snprintf(path, sizeof(path), ARCH_ROOT "/%s", suffix);
    if (mkdir_ok(path)) return -1;
    struct stat from, to;
    if (!stat(source, &from) && !stat(path, &to) &&
        from.st_dev == to.st_dev && from.st_ino == to.st_ino) return 0;
    return mount(source, path, NULL, MS_BIND, NULL);
}

static int ram_dir(const char *path, const char *options)
{
    struct statfs fs;
    if (!statfs(path, &fs) && fs.f_type == TMPFS_MAGIC) return 0;
    return mount("tmpfs", path, "tmpfs", MS_NOSUID | MS_NODEV, options);
}

static _Noreturn void start_arch(void)
{
    child_log("/tmp/arch-startup.log");
    if (prepare_userdata()) _exit(130);
    if (!is_arch_mount()) {
        if (mkdir_ok(ARCH_ROOT) || mount(USERDATA, ARCH_ROOT, "ext4", MS_NOATIME,
                                      "nodiscard,errors=remount-ro")) {
            perror("mount Arch"); _exit(131);
        }
    }
    if (!is_arch_mount() || !file_has_line(ARCH_ROOT "/etc/pad4-root-id", "59b8e7b3:archroot-v1\n")) _exit(132);
    // Bind mounts are made once per boot, independently from SSH restarts.
    if (access("/tmp/arch-mounts-ready", F_OK)) {
        if (bind_dir("/dev", "dev") || bind_dir("/dev/pts", "dev/pts") ||
            bind_dir("/proc", "proc") || bind_dir("/sys", "sys")) _exit(133);
        if (ram_dir(ARCH_ROOT "/run", "mode=0755") ||
            ram_dir(ARCH_ROOT "/tmp", "mode=1777")) _exit(134);
        int fd = open("/tmp/arch-mounts-ready", O_WRONLY | O_CREAT, 0600);
        if (fd < 0) _exit(135);
        close(fd);
    }
    if (chdir(ARCH_ROOT) || chroot(".") || chdir("/")) _exit(136);
    mkdir_ok("/run/sshd");
    clearenv();
    setenv("PATH", "/usr/bin:/usr/sbin", 1);
    setenv("HOME", "/root", 1);
    execl("/usr/bin/sshd", "/usr/bin/sshd", "-D", "-e", "-f", "/etc/ssh/sshd_config_pad4",
          "-p", ARCH_SSH_PORT, (char *)NULL);
    perror("Arch sshd");
    _exit(137);
}

static void persistent_tick(unsigned tick, int bound, int modules_done)
{
    current_tick = tick;
#ifdef RESCUE_MAINLINE
    // USB/storage are built in. The optional watchdog probe must not gate SSH.
    modules_done = 1;
#endif
    if (bound && !network_ready && network_pid < 0 && tick >= network_retry) {
        network_pid = supervised_fork();
        if (!network_pid) {
            child_log("/tmp/network-startup.log");
            execl("/rescue/busybox", "sh", "/rescue/network.sh", (char *)NULL);
            _exit(122);
        }
    }
    if (network_ready && modules_done && arch_pid < 0 && tick >= arch_retry) {
        arch_pid = supervised_fork();
        if (!arch_pid) start_arch();
    }
}

#ifndef RESCUE_PERSISTENT_TEST
int main(int argc, char **argv)
{
    if (argc == 2 && !strcmp(argv[1], "--check-storage")) return prepare_userdata() ? 1 : 0;
#ifdef RESCUE_PERSISTENT_LIVE_TEST
    if (argc == 2 && !strcmp(argv[1], "--test-start-arch")) start_arch();
#endif
    if (getpid() != 1) { fputs("Persistent rescue requires PID 1.\n", stderr); return 2; }
    run_rescue();
}
#endif
