/* SPDX-License-Identifier: GPL-2.0-only */
/* Unit fixture uses pipes and stub protocol calls, never a Wayland server. */
#define main presentation_worker_main
#include "denial-presentation.c"
#undef main
#include <assert.h>
static unsigned presented, withdrawn;

uint32_t __wrap_wl_proxy_get_version(struct wl_proxy *proxy) {
    (void)proxy; return 1;
}
struct wl_proxy *__wrap_wl_proxy_marshal_flags(struct wl_proxy *proxy, uint32_t opcode,
        const struct wl_interface *interface, uint32_t version, uint32_t flags, ...) {
    (void)proxy; (void)interface; (void)version; (void)flags;
    if (opcode == DENIAL_FINGERPRINT_SESSION_V1_PRESENT) presented++;
    if (opcode == DENIAL_FINGERPRINT_SESSION_V1_WITHDRAW) withdrawn++;
    return NULL;
}
static void event(int fd, unsigned code) {
    struct input_event value = {.type=EV_KEY, .code=code, .value=1};
    assert(write(fd, &value, sizeof(value)) == sizeof(value));
}
int main(void) {
    assert(setenv("JIIOV_SUSPEND_PROTOCOL", "1", 1) == 0);
    int input[2];
    assert(pipe(input) == 0);
    inputs[1] = input[0];
    session = (struct denial_fingerprint_session_v1 *)(uintptr_t)1;
    bool finish_requested = false;
    assert(control_command("suspend", &finish_requested) == 0 && suspended);
    event(input[1], 707);
    struct pollfd pending = {.fd=input[0], .events=POLLIN};
    assert(poll(&pending, 1, 0) == 1);  /* Buffered physical contact. */
    assert(presented == 0 && !active);
    assert(control_command("resume", &finish_requested) == 0 && !suspended);
    assert(read_input(1) == 0 && presented == 1 && active && !lost);
    assert(control_command("suspend", &finish_requested) == 0 && !suspended); /* busy */
    event(input[1], 708);
    assert(read_input(1) == 0 && lost && withdrawn == 1);

    active = lost = false;
    assert(control_command("suspend", &finish_requested) == 0 && suspended);
    event(input[1], 707);
    event(input[1], 708);
    assert(control_command("resume", &finish_requested) == 0 && !suspended);
    assert(read_input(1) == 0 && presented == 2 && withdrawn == 2 && lost && !ready);
    assert(control_command("resume", &finish_requested) == -1);
    assert(control_command("garbage", &finish_requested) == -1);
    close(input[0]); close(input[1]);
    puts("PASS: buffered contact, resume, active-contact refusal and early lift");
    return 0;
}
