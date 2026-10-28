/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* OneKeyMint compatibility framing and a deliberately narrow CBOR codec. */
#include "keymaster-shared-secret.h"
#include <errno.h>
#include <limits.h>
#include <string.h>

struct cursor {
    const uint8_t *next, *end;
};

struct parameters {
    uint8_t seed[32], nonce[32];
    size_t seed_size;
};

static void clear(void *memory, size_t size)
{
    volatile uint8_t *p = memory;
    while (size--) *p++ = 0;
}

/* Only definite, minimally encoded integers, arrays and byte strings are used
 * by these operations. Reject all other encodings rather than guessing. */
static int head(struct cursor *c, unsigned *major, uint64_t *value)
{
    if (c->next == c->end) return -EPROTO;
    uint8_t first = *c->next++;
    unsigned ai = first & 31;
    *major = first >> 5;
    *value = ai;
    if (ai < 24) return 0;
    unsigned bytes = ai == 24 ? 1 : ai == 25 ? 2 : ai == 26 ? 4 : ai == 27 ? 8 : 0;
    if (!bytes || (size_t)(c->end - c->next) < bytes) return -EPROTO;
    *value = 0;
    for (unsigned i = 0; i < bytes; i++) *value = (*value << 8) | *c->next++;
    uint64_t minimum = bytes == 1 ? 24 : bytes == 2 ? 256 : bytes == 4 ? 65536 : UINT64_C(4294967296);
    return *value < minimum ? -EPROTO : 0;
}

static int array(struct cursor *c, uint64_t count)
{
    unsigned major;
    uint64_t value;
    return head(c, &major, &value) || major != 4 || value != count ? -EPROTO : 0;
}

static int unsigned_integer(struct cursor *c, uint64_t *value)
{
    unsigned major;
    return head(c, &major, value) || major != 0 ? -EPROTO : 0;
}

static int bytes(struct cursor *c, const uint8_t **data, size_t *size)
{
    unsigned major;
    uint64_t count;
    if (head(c, &major, &count) || major != 2 || count > (uint64_t)(c->end - c->next))
        return -EPROTO;
    *data = c->next;
    *size = (size_t)count;
    c->next += count;
    return 0;
}

/* Success leaves the cursor at the operation's return fields. An error reply
 * must be exactly [negative_i32, []], with no trailing CBOR. */
static int reply(struct cursor *c, uint8_t operation,
                 struct keymaster_agreement_result *result)
{
    unsigned major;
    uint64_t value;
    if (array(c, 2) || head(c, &major, &value) || major > 1 || value > INT32_MAX)
        return -EPROTO;
    result->keymint_status = major ? (int32_t)(-1 - (int64_t)value) : (int32_t)value;
    if (result->keymint_status) {
        if (result->keymint_status > 0 || array(c, 0) || c->next != c->end)
            return -EPROTO;
        return result->keymint_status;
    }
    if (array(c, 1) || array(c, 2) || unsigned_integer(c, &value) || value != operation || array(c, 1))
        return -EPROTO;
    return 0;
}

static int invoke(keymaster_exchange_fn exchange, void *context,
                  const uint8_t *request, size_t request_size,
                  uint8_t *response, struct cursor *c,
                  struct keymaster_agreement_result *result)
{
    size_t size = KEYMASTER_REPLY_CAPACITY;
    result->exchange_status = 0;
    result->keymint_status = 0;
    memset(response, 0, size);
    int ret = exchange(context, request, request_size, response, &size);
    result->exchange_status = ret;
    if (ret) return ret;
    if (size < 8 || size > KEYMASTER_REPLY_CAPACITY) return -EPROTO;
    uint64_t length = 0;
    for (unsigned i = 0; i < 8; i++) length |= (uint64_t)response[i] << (i * 8);
    if (!length || length > size - 8) return -EPROTO;
    c->next = response + 8;
    c->end = c->next + length;
    return 0;
}

static int clock_ready(keymaster_exchange_fn exchange, void *context,
                       struct keymaster_agreement_result *result)
{
    /* 0xdead, CBOR [0x61, [0]]: a harmless challenge used only for readiness. */
    static const uint8_t request[] = {0xad, 0xde, 0, 0, 0x82, 0x18, 0x61, 0x81, 0};
    uint8_t response[KEYMASTER_REPLY_CAPACITY];
    struct cursor c;
    int ret = invoke(exchange, context, request, sizeof(request), response, &c, result);
    if (!ret) ret = reply(&c, 0x61, result);
    if (!ret) {
        uint64_t challenge, timestamp;
        const uint8_t *mac;
        size_t mac_size;
        if (array(&c, 3) || unsigned_integer(&c, &challenge) || challenge != 0 ||
            array(&c, 1) || unsigned_integer(&c, &timestamp) || timestamp > INT64_MAX ||
            bytes(&c, &mac, &mac_size) || mac_size != 32 || c.next != c.end)
            ret = -EPROTO;
    }
    clear(response, sizeof(response));
    return ret;
}

