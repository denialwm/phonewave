/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Exercise the real private framing helpers without TEE access or root. */
#define main gatekeeper_child_main
#include "gatekeeper-worker.c"
#undef main
#include <assert.h>

int main(void)
{
    uint8_t h[HEADER_SIZE] = {0};
    memcpy(h, "NXGK1\0\0\0", 8); put32(h + 8, 1); put32(h + 40, 4);
    assert(!validate_request(h));
    put32(h + 12, 1); assert(validate_request(h)); put32(h + 12, 0);
    put32(h + 44, 1); assert(validate_request(h)); put32(h + 44, 0);
    h[0] = 'X'; assert(validate_request(h)); h[0] = 'N';
    put32(h + 8, 0); assert(validate_request(h)); put32(h + 8, 4); assert(validate_request(h)); put32(h + 8, 1);
    put64(h + 16, 1); assert(validate_request(h)); put64(h + 16, 0);
    put64(h + 24, 1); assert(validate_request(h)); put64(h + 24, 0);
    put32(h + 32, 1); assert(validate_request(h)); put32(h + 32, 0);
    put32(h + 36, 1); assert(validate_request(h)); put32(h + 36, 0);
    put32(h + 40, 0); assert(validate_request(h));
    put32(h + 40, GATEKEEPER_PASSWORD_MAX); assert(!validate_request(h));
    put32(h + 40, GATEKEEPER_PASSWORD_MAX + 1); assert(validate_request(h));

    put32(h + 8, 2); put64(h + 24, 123); put32(h + 32, 3); put32(h + 36, 4); put32(h + 40, 4);
    assert(!validate_request(h));
    put32(h + 32, GATEKEEPER_HANDLE_MAX + 1); assert(validate_request(h)); put32(h + 32, 3);
    put32(h + 36, 0); assert(validate_request(h)); put32(h + 36, 4);
    put64(h + 24, 0); assert(validate_request(h)); put64(h + 24, 123);
    put64(h + 16, UINT64_MAX); assert(validate_request(h));
    put32(h + 8, 3); assert(validate_request(h)); put32(h + 40, 0); assert(!validate_request(h));
    assert(get64(h + 16) == UINT64_MAX); put64(h + 16, 0); assert(!validate_request(h));
    put32(h + 32, GATEKEEPER_HANDLE_MAX); put32(h + 36, GATEKEEPER_PASSWORD_MAX); assert(!validate_request(h));
    put32(h + 36, UINT32_MAX); assert(validate_request(h));

    int fds[2]; assert(!pipe(fds));
    int original_stdin = dup(STDIN_FILENO); assert(original_stdin >= 0);
    assert(dup2(fds[0], STDIN_FILENO) == STDIN_FILENO); close(fds[0]);
    uint8_t bytes[2] = {0xaa,0xbb};
    assert(read_private(bytes, sizeof(bytes), now_ms() - 1) == -ETIMEDOUT);
    assert(write(fds[1], "\x12", 1) == 1); close(fds[1]);
    assert(read_private(bytes, sizeof(bytes), now_ms() + 1000) == -ECANCELED);
    assert(bytes[0] == 0x12 && bytes[1] == 0xbb);
    assert(dup2(original_stdin, STDIN_FILENO) == STDIN_FILENO); close(original_stdin);

    assert(!pipe(fds));
    assert(!write_private(fds[1], h, sizeof(h))); uint8_t copy[HEADER_SIZE];
    assert(read(fds[0], copy, sizeof(copy)) == sizeof(copy)); assert(!memcmp(h, copy, sizeof(h)));
    signal(SIGPIPE, SIG_IGN); close(fds[0]); assert(write_private(fds[1], h, sizeof(h)) == -EPIPE); close(fds[1]);
    char state[] = "/tmp/goodix-gatekeeper-state-XXXXXX";
    assert(mkdtemp(state));
    int directory = open(state, O_RDONLY | O_DIRECTORY);
    assert(directory >= 0);
    int lock = state_lock(directory); assert(lock >= 0);
    static struct enrollment kept;
    const uint8_t first[] = {1, 2, 3}, second[] = {4, 5, 6, 7};
    assert(state_load(directory, &kept) == 0);
    assert(!state_admits(1, 0, &kept, 0, NULL, 0));
    assert(!state_admits(3, 0, &kept, 9, first, sizeof(first)));
    assert(!state_save(directory, 9, first, sizeof(first)));
    assert(state_load(directory, &kept) == 1);
    assert(kept.sid == 9 && kept.handle_size == sizeof(first) && !memcmp(kept.handle, first, sizeof(first)));
    assert(state_admits(1, 1, &kept, 0, NULL, 0) == -EEXIST);
    assert(!state_admits(3, 1, &kept, 9, first, sizeof(first)));
    assert(!state_admits(2, 1, &kept, 9, first, sizeof(first)));
    assert(state_admits(3, 1, &kept, 8, first, sizeof(first)) == -ESTALE);
    assert(state_admits(3, 1, &kept, 9, second, sizeof(second)) == -ESTALE);
    assert(state_admits(2, 1, &kept, 9, second, 3) == -ESTALE);
    assert(!state_save(directory, 9, second, sizeof(second)));
    assert(state_load(directory, &kept) == 1 && kept.handle_size == sizeof(second));
    assert(state_admits(3, 1, &kept, 9, first, sizeof(first)) == -ESTALE);
    assert(state_save(directory, 0, second, sizeof(second)) == -EINVAL);
    int fd = openat(directory, "enrollment", O_WRONLY | O_TRUNC);
    assert(fd >= 0 && write(fd, "NXGS1", 5) == 5); close(fd);
    assert(state_load(directory, &kept) == -EBADMSG);
    assert(state_admits(3, -EBADMSG, &kept, 9, second, sizeof(second)) == -EBADMSG);
    close(lock);
    assert(!unlinkat(directory, "enrollment", 0) && !unlinkat(directory, "lock", 0));
    close(directory); assert(!rmdir(state));

    puts("Gatekeeper worker: private framing, bounds, cancellation, broken-pipe cleanup and the kept enrollment passed");
    return 0;
}
