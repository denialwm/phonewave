/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Synthetic protocol/flow tests. No phone, firmware or authentication material. */
#include "keymaster-shared-secret.h"
#include "keymaster-wrapped-key.h"
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>

enum action { CLOCK, PARAMETERS, COMPUTE };
struct step {
    enum action action;
    unsigned char response[128];
    size_t size;
    int exchange_status;
};
struct fake {
    struct step steps[4];
    unsigned count, current;
    size_t seed_size;
};

static void frame(struct step *step, const unsigned char *cbor, size_t size)
{
    assert(size + 8 <= sizeof(step->response));
    memset(step->response, 0, sizeof(step->response));
    for (unsigned j = 0; j < 8; j++) step->response[j] = (uint64_t)size >> (j * 8);
    memcpy(step->response + 8, cbor, size);
    step->size = size + 8;
}

static void error_reply(struct step *step, unsigned code)
{
    assert(code > 24 && code <= 256);
    unsigned char cbor[] = {0x82, 0x38, code - 1, 0x80};
    frame(step, cbor, sizeof(cbor));
}

static void ready_reply(struct step *step)
{
    unsigned char cbor[49] = {0x82, 0, 0x81, 0x82, 0x18, 0x61, 0x81, 0x83, 0, 0x81, 0x1a, 0, 1, 0xe2, 0x40, 0x58, 32};
    /* Synthetic HMAC-shaped data, never checked against a real secret. */
    memset(cbor + 17, 0xaa, 32);
    frame(step, cbor, sizeof(cbor));
}

static void parameter_reply(struct step *step, size_t seed_size)
{
    unsigned char cbor[76] = {0x82, 0, 0x81, 0x82, 0x18, 0x51, 0x81, 0x82};
    size_t size = 8;
    if (seed_size) {
        cbor[size++] = 0x58; cbor[size++] = 32;
        for (unsigned j = 0; j < 32; j++) cbor[size++] = j + 64;
    } else cbor[size++] = 0x40;
    cbor[size++] = 0x58; cbor[size++] = 32;
    for (unsigned j = 0; j < 32; j++) cbor[size++] = j + 1;
    frame(step, cbor, size);
}

static void computed_reply(struct step *step)
{
    unsigned char cbor[41] = {0x82, 0, 0x81, 0x82, 0x18, 0x52, 0x81, 0x58, 32};
    memset(cbor + 9, 0xbb, 32);
    frame(step, cbor, sizeof(cbor));
}

static struct fake normal(size_t seed_size)
{
    struct fake f = {.count = 4, .seed_size = seed_size};
    f.steps[0].action = CLOCK; error_reply(&f.steps[0], 85);
    f.steps[1].action = PARAMETERS; parameter_reply(&f.steps[1], seed_size);
    f.steps[2].action = COMPUTE; computed_reply(&f.steps[2]);
    f.steps[3].action = CLOCK; ready_reply(&f.steps[3]);
    return f;
}

static int exchange(void *context, const void *request, size_t request_size,
                    void *response, size_t *response_size)
{
    struct fake *f = context;
    assert(f->current < f->count);
    struct step *step = &f->steps[f->current++];
    static const unsigned char clock[] = {0xad, 0xde, 0, 0, 0x82, 0x18, 0x61, 0x81, 0};
    static const unsigned char parameters[] = {0xad, 0xde, 0, 0, 0x82, 0x18, 0x51, 0x80};
    if (step->action == CLOCK) {
        assert(request_size == sizeof(clock));
        assert(!memcmp(request, clock, request_size));
    } else if (step->action == PARAMETERS) {
        assert(request_size == sizeof(parameters));
        assert(!memcmp(request, parameters, request_size));
    } else {
        unsigned char expected[78] = {0xad, 0xde, 0, 0, 0x82, 0x18, 0x52, 0x81, 0x81, 0x82};
        size_t size = 10;
        if (f->seed_size) {
            expected[size++] = 0x58; expected[size++] = 32;
            for (unsigned j = 0; j < 32; j++) expected[size++] = j + 64;
        } else expected[size++] = 0x40;
        expected[size++] = 0x58; expected[size++] = 32;
        for (unsigned j = 0; j < 32; j++) expected[size++] = j + 1;
        assert(size == (f->seed_size ? 78 : 45));
        assert(request_size == size);
        assert(!memcmp(request, expected, size));
    }
    assert(*response_size == KEYMASTER_REPLY_CAPACITY);
    if (step->exchange_status) return step->exchange_status;
    memcpy(response, step->response, step->size);
    *response_size = step->size;
    return 0;
}

