/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* One private request per root child; PINs and tokens never use argv or logs. */
#define _GNU_SOURCE
#include "gatekeeper.h"
#include "qcomtee_object.h"
#include "qcomtee_object_types.h"
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define HEADER_SIZE 48u
#define REPLY_SIZE 40u
#define BODY_MAX (GATEKEEPER_HANDLE_MAX + 2 * GATEKEEPER_PASSWORD_MAX)

static uint32_t get32(const uint8_t *p)
{ return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }
static uint64_t get64(const uint8_t *p)
{ return get32(p) | (uint64_t)get32(p + 4) << 32; }
static void put32(uint8_t *p, uint32_t v)
{ for (unsigned i = 0; i < 4; i++) p[i] = v >> (8 * i); }
static void put64(uint8_t *p, uint64_t v)
{ for (unsigned i = 0; i < 8; i++) p[i] = v >> (8 * i); }

static int64_t now_ms(void)
{
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) return -1;
    return (int64_t)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

static int read_private(void *data, size_t size, int64_t deadline)
{
    uint8_t *p = data;
    while (size) {
        int64_t now = now_ms();
        if (now < 0) return -EIO;
        int64_t remaining = deadline - now;
        if (remaining <= 0) return -ETIMEDOUT;
        struct pollfd fd = {.fd = STDIN_FILENO, .events = POLLIN};
        int n = poll(&fd, 1, remaining > 1000 ? 1000 : (int)remaining);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 || fd.revents & (POLLERR | POLLNVAL)) return -EIO;
        if (!n) continue;
        ssize_t count = read(STDIN_FILENO, p, size);
        if (count < 0 && (errno == EINTR || errno == EAGAIN)) continue;
        if (count <= 0) return -ECANCELED;
        p += count; size -= count;
    }
    return 0;
}

static int write_private(int fd, const void *data, size_t size)
{
    const uint8_t *p = data;
    int64_t start = now_ms();
    if (start < 0) return -EIO;
    while (size) {
        int64_t now = now_ms();
        if (now < 0) return -EIO;
        int64_t remaining = start + 30000 - now;
        if (remaining <= 0) return -ETIMEDOUT;
        struct pollfd f = {.fd = fd, .events = POLLOUT};
        int n = poll(&f, 1, remaining > 1000 ? 1000 : (int)remaining);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 || f.revents & (POLLERR | POLLHUP | POLLNVAL)) return -EPIPE;
        if (!n) continue;
        ssize_t count = write(fd, p, size);
        if (count < 0 && (errno == EINTR || errno == EAGAIN)) continue;
        if (count <= 0) return -EPIPE;
        p += count; size -= count;
    }
    return 0;
}

static struct qcomtee_param input(void *p, size_t n)
{ return (struct qcomtee_param){.attr = QCOMTEE_UBUF_INPUT, .ubuf = {p, n}}; }
static struct qcomtee_param output(void *p, size_t n)
{ return (struct qcomtee_param){.attr = QCOMTEE_UBUF_OUTPUT, .ubuf = {p, n}}; }
static struct qcomtee_param object(void)
{ return (struct qcomtee_param){.attr = QCOMTEE_OBJREF_INPUT, .object = NULL}; }
static struct qcomtee_param new_object(void)
{ return (struct qcomtee_param){.attr = QCOMTEE_OBJREF_OUTPUT}; }

static int invoke(struct qcomtee_object *object_ref, uint32_t op, struct qcomtee_param *p, unsigned count)
{
    qcomtee_result_t service = UINT32_MAX;
    int ret = qcomtee_object_invoke(object_ref, op, p, count, &service);
    return ret ? -EIO : service ? -EACCES : 0;
}

static int exchange(void *context, const void *request, size_t size, void *reply, size_t *reply_size)
{
    if (size > GATEKEEPER_REQUEST_MAX || *reply_size > KEYMASTER_REPLY_CAPACITY) return -EMSGSIZE;
    uint8_t *request_out = calloc(1, size);
    if (!request_out) return -ENOMEM;
    uint32_t offsets[4] = {0}, is64 = 1;
    struct qcomtee_param p[] = {
        input((void *)request, size), input(reply, *reply_size), input(offsets, sizeof(offsets)), input(&is64, 4),
        output(request_out, size), output(reply, *reply_size), object(), object(), object(), object()
    };
    int ret = invoke(context, 0, p, 10);
    *reply_size = p[5].ubuf.size;
    gatekeeper_clear(request_out, size); free(request_out);
    return ret;
}

