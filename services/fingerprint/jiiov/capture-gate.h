/* SPDX-License-Identifier: GPL-2.0-only */
/* One inherited pipe authorizes one capture. EOF, timeout and cancellation
 * deny capture. The distinct gated command also rejects older probe binaries. */
#include <poll.h>

static uint64_t monotonic_us(void)
{
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC, &value)) return 0;
    return (uint64_t)value.tv_sec * 1000000 + value.tv_nsec / 1000;
}

static int capture_gate_wait(int fd, unsigned timeout_ms)
{
    uint64_t start = monotonic_us();
    if (!start || fd < 0 || !timeout_ms) return -1;
    uint64_t deadline = start + (uint64_t)timeout_ms * 1000;
    while (!interrupted) {
        uint64_t now = monotonic_us();
        if (!now || now >= deadline) return -1;
        int remaining = (int)((deadline - now + 999) / 1000);
        struct pollfd descriptor = {.fd=fd, .events=POLLIN};
        int result = poll(&descriptor, 1, remaining > 100 ? 100 : remaining);
        if (result < 0 && errno == EINTR) continue;
        if (result < 0 || descriptor.revents & (POLLERR | POLLNVAL)) return -1;
        if (interrupted) return -1;
        if (descriptor.revents & (POLLIN | POLLHUP)) {
            unsigned char permission = 0;
            return read(fd, &permission, 1) == 1 && permission == 1 && !interrupted ? 0 : -1;
        }
    }
    return -1;
}
