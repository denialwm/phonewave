/* SPDX-License-Identifier: LGPL-2.1-or-later */
#ifndef NX809J_GATEKEEPER_H
#define NX809J_GATEKEEPER_H

#include "keymaster-shared-secret.h"

#define GATEKEEPER_NATIVE_UID 941003u
#define GATEKEEPER_PASSWORD_MAX 1024u
#define GATEKEEPER_HANDLE_MAX 4096u
#define GATEKEEPER_HAT_SIZE 69u
#define GATEKEEPER_REQUEST_MAX 16384u

/* The Android user identifier below is a credential namespace, NOT a caller
 * UID or a secure user ID. Keep this native namespace separate from Android.
 * Persist uid, secure_user_id and the exact handle together; never the PIN.
 * All sizes are byte counts. Passwords are opaque bytes without a terminator;
 * PIN policy/derivation belongs to the caller and must be stable across calls. */
struct gatekeeper_credential {
    uint32_t uid;
    uint64_t secure_user_id;
    size_t handle_size;
    uint8_t handle[GATEKEEPER_HANDLE_MAX];
};

struct gatekeeper_result {
    int exchange_status;
    int32_t service_status;
    uint32_t retry_ms;
    /* Non-secret decoder diagnostics: locations/types only, never values. */
    const char *stage;
    size_t reply_size, parse_offset;
    unsigned parse_major;
};

/* Borrow keymaster64 using the real caller identity, then complete the native
 * shared-secret setup before calling these operations. Serialize credential
 * changes and durably save a successful replacement handle before reporting
 * success to the user. No delete/reset/empty-password operation is exposed.
 *
 * First enrollment: current=NULL, password=NULL, password_size=0; caller must
 * establish that this is a new native identity, never reset an existing one.
 * PIN change: current plus its current password are mandatory. A failed change
 * leaves current untouched. Output must not alias current. On failure output
 * is cleared (except an invalid aliased output, which preserves current).
 *
 * Return 0 on success; -EAGAIN with retry_ms for secure throttling, -EACCES for
 * rejected credentials, otherwise a negative errno. Never retry automatically.
 * No buffers are logged. Caller owns and must erase its own password copies. */
int gatekeeper_enroll(keymaster_exchange_fn exchange, void *context, uint32_t uid,
    const struct gatekeeper_credential *current,
    const uint8_t *password, size_t password_size,
    const uint8_t *desired, size_t desired_size,
    struct gatekeeper_credential *output, struct gatekeeper_result *result);

/* challenge may be zero for PIN-only verification. For Goodix, pass the exact
 * fresh challenge while retaining the same fingerprint session. Success returns
 * the unchanged 69-byte signed token. Its version, challenge, secure user ID,
 * password authenticator type and MAC length are checked; only the trusted
 * consumer can validate its MAC. Any failure clears all token bytes. */
int gatekeeper_verify(keymaster_exchange_fn exchange, void *context,
    const struct gatekeeper_credential *credential, uint64_t challenge,
    const uint8_t *password, size_t password_size,
    uint8_t token[GATEKEEPER_HAT_SIZE], struct gatekeeper_result *result);

void gatekeeper_clear(void *memory, size_t size);

#endif
