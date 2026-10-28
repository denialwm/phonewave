/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Native OneKeyMint Gatekeeper wire codec. No Android runtime dependencies. */
#include "gatekeeper.h"
#include <errno.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>

void gatekeeper_clear(void *memory, size_t size)
{
    volatile uint8_t *p = memory;
    while (size--) *p++ = 0;
}

struct encoder { uint8_t *data; size_t size; int error; };
struct decoder { const uint8_t *next, *end; };
struct response {
    int32_t status, timeout;
    uint64_t sid;
    size_t size;
    uint8_t data[GATEKEEPER_HANDLE_MAX];
};

static void put(struct encoder *e, const void *data, size_t size)
{
    if (e->error) return;
    if (size > GATEKEEPER_REQUEST_MAX - e->size) { e->error = -EMSGSIZE; return; }
    memcpy(e->data + e->size, data, size);
    e->size += size;
}

static void title(struct encoder *e, unsigned major, uint64_t value)
{
    uint8_t b[9]; size_t n = 1;
    if (value < 24) b[0] = (major << 5) | value;
    else {
        unsigned bytes = value <= UINT8_MAX ? 1 : value <= UINT16_MAX ? 2 : value <= UINT32_MAX ? 4 : 8;
        b[0] = (major << 5) | (bytes == 1 ? 24 : bytes == 2 ? 25 : bytes == 4 ? 26 : 27);
        for (unsigned i = 0; i < bytes; i++) b[n++] = value >> ((bytes - i - 1) * 8);
    }
    put(e, b, n);
}

static void text(struct encoder *e, const char *value)
{
    size_t n = strlen(value);
    title(e, 3, n); put(e, value, n);
}

/* Stock derives Serialize on Vec<u8>: these are arrays of unsigned bytes,
 * NOT CBOR byte strings. The difference matters for the signed service. */
static void vector(struct encoder *e, const uint8_t *data, size_t size)
{
    title(e, 4, size);
    for (size_t i = 0; i < size; i++) title(e, 0, data[i]);
}

static int head(struct decoder *d, unsigned *major, uint64_t *value)
{
    if (d->next == d->end) return -EPROTO;
    unsigned first = *d->next++, ai = first & 31;
    *major = first >> 5; *value = ai;
    if (ai < 24) return 0;
    unsigned n = ai == 24 ? 1 : ai == 25 ? 2 : ai == 26 ? 4 : ai == 27 ? 8 : 0;
    if (!n || (size_t)(d->end - d->next) < n) return -EPROTO;
    *value = 0;
    for (unsigned i = 0; i < n; i++) *value = (*value << 8) | *d->next++;
    uint64_t minimum = n == 1 ? 24 : n == 2 ? 256 : n == 4 ? 65536 : UINT64_C(4294967296);
    return *value < minimum ? -EPROTO : 0;
}

static int uint_value(struct decoder *d, uint64_t *value)
{
    unsigned major;
    return head(d, &major, value) || major ? -EPROTO : 0;
}

static int int32_value(struct decoder *d, int32_t *value)
{
    unsigned major; uint64_t v;
    if (head(d, &major, &v) || major > 1 || v > INT32_MAX) return -EPROTO;
    *value = major ? (int32_t)(-1 - (int64_t)v) : (int32_t)v;
    return 0;
}

static int key(struct decoder *d, const char *const *names, size_t count)
{
    unsigned major; uint64_t n;
    if (head(d, &major, &n) || major != 3 || n > (uint64_t)(d->end - d->next)) return -EPROTO;
    int found = -EPROTO;
    for (size_t i = 0; i < count; i++)
        if (strlen(names[i]) == n && !memcmp(d->next, names[i], n)) found = (int)i;
    d->next += n;
    return found;
}

static int read_vector(struct decoder *d, uint8_t *data, size_t *size, size_t capacity)
{
    unsigned major; uint64_t n;
    if (head(d, &major, &n) || major != 4 || n > capacity) return -EPROTO;
    for (size_t i = 0; i < n; i++) {
        uint64_t byte;
        if (uint_value(d, &byte) || byte > UINT8_MAX) return -EPROTO;
        data[i] = byte;
    }
    *size = n;
    return 0;
}

