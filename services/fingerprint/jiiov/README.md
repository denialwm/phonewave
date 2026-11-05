# JIIOV optical fingerprint (Moto Edge 70)

The `jiiov` worker for the JIIOV JV0307 under-display sensor on `roadstr`.
It loads the signed `ancapp64` trusted app unchanged through the upstream
qcomtee driver (`/dev/tee0`) and powers and resets the sensor through
`/dev/tee_fp` (`drivers/misc/tee-fp.c` in linux-perpetua, compatible
`jiiov,jv0307`). The panel driver lights the sensor through
`/dev/fp_illumination`, and Denial asks for it while a finger is down. The
rest of the stack is shared (`../README.md`).

## Files

| File | Role |
|---|---|
| `jiiov-worker.c` | The worker: qcomtee transport, `ancapp64` commands, enroll, verify, list and delete |
| `anc-protocol.c/.h` | The trusted app's request and reply format |
| `jiiov-files.h` | The file service the trusted app calls back into, confined to the worker's state |
| `test-files.c`, `test-protocol.c` | Host tests; every build of the recipe runs them |
| `assemble-firmware.py` | Joins the split `ancapp64` from the stock segments into the `ancapp64.elf` the worker loads; `devices/roadstr/boot/build-boot.sh` runs it |

## The Edge 70's part

The tail's `fingerprint/sensor.conf` names the `jiiov` worker, the
`presentation` contact and `/dev/fp_illumination`, and points at
`firmware/jiiov/ancapp64.elf`. The factory calibration stays on the phone:
`calibration` copies persist's `jiiov/` files into
`/var/lib/phonewave-fingerprint/calibration` once, read-only.
`denial/fingerprint.json` places the mark and names the panel,
`csot,nt37706-v3`, whose lighting Denial checks before it uses it.
