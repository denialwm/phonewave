# OPLUS UFF optical fingerprint (OnePlus 12R), and the shared stack

The 12R's under-display sensor on `aston` is a Goodix G7s behind OPLUS's
UFF stack. The signed `uff_gx` trusted app runs unchanged in QTEE; no Android
HAL or vendor userspace is involved. The kernel side is
`drivers/misc/oplus-uff-fp.c` in linux-perpetua (power, reset and IRQ;
`CONFIG_OPLUS_UFF_FP`), and the panel driver lights the sensor through
`/dev/fp_illumination`.

This folder also holds what every phone's sensor shares; the package and how
a phone names its sensor are in `../README.md`.

## Files

| File | Role |
|---|---|
| `uff-probe.c`, `uff-wire.h`, `uff-storage.h`, `uff-gp-storage.h`, `uff-template-names.h` | UFF transport: QTEE objects, `uff_gx` commands, confined storage listeners |
| `uff-service-worker.c`, `uff-resident.h` | The `uff` worker, built on `uff-probe.c` |
| `helper/` | The helper fprintd's uff driver starts, for every sensor (Rust) |
| `libfprint/uff.c` | libfprint driver, built as `drivers/uff.c` in libfprint |
| `libfprint/*.patch` | libfprint v1.94.100 (`80a4b5ec`) patches: tests without introspection, platform device discovery, the uff driver in the build |
| `libfprint/test-uff-suspend.c` | libfprint test for suspend and resume, built as `tests/test-uff-suspend.c` |
| `uff-denial-presentation.c` | `presentation` contact: asks Denial through `denial-fingerprint-v1` to light the sensor area the helper passes it |
| `uff-touch-contact.c`, `uff-contact.h` | `touch` contact: a finger held on the mark, from the touchscreen alone |
| `calibration`, `system/` | Imports the sensor's factory calibration, read-only; fprintd's drop-in; Denial's profile copied from the tail to `/etc/denial/fingerprint.json` |
| `include/linux/`, `protocol/` | The kernel's `uff_fp.h` and `tee_fp.h`, and Denial's `denial-fingerprint-v1.xml`, as the package builds against them |
| `deps/minkipc/` | Patch on Qualcomm minkipc, whose storage listeners the workers link |
| `test-wire.c`, `test-contact.c` | Host tests; every build of the recipe runs them |
| `uff-storage-test.c`, `test-libfprint-passive.c`, `test-libfprint-optical.c` | Tests that run on the phone: storage confinement, libfprint discovery, real optical enroll and verify |
| `build/assemble_firmware.py` | Assembles `uff_gx.elf` and `uff_spi.elf` from the factory `odm_b` blobs in the vault (`aston/firmware/factory/`) |

## The 12R's part

The tail's `fingerprint/sensor.conf` names the `uff` worker, the
`presentation` contact and `/dev/fp_illumination`, and points at
`firmware/uff/uff_gx.elf`, from the vault's `aston/firmware/tail/`. The
factory calibration stays on the phone: `calibration` copies persist's
`fingerprint/` files into `/var/lib/phonewave-fingerprint/calibration` once,
before fprintd first opens the sensor. Templates stay in the TEE's storage
under `/var/lib/phonewave-fingerprint/secure-cache`.

The helper finds the compositor through `denial.service`: its `User` is the
phone's user, and the presentation client accepts only that user's Denial.
