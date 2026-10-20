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
| `uff-fprint-helper.py` | Root bridge behind the libfprint driver (`UFF1` pipe protocol) |
| `uff-denial-presentation.c`, `uff-contact.h` | Presentation client for `denial-fingerprint-v1`, adapted from the Moto one |
| `libfprint/uff.c` | libfprint driver, built as `drivers/uff.c` in libfprint |
| `libfprint/test-uff-suspend.c` | libfprint test for suspend and resume, built as `tests/test-uff-suspend.c` |
| `register-print.c` | Registers an enrolled template with fprintd |
| `test-wire.c`, `test-contact.c`, `test_helper.py` | Host tests; no device needed |
| `uff-storage-test.c`, `test-libfprint-passive.c`, `test-libfprint-optical.c` | Tests that run on the phone: storage confinement, libfprint discovery, real optical enroll and verify |

## Build

Built and deployed from the Ace 3 workspace,
`/mnt/development/denial-ace3/v7.3-port/step3/fingerprint/`:

- `build_native.py` cross-builds the worker against `libqcomtee`, QCBOR and
  `minkipc` from `step3/secure-modem`, and the kernel's `uff_fp.h`.
- `build_presentation.py` builds the presentation client.
- `snapshot_libfprint.py` puts `libfprint/` into a libfprint source archive;
  `build_libfprint.py` builds it on the phone.
- `test_storage.py`, `test_libfprint_passive.py` and `run_libfprint_optical.py`
  run the phone tests.

The build manifests in that workspace still record the sources at their old
paths. The next `build_native.py` run records the new ones.

The bring-up history, live evidence and current deployment are in that
workspace's README.