static void parse_stage(struct gatekeeper_result *result, const char *stage,
    const uint8_t *start, const struct decoder *d)
{
    result->stage = stage;
    result->parse_offset = (size_t)(d->next - start);
    result->parse_major = d->next < d->end ? *d->next >> 5 : 8;
}

static int decode(const uint8_t *data, size_t size, int enroll, struct response *r,
    struct gatekeeper_result *result)
{
    static const char *const enroll_keys[] = {"status", "timeout_ms", "secure_user_id", "password_handle"};
    static const char *const verify_keys[] = {"status", "timeout_ms", "hw_auth_token"};
    result->stage = "response frame";
    result->reply_size = size;
    if (size < 9 || size > KEYMASTER_REPLY_CAPACITY) return -EPROTO;
    uint64_t n = 0;
    for (unsigned i = 0; i < 8; i++) n |= (uint64_t)data[i] << (8 * i);
    if (!n || n > size - 8) {
        if (!n) result->stage = "empty response";
        else if (data[0] == 0xff && data[1] == 0xff && data[2] == 0xff && data[3] == 0xff)
            result->stage = "legacy command failure";
        else result->stage = "invalid response length";
        result->parse_major = data[0] >> 5;
        return -EPROTO;
    }
    struct decoder d = {data + 8, data + 8 + n};
    unsigned major, seen = 0, count = enroll ? 4 : 3; uint64_t fields;
    parse_stage(result, "response map", data, &d);
    if (head(&d, &major, &fields) || major != 5 || fields != count) return -EPROTO;
    for (unsigned i = 0; i < count; i++) {
        parse_stage(result, "response field", data, &d);
        int field = key(&d, enroll ? enroll_keys : verify_keys, count);
        if (field < 0 || (seen & (1u << field))) return -EPROTO;
        seen |= 1u << field;
        parse_stage(result, (enroll ? enroll_keys : verify_keys)[field], data, &d);
        if (field == 0) {
            if (int32_value(&d, &r->status)) return -EPROTO;
            result->service_status = r->status;
        }
        else if (field == 1) { if (int32_value(&d, &r->timeout) || r->timeout < 0) return -EPROTO; }
        else if (enroll && field == 2) { if (uint_value(&d, &r->sid)) return -EPROTO; }
        else if (read_vector(&d, r->data, &r->size, enroll ? GATEKEEPER_HANDLE_MAX : GATEKEEPER_HAT_SIZE)) return -EPROTO;
    }
    parse_stage(result, "response end", data, &d);
    if (d.next != d.end || seen != (1u << count) - 1) return -EPROTO;
    return 0;
}

static int credential_valid(const struct gatekeeper_credential *c)
{
    return c && c->uid == GATEKEEPER_NATIVE_UID && c->secure_user_id &&
           c->handle_size && c->handle_size <= GATEKEEPER_HANDLE_MAX;
}

static int password_valid(const uint8_t *p, size_t size)
{
    return p && size && size <= GATEKEEPER_PASSWORD_MAX;
}

static int exchange_request(keymaster_exchange_fn exchange, void *context,
    struct encoder *e, int enroll, struct response *r, struct gatekeeper_result *result)
{
    if (e->error) return e->error;
    uint8_t *reply = calloc(1, KEYMASTER_REPLY_CAPACITY);
    if (!reply) return -ENOMEM;
    size_t size = KEYMASTER_REPLY_CAPACITY;
    int ret = exchange(context, e->data, e->size, reply, &size);
    result->exchange_status = ret;
    if (!ret) ret = decode(reply, size, enroll, r, result);
    gatekeeper_clear(reply, KEYMASTER_REPLY_CAPACITY); free(reply);
    if (ret) return ret > 0 ? -EIO : ret;
    result->stage = "response semantics";
    result->service_status = r->status;
    if (r->status == -2) {
        if (!r->timeout || r->size || r->sid) return -EPROTO;
        result->retry_ms = r->timeout;
        return -EAGAIN;
    }
    if (r->timeout) return -EPROTO;
    if (r->status) {
        if (r->size || r->sid) return -EPROTO;
        return r->status == -1 ? -EACCES : r->status == -3 ? -ENOTSUP : -EREMOTEIO;
    }
    return 0;
}

static void begin(struct encoder *e, const char *operation)
{
    static const uint8_t wrapper[] = {0xfe, 0xca, 0, 0};
    put(e, wrapper, sizeof(wrapper)); title(e, 5, 1); text(e, operation); title(e, 5, 4);
    text(e, "uid"); title(e, 0, GATEKEEPER_NATIVE_UID);
}