static int run(struct fake *f, struct keymaster_agreement_result *result)
{
    return keymaster_shared_secret_agree_single_tee(exchange, f, result);
}

struct wrapped_fake {
    unsigned char response[KEYMASTER_WRAPPED_KEY_SIZE];
    size_t size;
    int transport_error;
    unsigned calls;
};

static int wrapped_exchange(void *context, const void *request, size_t request_size,
                            void *response, size_t *response_size)
{
    struct wrapped_fake *f = context;
    static const unsigned char expected[] = {5, 2, 0, 0, 2, 0, 0, 0};
    assert(request_size == sizeof(expected) && !memcmp(request, expected, sizeof(expected)));
    assert(*response_size == KEYMASTER_WRAPPED_KEY_SIZE);
    f->calls++;
    if (f->transport_error) return f->transport_error;
    memcpy(response, f->response, sizeof(f->response));
    *response_size = f->size;
    return 0;
}

static void wrapped_tests(void)
{
    struct wrapped_fake valid = {.size = 256};
    valid.response[4] = 12; valid.response[8] = 152;
    memset(valid.response + 12, 0xcc, 152); /* synthetic opaque data */
    unsigned char reply[KEYMASTER_WRAPPED_KEY_SIZE];
    struct keymaster_wrapped_key_result result;
    struct wrapped_fake f = valid;
    assert(!keymaster_fingerprint_wrapped_key(wrapped_exchange, &f, reply, &result));
    assert(f.calls == 1 && !memcmp(reply, valid.response, sizeof(reply)));
    assert(result.status == 0 && result.offset == 12 && result.length == 152);
    for (size_t size = 0; size < 256; size++) {
        f = valid; f.size = size;
        assert(keymaster_fingerprint_wrapped_key(wrapped_exchange, &f, reply, &result) == -EPROTO);
        for (unsigned j = 0; j < sizeof(reply); j++) assert(reply[j] == 0);
    }
    for (unsigned bad = 0; bad < 7; bad++) {
        f = valid;
        if (bad == 0) memset(f.response, 0xff, 4); /* failed TA status */
        if (bad == 1) f.response[4] = 8; /* offset into header */
        if (bad == 2) f.response[8] = 0; /* empty payload */
        if (bad == 3) memset(f.response + 4, 0xff, 4); /* offset overflow */
        if (bad == 4) memset(f.response + 8, 0xff, 4); /* length overflow */
        if (bad == 5) f.size = 257; /* reported length beyond capacity */
        if (bad == 6) f.transport_error = -EIO;
        memset(reply, 0xdd, sizeof(reply));
        assert(keymaster_fingerprint_wrapped_key(wrapped_exchange, &f, reply, &result));
        assert(f.calls == 1);
        for (unsigned j = 0; j < sizeof(reply); j++) assert(reply[j] == 0);
    }
    puts("PASS: 517/2 exact request, opaque reply preservation and clearing on every failure");
}