struct transport { struct qcomtee_object *root, *env, *loader, *app; };

static int connect_keymaster(struct transport *t)
{
    t->root = qcomtee_object_root_init("/dev/tee0", ioctl, NULL, NULL);
    if (!t->root) return -ENODEV;
    uint32_t uid = geteuid();
    uint8_t identity[] = {0xa2, 1, 0x1a, uid >> 24, uid >> 16, uid >> 8, uid,
        3, 0x71, 'g','a','t','e','k','e','e','p','e','r','-','w','o','r','k','e','r'};
    struct qcomtee_param reg[] = {input(identity, sizeof(identity)), new_object()};
    int ret = invoke(t->root, 1, reg, 2);
    if (ret) return ret;
    t->env = reg[1].object;
    uint32_t loader_id = 122;
    struct qcomtee_param open[] = {input(&loader_id, 4), new_object()};
    if ((ret = invoke(t->env, 0, open, 2))) return ret;
    t->loader = open[1].object;
    char name[] = "keymaster64"; uint32_t arch = 0;
    struct qcomtee_param find[] = {input(name, sizeof(name) - 1), output(&arch, 4), new_object()};
    if ((ret = invoke(t->loader, 2, find, 3))) return ret;
    t->app = find[2].object;
    if (arch != 2) { fprintf(stderr,"Gatekeeper keymaster architecture=%u\n",arch); return -EPROTO; }
    struct keymaster_agreement_result agreement;
    ret = keymaster_shared_secret_agree_single_tee(exchange, t->app, &agreement);
    fprintf(stderr,"Gatekeeper readiness stage=%s result=%d exchange=%d keymint=%d\n",
        agreement.stage,ret,agreement.exchange_status,agreement.keymint_status);
    return ret;
}

static void disconnect_keymaster(struct transport *t)
{
    /* Every object is borrowed. In particular, never unload Keymaster. */
    if (t->app) qcomtee_object_refs_dec(t->app);
    if (t->loader) qcomtee_object_refs_dec(t->loader);
    if (t->env) qcomtee_object_refs_dec(t->env);
    if (t->root) qcomtee_object_refs_dec(t->root);
}

static int board(void)
{
    char b[256]; const char expected[] = "nubia,nx809j";
    int fd = open("/sys/firmware/devicetree/base/compatible", O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) return -ENODEV;
    ssize_t n = read(fd, b, sizeof(b)); close(fd);
    return n >= (ssize_t)sizeof(expected) && !memcmp(b, expected, sizeof(expected)) ? 0 : -ENODEV;
}

static int validate_request(const uint8_t header[HEADER_SIZE])
{
    uint32_t op = get32(header + 8), h = get32(header + 32), p = get32(header + 36), n = get32(header + 40);
    uint64_t challenge = get64(header + 16), sid = get64(header + 24);
    if (memcmp(header, "NXGK1\0\0\0", 8) || get32(header + 12) || get32(header + 44) ||
        op < 1 || op > 3 || h > GATEKEEPER_HANDLE_MAX || p > GATEKEEPER_PASSWORD_MAX || n > GATEKEEPER_PASSWORD_MAX ||
        (op == 1 && (h || p || !n || challenge || sid)) ||
        (op == 2 && (!h || !p || !n || challenge || !sid)) ||
        (op == 3 && (!h || !p || n || !sid))) return -EINVAL;
    return 0;
}

