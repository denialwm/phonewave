// SPDX-License-Identifier: GPL-2.0-only
// NX809J contact child for goodix-fprint-helper: a finger held on the
// ultrasonic sensor's circle. The sensor needs no light, so unlike the
// Aston's presentation client this one never talks to the compositor; it
// only watches the touchscreen and answers in the same DFP1 records.
// No input injection or event grabbing; no biometric data passes here.
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/input.h>
#include <poll.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include "../uff/uff-contact.h"

static volatile sig_atomic_t interrupted;
static struct uff_contact contact;
static bool active, ready, suspended;

static void signal_stop(int signal_number) { (void)signal_number; interrupted = 1; }

static bool identity(void)
{
    char text[256] = {0};
    int fd = open("/proc/device-tree/compatible", O_RDONLY | O_CLOEXEC);
    if (fd < 0) return false;
    ssize_t n = read(fd, text, sizeof(text) - 1);
    close(fd);
    if (n <= 0 || strcmp(text, "nubia,nx809j")) return false;
    static char boot[16384];
    FILE *bootconfig = fopen("/proc/bootconfig", "r");
    if (!bootconfig) return false;
    size_t size = fread(boot, 1, sizeof(boot) - 1, bootconfig);
    boot[size] = 0;
    fclose(bootconfig);
    return strstr(boot, "androidboot.slot_suffix = \"_b\"") != NULL;
}

/* The TCM touchscreen, its current slots read in: a finger already held when
 * the child starts must lift before it counts. */
static int open_touch(void)
{
    DIR *directory = opendir("/dev/input");
    if (!directory) return -1;
    int found = -1;
    struct dirent *entry;
    while (found < 0 && (entry = readdir(directory))) {
        if (strncmp(entry->d_name, "event", 5)) continue;
        int fd = openat(dirfd(directory), entry->d_name, O_RDONLY | O_NONBLOCK | O_CLOEXEC | O_NOFOLLOW);
        if (fd < 0) continue;
        char name[128] = {0};
        struct input_id id;
        struct input_absinfo slot, ax, ay;
        if (ioctl(fd, EVIOCGNAME(sizeof(name) - 1), name) < 0 || strcmp(name, "synaptics_tcm_touch") ||
            ioctl(fd, EVIOCGID, &id) || id.bustype != BUS_SPI ||
            ioctl(fd, EVIOCGABS(ABS_MT_SLOT), &slot) || ioctl(fd, EVIOCGABS(ABS_MT_POSITION_X), &ax) ||
            ioctl(fd, EVIOCGABS(ABS_MT_POSITION_Y), &ay) ||
            slot.minimum != 0 || slot.maximum != UFF_CONTACT_SLOTS - 1 || slot.value < 0 ||
            slot.value >= UFF_CONTACT_SLOTS || ax.minimum != 0 || ax.maximum != UFF_CONTACT_NX809J.max_x ||
            ay.minimum != 0 || ay.maximum != UFF_CONTACT_NX809J.max_y) {
            close(fd);
            continue;
        }
        uff_contact_init(&contact, &UFF_CONTACT_NX809J);
        contact.slot = slot.value;
        int values[UFF_CONTACT_SLOTS + 1];
        const int axes[] = {ABS_MT_TRACKING_ID, ABS_MT_POSITION_X, ABS_MT_POSITION_Y};
        bool read_all = true;
        for (unsigned a = 0; read_all && a < 3; a++) {
            values[0] = axes[a];
            if (ioctl(fd, EVIOCGMTSLOTS(sizeof(values)), values)) { read_all = false; break; }
            for (unsigned i = 0; i < UFF_CONTACT_SLOTS; i++) {
                if (a == 0) {
                    contact.slots[i].id = values[i + 1];
                    if (values[i + 1] >= 0) contact.armed = false;
                } else if (a == 1) contact.slots[i].x = values[i + 1];
                else contact.slots[i].y = values[i + 1];
            }
        }
        if (read_all) found = fd;
        else close(fd);
    }
    closedir(directory);
    return found;
}

/* 0 to go on, 1 when the finger lifted early, -1 when the stream broke. */
static int read_touch(int fd)
{
    struct input_event events[64];
    ssize_t bytes = read(fd, events, sizeof(events));
    if (bytes < 0 && errno == EAGAIN) return 0;
    if (bytes <= 0 || bytes % sizeof(events[0])) return -1;
    for (size_t i = 0; i < (size_t)bytes / sizeof(events[0]); i++) {
        int transition = uff_contact_event(&contact, &events[i]);
        if (transition < 0) return -1;
        if (transition == 1 && !active) {
            active = true;
            puts("DFP1 down 1");
        } else if (transition == 2 && active) {
            puts("DFP1 lost 1");
            return 1;
        }
    }
    return 0;
}

/* 1 when finished cleanly, 0 to go on, -1 for a command out of turn. */
static int control_command(const char *command)
{
    if (!strcmp(command, "suspend")) {
        if (active || suspended) puts("DFP1 suspend-busy");
        else { suspended = true; puts("DFP1 suspended"); }
    } else if (!strcmp(command, "resume") && suspended) {
        suspended = false;
        puts("DFP1 resumed");
    } else if (!strcmp(command, "present") && active && !ready && !suspended) {
        // Nothing to light: the sensor is ready as soon as the finger is.
        ready = true;
        puts("DFP1 ready 1");
    } else if (!strcmp(command, "finish") && ready && active) {
        puts("DFP1 stopped 1 0");
        return 1;
    } else return -1;
    return 0;
}

int main(void)
{
    setvbuf(stdout, NULL, _IOLBF, 0);
    signal(SIGINT, signal_stop); signal(SIGTERM, signal_stop); signal(SIGHUP, signal_stop);
    signal(SIGPIPE, SIG_IGN);
    if (geteuid() != 0 || !identity()) return 2;
    int touch = open_touch();
    if (touch < 0) { fputs("DFP1 unavailable\n", stderr); return 2; }
    puts("DFP1 waiting");
    char buffer[32];
    size_t length = 0;
    int status = 2;
    while (!interrupted) {
        struct pollfd fds[2] = {
            {.fd = suspended ? -1 : touch, .events = POLLIN},
            {.fd = STDIN_FILENO, .events = POLLIN},
        };
        int count = poll(fds, 2, 100);
        if (count < 0 && errno != EINTR) break;
        if (count <= 0) continue;
        if (fds[0].revents & (POLLERR | POLLHUP | POLLNVAL)) break;
        // A lift read before a simultaneous finish cancels rather than
        // completes, so touch goes first.
        if ((fds[0].revents & POLLIN) && read_touch(touch)) break;
        if (fds[1].revents & (POLLIN | POLLHUP)) {
            ssize_t n = read(STDIN_FILENO, buffer + length, sizeof(buffer) - 1 - length);
            if (n <= 0) break;
            length += (size_t)n;
            buffer[length] = 0;
            char *newline;
            int answer = 0;
            while (!answer && (newline = memchr(buffer, '\n', length))) {
                *newline = 0;
                answer = control_command(buffer);
                size_t used = (size_t)(newline - buffer) + 1;
                memmove(buffer, buffer + used, length - used);
                length -= used;
            }
            if (answer == 1) { status = 0; break; }
            if (answer < 0 || length == sizeof(buffer) - 1) break;
        }
    }
    close(touch);
    return status;
}