int gatekeeper_enroll(keymaster_exchange_fn exchange, void *context, uint32_t uid,
    const struct gatekeeper_credential *current,
    const uint8_t *password, size_t password_size,
    const uint8_t *desired, size_t desired_size,
    struct gatekeeper_credential *output, struct gatekeeper_result *result)
{
    if (output && output != current) gatekeeper_clear(output, sizeof(*output));
    if (!result) return -EINVAL;
    *result = (struct gatekeeper_result){.stage = "input"};
    if (!output || output == current) return -EINVAL;
    if (!exchange || uid != GATEKEEPER_NATIVE_UID || !password_valid(desired, desired_size) ||
        (current ? (!credential_valid(current) || !password_valid(password, password_size)) : password_size != 0))
        return -EINVAL;
    uint8_t *request = calloc(1, GATEKEEPER_REQUEST_MAX);
    if (!request) return -ENOMEM;
    struct encoder e = {request, 0, 0}; struct response r = {0};
    begin(&e, "Enroll");
    text(&e, "enrolled_password_handle"); vector(&e, current ? current->handle : NULL, current ? current->handle_size : 0);
    text(&e, "password"); vector(&e, password, password_size);
    text(&e, "desired"); vector(&e, desired, desired_size);
    int ret = exchange_request(exchange, context, &e, 1, &r, result);
    if (!ret) result->stage = "credential binding";
    if (!ret && (!r.size || !r.sid || (current && r.sid != current->secure_user_id))) ret = -EPROTO;
    if (!ret) {
        output->uid = uid; output->secure_user_id = r.sid; output->handle_size = r.size;
        memcpy(output->handle, r.data, r.size);
    }
    gatekeeper_clear(request, GATEKEEPER_REQUEST_MAX); free(request);
    gatekeeper_clear(&r, sizeof(r));
    return ret;
}

static uint64_t u64le(const uint8_t *p)
{
    uint64_t v = 0;
    for (unsigned i = 0; i < 8; i++) v |= (uint64_t)p[i] << (i * 8);
    return v;
}

int gatekeeper_verify(keymaster_exchange_fn exchange, void *context,
    const struct gatekeeper_credential *credential, uint64_t challenge,
    const uint8_t *password, size_t password_size,
    uint8_t token[GATEKEEPER_HAT_SIZE], struct gatekeeper_result *result)
{
    if (token) gatekeeper_clear(token, GATEKEEPER_HAT_SIZE);
    if (!result || !token) return -EINVAL;
    *result = (struct gatekeeper_result){.stage = "input"};
    if (!exchange || !credential_valid(credential) || !password_valid(password, password_size)) return -EINVAL;
    uint8_t *request = calloc(1, GATEKEEPER_REQUEST_MAX);
    if (!request) return -ENOMEM;
    struct encoder e = {request, 0, 0}; struct response r = {0};
    begin(&e, "Verify");
    text(&e, "challenge"); title(&e, 0, challenge);
    text(&e, "password_handle"); vector(&e, credential->handle, credential->handle_size);
    text(&e, "password"); vector(&e, password, password_size);
    int ret = exchange_request(exchange, context, &e, 0, &r, result);
    if (!ret) {
        unsigned mac_nonzero = 0;
        for (unsigned i = 37; i < GATEKEEPER_HAT_SIZE; i++) mac_nonzero |= r.data[i];
        if (r.size != GATEKEEPER_HAT_SIZE || r.data[0] || u64le(r.data + 1) != challenge ||
            u64le(r.data + 9) != credential->secure_user_id ||
            memcmp(r.data + 25, "\0\0\0\1", 4) || !mac_nonzero) ret = -EPROTO;
        else {
            memcpy(token, r.data, GATEKEEPER_HAT_SIZE);
            /* This Gatekeeper returns a little-endian timestamp but signs its
             * network-order encoding. Emit the signed wire representation. */
            for (unsigned i = 0; i < 8; i++) token[29 + i] = r.data[36 - i];
        }
    }
    gatekeeper_clear(request, GATEKEEPER_REQUEST_MAX); free(request);
    gatekeeper_clear(&r, sizeof(r));
    return ret;
}