int main(int argc, char **argv)
{
    (void)argv;
    struct stat in_stat, out_stat;
    if (argc != 1 || geteuid() || prctl(PR_SET_DUMPABLE, 0) ||
        fstat(STDIN_FILENO, &in_stat) || fstat(STDOUT_FILENO, &out_stat) ||
        !(S_ISFIFO(in_stat.st_mode) || S_ISSOCK(in_stat.st_mode)) ||
        !(S_ISFIFO(out_stat.st_mode) || S_ISSOCK(out_stat.st_mode))) return 2;
    signal(SIGPIPE, SIG_IGN);
    int records = fcntl(STDOUT_FILENO, F_DUPFD_CLOEXEC, 3);
    if (records < 0) return 2;
    int flags = fcntl(records, F_GETFL);
    if (flags < 0 || fcntl(records, F_SETFL, flags | O_NONBLOCK) < 0 ||
        dup2(STDERR_FILENO, STDOUT_FILENO) < 0) { close(records); return 2; }
    uint8_t header[HEADER_SIZE] = {0}, reply[REPLY_SIZE] = {0};
    struct {
        uint8_t body[BODY_MAX], token[GATEKEEPER_HAT_SIZE];
        struct gatekeeper_credential current, next;
    } *secrets = calloc(1, sizeof(*secrets));
    if (!secrets) { close(records); return 1; }
    struct transport t = {0}; struct gatekeeper_result result = {0};
    size_t payload_size = 0; const void *payload = NULL; uint64_t sid = 0;
    int ret = mlock(secrets, sizeof(*secrets)) ? -errno : 0;
    int64_t now = now_ms(), deadline = now + 30000;
    if (!ret && now < 0) ret = -EIO;
    if (!ret) ret = read_private(header, sizeof(header), deadline);
    uint32_t op = get32(header + 8), h = get32(header + 32), p = get32(header + 36), n = get32(header + 40);
    uint64_t challenge = get64(header + 16);
    if (!ret) ret = validate_request(header);
    if (!ret) ret = read_private(secrets->body, (size_t)h + p + n, deadline);
    if (!ret) ret = board();
    if (!ret) ret = connect_keymaster(&t);
    if (!ret) {
        secrets->current.uid = GATEKEEPER_NATIVE_UID;
        secrets->current.secure_user_id = get64(header + 24);
        secrets->current.handle_size = h;
        memcpy(secrets->current.handle, secrets->body, h);
        if (op == 3) {
            ret = gatekeeper_verify(exchange, t.app, &secrets->current, challenge,
                secrets->body + h, p, secrets->token, &result);
            if (!ret) { payload = secrets->token; payload_size = GATEKEEPER_HAT_SIZE; sid = secrets->current.secure_user_id; }
        } else {
            ret = gatekeeper_enroll(exchange, t.app, GATEKEEPER_NATIVE_UID,
                op == 1 ? NULL : &secrets->current, secrets->body + h, p,
                secrets->body + h + p, n, &secrets->next, &result);
            /* An initial enrollment is usable only if the returned credential
             * verifies with the PIN that produced it. Keep both in locked memory. */
            if (!ret && op == 1) {
                ret = gatekeeper_verify(exchange, t.app, &secrets->next, 0,
                    secrets->body + h + p, n, secrets->token, &result);
                fprintf(stderr, "Gatekeeper initial enrollment verification result=%d service=%d stage=%s\n",
                    ret, result.service_status, result.stage ? result.stage : "unknown");
                gatekeeper_clear(secrets->token, sizeof(secrets->token));
            }
            if (!ret) { payload = secrets->next.handle; payload_size = secrets->next.handle_size; sid = secrets->next.secure_user_id; }
        }
    }
    fprintf(stderr,"Gatekeeper operation=%u result=%d exchange=%d service=%d stage=%s reply=%zu offset=%zu major=%u\n",
        op,ret,result.exchange_status,result.service_status,result.stage?result.stage:"startup",
        result.reply_size,result.parse_offset,result.parse_major);
    disconnect_keymaster(&t);
    memcpy(reply, "NXGR1\0\0\0", 8);
    put32(reply + 8, (uint32_t)ret); put32(reply + 12, (uint32_t)result.service_status);
    put32(reply + 16, result.retry_ms); put32(reply + 20, GATEKEEPER_NATIVE_UID);
    put64(reply + 24, sid); put32(reply + 32, payload_size);
    int sent = write_private(records, reply, sizeof(reply));
    if (!sent && payload_size) sent = write_private(records, payload, payload_size);
    gatekeeper_clear(secrets, sizeof(*secrets)); munlock(secrets, sizeof(*secrets)); free(secrets);
    gatekeeper_clear(header, sizeof(header)); gatekeeper_clear(reply, sizeof(reply));
    close(records);
    return ret || sent ? 1 : 0;
}
