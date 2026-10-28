/* SPDX-License-Identifier: LGPL-2.1-or-later */
#ifndef GOODIX_KEYMASTER_WRAPPED_KEY_H
#define GOODIX_KEYMASTER_WRAPPED_KEY_H

#include "keymaster-shared-secret.h"

#define KEYMASTER_WRAPPED_KEY_SIZE 256u

struct keymaster_wrapped_key_result {
    int exchange_status;
    int32_t status;
    uint32_t offset, length;
};

/* The Goodix recipient app must already be loaded, and remain loaded until
 * it consumes this opaque reply. The caller must also complete normal
 * shared-secret agreement first. On failure the entire reply is cleared.
 * Never log the wrapped payload. No recipient/caller override is supported. */
int keymaster_fingerprint_wrapped_key(
    keymaster_exchange_fn exchange, void *context,
    uint8_t reply[KEYMASTER_WRAPPED_KEY_SIZE],
    struct keymaster_wrapped_key_result *result);

#endif
