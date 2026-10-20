# JIIOV optical fingerprint (Moto Edge 70)

Native Linux driver for the JIIOV JV0307 under-display sensor on `roadstr`.
The signed `ancapp64` trusted app runs unchanged in QTEE; no Android HAL or
vendor userspace is involved.

Validated on the Moto's 6.6 vendor kernel with the vendor `anc_fps_mmi.ko`
control module: enrollment, persistent templates, match, rejection, deletion,
screen-off unlock through Denial and deep-suspend retention. The kernel side is
not in linux-perpetua yet, so this does not run on it; the vendor sources are in
`/mnt/development/baseline-kernels/roadstr/moto-6.6/`:
`motorola-kernel-modules/drivers/input/misc/anc_fps_mmi` (`/dev/jiiov_fp`),
`kernel-devicetree/qcom/kera-roadstr-fingerprint-overlay.dtsi` and
`vendor-qcom-opensource-securemsm-kernel` (`/dev/smcinvoke`).

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
| `packaging/` | systemd units, fprintd drop-ins, the system-sleep resume hook and Denial's fingerprint and wake-gesture configs |
| `test-*.c`, `test-wayland-ready.py`, `test-helper.py` | Host tests; no device needed |
| `traces/` | Status traces captured on the phone, used by `test-helper.py` |
| `wayland-protocol-requirements.md` | Requirements that led to `denial-fingerprint-v1` |

## Build

Outputs go to `${XDG_CACHE_HOME:-~/.cache}/phonewave/roadstr/`.

- `build-probe.py` cross-builds the worker and runs the C host tests under
  ASan and UBSan. On first use it clones Qualcomm `minkipc` at `bb99356` into
  `~/.cache/phonewave/src/minkipc`.
- `build-presentation.py` and `build-illumination.py` build the Wayland clients
  against `/mnt/development/cross/sysroot`.
- `assemble-firmware.py` joins the split `ancapp64` trusted app from the
  vault's `roadstr/firmware/jiiov/` into the `ancapp64.elf` the worker loads.

The phone-side steps (building the patched libfprint on the phone, installing
it, optical tests) are in `devices/roadstr/tools/fingerprint/`.
