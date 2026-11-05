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
| `libfprint/uff.c` | libfprint driver, built as `drivers/uff.c` in libfprint. It serves every sensor a boot tail names, through the helper |
| `libfprint/test-uff-suspend.c` | libfprint test for suspend and resume, built as `tests/test-uff-suspend.c` |
| `uff-calibration`, `system/` | Imports the sensor's factory calibration from the persist partition, read-only; fprintd's drop-in and the state folder |
| `include/linux/`, `protocol/` | The kernel's `uff_fp.h`, and Denial's `denial-fingerprint-v1.xml`, as the package builds against them |
| `register-print.c` | Registers an enrolled template with fprintd |
| `test-wire.c`, `test-contact.c`, `test_helper.py` | Host tests; no device needed |
| `uff-storage-test.c`, `test-libfprint-passive.c`, `test-libfprint-optical.c` | Tests that run on the phone: storage confinement, libfprint discovery, real optical enroll and verify |

## Package

`image/recipes/fingerprint-uff` builds two packages from this directory,
and every phone's image carries them:

- `libfprint-uff`: libfprint v1.94.100 (`80a4b5ec`), the release Arch Linux
  ARM ships, with `../jiiov/libfprint/0001`-`0007`,
  `libfprint/0008-build-uff-driver.patch` and `uff.c`: Arch's default
  drivers and the uff driver. It replaces Arch's libfprint; fprintd is
  Arch's.
- `fingerprint-uff`: the helper at `/usr/lib/uff-fingerprint/uff-fprint-helper`,
  where the driver starts it, the static worker, the presentation client and
  `uff-calibration`. The worker links quic-teec, QCBOR and minkipc at the
  commits the recipe pins, with `deps/<name>/*.patch` on top.

A phone with the sensor says so in its boot image's tail, which carries what
is the phone's own:

- `systemd/system/fprintd.service.d/50-uff.conf`: the sensor's devices in
  fprintd's sandbox, the state folder `/var/lib/phonewave-fingerprint` and the
  calibration import. fprintd's sandbox hides `/usr/lib/modules`, so the
  drop-in binds the signed app at `/run/uff-firmware`.
- the signed `uff_gx` app (`firmware/uff/uff_gx.elf`, from the vault's
  `aston/firmware/tail/`).
- Denial's `denial/fingerprint.json`, which the package copies to
  `/etc/denial/fingerprint.json`.

On a phone without them the uff driver finds no sensor, fprintd lists none and
nothing of this runs. The factory calibration stays on the phone:
`uff-calibration` copies it from the persist partition's `fingerprint/` into
`/var/lib/phonewave-fingerprint/calibration` once, before fprintd first opens the
sensor. Templates stay in the TEE's storage under
`/var/lib/phonewave-fingerprint/secure-cache`.

The helper finds the compositor through `denial.service`: its `User` is the
phone's user, and the presentation client accepts only that user's Denial.

## Build for bring-up

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