int keymaster_shared_secret_ready(keymaster_exchange_fn exchange, void *context,
                                 struct keymaster_agreement_result *result)
{
    *result = (struct keymaster_agreement_result){.stage = "check readiness"};
    return clock_ready(exchange, context, result);
}

static int get_parameters(keymaster_exchange_fn exchange, void *context,
                          struct parameters *parameters,
                          struct keymaster_agreement_result *result)
{
    static const uint8_t request[] = {0xad, 0xde, 0, 0, 0x82, 0x18, 0x51, 0x80};
    uint8_t response[KEYMASTER_REPLY_CAPACITY];
    struct cursor c;
    int ret = invoke(exchange, context, request, sizeof(request), response, &c, result);
    if (!ret) ret = reply(&c, 0x51, result);
    if (!ret) {
        const uint8_t *seed, *nonce;
        size_t seed_size, nonce_size;
        if (array(&c, 2) || bytes(&c, &seed, &seed_size) || (seed_size != 0 && seed_size != 32) ||
            bytes(&c, &nonce, &nonce_size) || nonce_size != 32 || c.next != c.end) {
            ret = -EPROTO;
        } else {
            memcpy(parameters->seed, seed, seed_size);
            memcpy(parameters->nonce, nonce, nonce_size);
            parameters->seed_size = seed_size;
        }
    }
    clear(response, sizeof(response));
    return ret;
}

static size_t put_bytes(uint8_t *output, const uint8_t *input, size_t size)
{
    size_t prefix = 1;
    if (size < 24) output[0] = 0x40 | size;
    else { output[0] = 0x58; output[1] = (uint8_t)size; prefix = 2; }
    memcpy(output + prefix, input, size);
    return prefix + size;
}

static int compute(keymaster_exchange_fn exchange, void *context,
                   const struct parameters *parameters,
                   struct keymaster_agreement_result *result)
{
    /* One participant: [0x52, [[[seed_bstr, nonce_bstr]]]]. A one-item list
     * is already sorted. The secure service supplies both values. */
    uint8_t request[KEYMASTER_REQUEST_CAPACITY] = {0xad, 0xde, 0, 0, 0x82, 0x18, 0x52, 0x81, 0x81, 0x82};
    size_t size = 10;
    size += put_bytes(request + size, parameters->seed, parameters->seed_size);
    size += put_bytes(request + size, parameters->nonce, sizeof(parameters->nonce));
    uint8_t response[KEYMASTER_REPLY_CAPACITY];
    struct cursor c;
    int ret = invoke(exchange, context, request, size, response, &c, result);
    if (!ret) ret = reply(&c, 0x52, result);
    if (!ret) {
        const uint8_t *check;
        size_t check_size;
        if (bytes(&c, &check, &check_size) || check_size != 32 || c.next != c.end)
            ret = -EPROTO;
    }
    clear(response, sizeof(response));
    clear(request, sizeof(request));
    return ret;
}

int keymaster_shared_secret_agree_single_tee(
    keymaster_exchange_fn exchange, void *context,
    struct keymaster_agreement_result *result)
{
    int ret = keymaster_shared_secret_ready(exchange, context, result);
    if (!ret) { result->already_ready = true; return 0; }
    /* A transport failure numerically equal to -85 must never authorize
     * agreement. Only a well-formed KeyMint -85 reply may do that. */
    if (result->exchange_status || ret != KEYMASTER_NOT_READY || result->keymint_status != KEYMASTER_NOT_READY)
        return ret;
    struct parameters parameters = {0};
    result->stage = "get parameters 0x51";
    ret = get_parameters(exchange, context, &parameters, result);
    if (!ret) {
        result->stage = "compute shared secret 0x52";
        ret = compute(exchange, context, &parameters, result);
    }
    clear(&parameters, sizeof(parameters));
    if (!ret) {
        result->stage = "verify readiness";
        ret = clock_ready(exchange, context, result);
    }
    return ret;
}
