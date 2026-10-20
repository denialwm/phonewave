# JIIOV optical fingerprint (Moto Edge 70)

Native Linux driver for the JIIOV JV0307 under-display sensor on `roadstr`.
The signed `ancapp64` trusted app runs unchanged in QTEE; no Android HAL or
vendor userspace is involved.

Validated on the Moto's 6.6 vendor kernel with the vendor `anc_fps_mmi.ko`
control module: enrollment, persistent templates, match, rejection, deletion,
screen-off unlock through Denial and deep-suspend retention. linux-perpetua has
no JIIOV control driver for `roadstr` yet, so this does not run on it.

## Files

| File | Role |
|---|---|
| `tee-probe.c`, `tee-files.h`, `anc-protocol.c/.h`, `capture-gate.h` | Native worker: smcinvoke transport, `ancapp64` commands, file-system listeners, capture gating |
| `jiiov-fprint-helper.py` | Root worker behind the libfprint driver (`JFP1` pipe protocol) |
| `denial-presentation.c` | Presentation client for `denial-fingerprint-v1` |
| `illumination-test.c`, `diagnostic-ui.h` | Screen-on diagnostic client used for enrollment |
| `jiiov-platform-setup.py` | Loads the sensor module and arms FOD and double-tap gestures |
| `jiiov-wayland-ready.py` | Read-only check, before fprintd starts, of the Wayland socket bind mount it relies on |
| `libfprint/` | Patch series for libfprint v1.94.100, including the JIIOV driver |
| `packaging/` | systemd units, fprintd drop-ins and Denial's wake-gesture config |
| `test-*.c`, `test-wayland-ready.py` | Host tests; no device needed |
| `wayland-protocol-requirements.md` | Requirements that led to `denial-fingerprint-v1` |

## Build

Built and deployed from the Moto workspace,
`/mnt/development/moto70edge/tools/fingerprint/`:

- `build-probe.py` cross-builds the worker and runs the C host tests under
  ASan and UBSan. It needs Qualcomm `minkipc` at
  `moto70edge/sources/common/fingerprint/minkipc`.
- `build-presentation.py` and `build-illumination.py` build the Wayland clients.
- `build-libfprint-phone.py` builds the patched libfprint on the phone.
- `test-helper.py` checks the helper against captured traces from
  `moto70edge/investigation/`.

The bring-up history and live evidence are in that workspace's README.
