# OPLUS UFF optical fingerprint (OnePlus Ace 3)

Native Linux driver for the Ace 3's under-display sensor on `aston`: a Goodix
G7s behind OPLUS's UFF stack. The signed `uff_spi` and `uff_gx` trusted apps
run unchanged in QTEE; no Android HAL or vendor userspace is involved.

The kernel side is `drivers/misc/oplus-uff-fp.c` in linux-perpetua (power,
reset and IRQ; `CONFIG_OPLUS_UFF_FP`), with panel illumination in the
`aston` panel driver. Enrollment, match, rejection and screen-off unlock
through Denial work on `7.3.0-devices35-aston`.

## Files

| File | Role |
|---|---|
| `uff-probe.c`, `uff-wire.h`, `uff-storage.h`, `uff-gp-storage.h`, `uff-template-names.h` | Native transport: QTEE objects, `uff_gx` commands, confined storage listeners |
| `uff-service-worker.c`, `uff-resident.h` | Worker child that keeps blocking secure calls out of the helper |
| `uff-fprint-helper.py` | Root bridge behind the Aston libfprint driver (`UFF1` pipe protocol); NX809J uses its own native helper |
| `uff-denial-presentation.c`, `uff-contact.h` | Presentation client for `denial-fingerprint-v1`, adapted from the Moto one; `uff-contact.h` holds each phone's touch axes and sensor circle |
| `libfprint/uff.c` | libfprint driver, built as `drivers/uff.c` in libfprint. It also serves the NX809J's Goodix sensor through `goodix-fprint-helper`, which speaks the same `UFF1` |
| `libfprint/test-uff-suspend.c` | libfprint test for suspend and resume, built as `tests/test-uff-suspend.c` |
| `register-print.c` | Registers an enrolled template with fprintd |
| `test-wire.c`, `test-contact.c`, `test_helper.py` | Host tests; no device needed |
| `uff-storage-test.c`, `test-libfprint-passive.c`, `test-libfprint-optical.c` | Tests that run on the phone: storage confinement, libfprint discovery, real optical enroll and verify |

## Build

The scripts in `build/` cross-build from this directory and write only to
`${XDG_CACHE_HOME:-~/.cache}/phonewave/aston/fingerprint/`:

- `build_native.py` builds `uff-probe` and the `uff-service-worker` child,
  statically, against the kernel's `uff_fp.h`. It clones quic-teec, QCBOR and
  minkipc at the commits it pins into `~/.cache/phonewave/src/`, applies
  `deps/<name>/*.patch` on top, and builds `libqcomtee.a` and `libqcbor.a`.
- `build_presentation.py` builds the presentation client against the cross
  sysroot `/mnt/development/denial-os/cross/sysroot` and Denial's protocol XML.
- `assemble_firmware.py` assembles `uff_gx.elf` and `uff_spi.elf` from the
  factory `odm_b` blobs in the device vault (`aston/firmware/factory/`).
- `snapshot_libfprint.py [aston|nx809j]` makes the libfprint source archive
  in that phone's cache: upstream
  v1.94.100 (`80a4b5ec`), `../jiiov/libfprint/0001`-`0007`,
  `libfprint/0008-build-uff-driver.patch`, then `uff.c` and
  `test-uff-suspend.c`. The archive name is its hash.

Staging and installing on the phone are in `devices/aston/tools/fingerprint/`.

The phone runs older builds than these sources: worker `b072d4d9e9d6`,
helper `b85a1c64bd46` and libfprint `418190cd903a`. The worker and the
libfprint archive are kept in the device vault (`aston/build-deps/`); the
helper is in `devices/aston/rootfs/`. A `-g` build records absolute paths, so
a rebuild of the same source matches the code but not the file hash.
