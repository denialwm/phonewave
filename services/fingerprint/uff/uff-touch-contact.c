// SPDX-License-Identifier: GPL-2.0-only
// Contact child for the fingerprint helper on a phone whose sensor needs no
// light, such as an ultrasonic one: a finger held on the sensor's circle.
// Unlike the presentation client this one never talks to the compositor; it
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
#include "uff-contact.h"

static volatile sig_atomic_t interrupted;
static struct uff_contact contact;
static bool active, ready, suspended;

static void signal_stop(int signal_number) { (void)signal_number; interrupted = 1; }

static struct uff_contact_area area;

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
    if (geteuid() != 0) return 2;
    int touch = uff_contact_open(&contact, &area);
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
