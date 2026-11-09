/* SPDX-License-Identifier: LGPL-2.1-or-later */
#ifndef GOODIX_SECURE_STORAGE_H
#define GOODIX_SECURE_STORAGE_H
#include <stddef.h>
#include <stdint.h>
/* All descriptors are preopened directories. Canonical mode, the service's
 * choice while persist is writable, uses the actual protected-file directory,
 * paired with authenticated commit. Otherwise it reads stock base and updates
 * private Linux directories. */
struct secure_store { int base, persist, data; int canonical; };
int secure_gpfs(struct secure_store *, void *, size_t);
/* Only RPMB authenticated READ and READ COUNTER frames reach this transport.
 * The transport must not interpret or log frame contents. */
typedef int (*secure_rpmb_io)(void *, unsigned, void *, uint32_t);
int secure_rpmb(void *, size_t, secure_rpmb_io, void *);
/* Authenticated-write mode must use canonical persist backing files,
 * never the private diagnostic overlay. It still rejects key programming. */
int secure_rpmb_authenticated(void *, size_t, secure_rpmb_io, void *);
#endif
