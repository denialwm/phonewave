/* SPDX-License-Identifier: LGPL-2.1-or-later */
#include "keymaster-wrapped-key.h"
#include <errno.h>
#include <string.h>

static uint32_t u32le(const uint8_t *bytes)
{
    return (uint32_t)bytes[0] | (uint32_t)bytes[1] << 8 |
           (uint32_t)bytes[2] << 16 | (uint32_t)bytes[3] << 24;
}

int keymaster_fingerprint_wrapped_key(
    keymaster_exchange_fn exchange, void *context,
    uint8_t reply[KEYMASTER_WRAPPED_KEY_SIZE],
    struct keymaster_wrapped_key_result *result)
{
    /* Stock get-authentication-token-key, fingerprint authenticator type. */
    static const uint8_t request[] = {0x05, 0x02, 0, 0, 2, 0, 0, 0};
    size_t size = KEYMASTER_WRAPPED_KEY_SIZE;
    *result = (struct keymaster_wrapped_key_result){0};
    memset(reply, 0, size);
    int ret = exchange(context, request, sizeof(request), reply, &size);
    result->exchange_status = ret;
    if (ret) goto failed;
    if (size != KEYMASTER_WRAPPED_KEY_SIZE) { ret = -EPROTO; goto failed; }
    result->status = (int32_t)u32le(reply);
    result->offset = u32le(reply + 4);
    result->length = u32le(reply + 8);
    if (result->status) { ret = result->status; goto failed; }
    if (result->offset < 12 || result->offset > size || !result->length ||
        result->length > size - result->offset) {
        ret = -EPROTO;
        goto failed;
    }
    return 0;
 failed:
    /* Even callers that mishandle a failure must not retain opaque material. */
    for (volatile uint8_t *p = reply; p < reply + KEYMASTER_WRAPPED_KEY_SIZE; p++) *p = 0;
    return ret;
}