int main(void)
{
    wrapped_tests();
    struct keymaster_agreement_result result;
    for (unsigned seed = 0; seed <= 32; seed += 32) {
        struct fake f = normal(seed);
        assert(!run(&f, &result) && f.current == 4 && !result.already_ready);
    }
    puts("PASS: current-boot parameters delivered unchanged; signing checked after agreement");
    {
        struct fake f = normal(0); ready_reply(&f.steps[0]);
        assert(!run(&f, &result) && f.current == 1 && result.already_ready);
        assert(!strcmp(result.stage, "check readiness"));
    }
    puts("PASS: already agreed key preserved; no parameter or compute operation");
    {
        struct fake f = normal(0); error_reply(&f.steps[0], 45);
        assert(run(&f, &result) == -45 && f.current == 1);
        assert(result.keymint_status == -45);
        f = normal(0); f.steps[0].exchange_status = KEYMASTER_NOT_READY;
        assert(run(&f, &result) == KEYMASTER_NOT_READY && f.current == 1);
        assert(result.exchange_status == KEYMASTER_NOT_READY && result.keymint_status == 0);
        f = normal(0); f.steps[1].exchange_status = -EIO;
        assert(run(&f, &result) == -EIO && f.current == 2);
    }
    puts("PASS: caller/service/transport errors stop without attempting agreement");
    {
        struct fake f = normal(0); error_reply(&f.steps[2], 38);
        assert(run(&f, &result) == -38 && f.current == 3);
        assert(!strcmp(result.stage, "compute shared secret 0x52"));
        f = normal(0); error_reply(&f.steps[3], 85);
        assert(run(&f, &result) == KEYMASTER_NOT_READY && f.current == 4);
        assert(!strcmp(result.stage, "verify readiness"));
    }
    puts("PASS: rejected agreement or absent post-agreement signing never reports success");
    /* Exhaust every truncation of each valid response, including the length
     * prefix, and ensure no next command is sent. */
    for (unsigned which = 0; which < 4; which++) {
        struct fake original = normal(0);
        for (size_t size = 0; size < original.steps[which].size; size++) {
            struct fake f = original; f.steps[which].size = size;
            assert(run(&f, &result) == -EPROTO && f.current == which + 1);
        }
    }
    puts("PASS: every truncated reply rejected before any subsequent command");
    {
        struct fake f = normal(0);
        memset(f.steps[0].response, 0xff, 8);
        assert(run(&f, &result) == -EPROTO && f.current == 1);
        f = normal(0); f.steps[1].response[8 + 5] = 0x52; /* wrong operation */
        assert(run(&f, &result) == -EPROTO && f.current == 2);
        f = normal(0); f.steps[1].response[8 + 10] = 31; /* wrong nonce length */
        assert(run(&f, &result) == -EPROTO && f.current == 2);
        f = normal(0); f.steps[1].response[8 + 8] = 0x5f; /* indefinite bytes */
        assert(run(&f, &result) == -EPROTO && f.current == 2);
        f = normal(0); f.steps[2].response[8 + 8] = 31; /* short sharing check */
        assert(run(&f, &result) == -EPROTO && f.current == 3);
        f = normal(0); f.steps[3].response[8 + 8] = 1; /* wrong challenge */
        assert(run(&f, &result) == -EPROTO && f.current == 4);
        f = normal(0); f.steps[3].response[8 + 9] = 0; /* missing timestamp wrapper */
        assert(run(&f, &result) == -EPROTO && f.current == 4);
        f = normal(0); f.steps[3].response[8 + 16] = 31; /* short signing MAC */
        assert(run(&f, &result) == -EPROTO && f.current == 4);
        f = normal(0); f.steps[1].response[0]++; f.steps[1].response[f.steps[1].size++] = 0;
        assert(run(&f, &result) == -EPROTO && f.current == 2); /* trailing CBOR */
        f = normal(0); f.steps[0].response[8 + 3] = 0x81; /* malformed error */
        assert(run(&f, &result) == -EPROTO && f.current == 1);
    }
    puts("PASS: oversized, mismatched and malformed replies rejected");
    {
        struct fake f = normal(0);
        assert(keymaster_shared_secret_ready(exchange, &f, &result) == KEYMASTER_NOT_READY);
        assert(f.current == 1);
    }
    puts("PASS: status mode never performs agreement");
    return 0;
}
