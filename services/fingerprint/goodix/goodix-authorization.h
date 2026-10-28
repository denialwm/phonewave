/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Root-only, one-enrollment Denial broker. No PIN enters this process. */
#ifndef GOODIX_AUTHORIZATION_H
#define GOODIX_AUTHORIZATION_H
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
#define GOODIX_AUTH_SOCKET "/run/nx809j-fingerprint/enroll.sock"

static int authorization_available(void)
{
    struct stat parent, socket_node;
    return !lstat("/run/nx809j-fingerprint", &parent) && S_ISDIR(parent.st_mode) &&
        !parent.st_uid && (parent.st_mode & 0777) == 0700 &&
        !lstat(GOODIX_AUTH_SOCKET, &socket_node) && S_ISSOCK(socket_node.st_mode) &&
        !socket_node.st_uid && (socket_node.st_mode & 0777) == 0600;
}
static int authorization_transfer(int fd, void *data, size_t size, int sending)
{
    struct timespec start, now;
    if (clock_gettime(CLOCK_MONOTONIC, &start)) return -EIO;
    uint8_t *p = data;
    while (size) {
        if (worker_stopping) return -ECANCELED;
        if (clock_gettime(CLOCK_MONOTONIC, &now)) return -EIO;
        if (now.tv_sec - start.tv_sec >= 28) return -ETIMEDOUT;
        struct pollfd f = {.fd = fd, .events = sending ? POLLOUT : POLLIN};
        int ready = poll(&f, 1, 100);
        if (ready < 0 && errno == EINTR) continue;
        if (ready < 0 || f.revents & (POLLERR | POLLNVAL)) return -EIO;
        if (!ready) continue;
        ssize_t n = sending ? send(fd, p, size, MSG_NOSIGNAL) : read(fd, p, size);
        if (n < 0 && (errno == EINTR || errno == EAGAIN)) continue;
        if (n <= 0) return -EPIPE;
        p += n; size -= n;
    }
    return 0;
}
static int authorization_exchange(int fd, uint64_t challenge, uint8_t token[69])
{
    memset(token, 0, 69);
    uint8_t request[24] = {'N','X','E','A','1'}, reply[16] = {0};
    for (unsigned i = 0; i < 8; i++) request[8 + i] = challenge >> (8 * i);
    uint32_t uid = GOODIX_NATIVE_GROUP;
    for (unsigned i = 0; i < 4; i++) request[16 + i] = uid >> (8 * i);
    int ret = authorization_transfer(fd, request, sizeof(request), 1);
    if (!ret) ret = authorization_transfer(fd, reply, sizeof(reply), 0);
    if (!ret && memcmp(reply, "NXER1\0\0\0", 8)) ret = -EPROTO;
    uint32_t status = (uint32_t)reply[8] | (uint32_t)reply[9] << 8 | (uint32_t)reply[10] << 16 | (uint32_t)reply[11] << 24;
    if (!ret && status) ret = (int32_t)status < 0 ? (int32_t)status : -EPROTO;
    if (!ret && (reply[12] || reply[13] || reply[14] || reply[15])) ret = -EPROTO;
    if (!ret) ret = authorization_transfer(fd, token, 69, 0);
    if (ret) explicit_bzero(token, 69);
    return ret;
}
static int worker_authorize(void *opaque, uint64_t challenge, uint8_t token[69])
{
    (void)opaque;
    memset(token, 0, 69);
    if (!challenge || !authorization_available()) return -ENOKEY;
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) return -errno;
    struct sockaddr_un address = {.sun_family = AF_UNIX};
    memcpy(address.sun_path, GOODIX_AUTH_SOCKET, sizeof(GOODIX_AUTH_SOCKET));
    int ret = -EACCES;
    if (connect(fd, (struct sockaddr *)&address, sizeof(address))) goto done;
    struct ucred peer; socklen_t n = sizeof(peer);
    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &n) || n != sizeof(peer) || peer.uid) goto done;
    ret = authorization_exchange(fd, challenge, token);
 done:
    close(fd);
    if (ret) explicit_bzero(token, 69);
    return ret;
}
#endif
