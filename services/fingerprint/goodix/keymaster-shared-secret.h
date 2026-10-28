/* SPDX-License-Identifier: LGPL-2.1-or-later */
#ifndef GOODIX_KEYMASTER_SHARED_SECRET_H
#define GOODIX_KEYMASTER_SHARED_SECRET_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define KEYMASTER_REPLY_CAPACITY 32768u
#define KEYMASTER_REQUEST_CAPACITY 96u
#define KEYMASTER_NOT_READY (-85)

/* Plain compatibility invocation; response_size is capacity in, length out.
 * This adapter must not log request or response bytes. */
typedef int (*keymaster_exchange_fn)(void *context, const void *request,
                                    size_t request_size, void *response,
                                    size_t *response_size);

struct keymaster_agreement_result {
    const char *stage;
    int exchange_status;
    int32_t keymint_status;
    bool already_ready;
};

/* Readiness check only. Returns KEYMASTER_NOT_READY if agreement is absent. */
int keymaster_shared_secret_ready(keymaster_exchange_fn exchange, void *context,
                                 struct keymaster_agreement_result *result);

/* For the native platform with exactly one TEE ISharedSecret participant.
 * Caller serializes startup. Never replaces an already usable agreed key.
 * No credential, provisioning, root-of-trust or recipient-policy operations.
 * Returns zero on readiness, otherwise a KeyMint, transport or protocol error. */
int keymaster_shared_secret_agree_single_tee(
    keymaster_exchange_fn exchange, void *context,
    struct keymaster_agreement_result *result);

#endif
