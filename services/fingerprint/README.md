# Fingerprint

Under-display fingerprint sensors that are only reachable through a signed
trusted app in the Qualcomm TEE. One package serves every phone: the
`fingerprint-phonewave` recipe (`image/recipes/fingerprint-phonewave`) builds
libfprint with the uff driver, the helper the driver starts, a worker for
each sensor family and the contact children.

| Path | Hardware | Trusted app | Device |
|---|---|---|---|
| [`uff`](uff) | OPLUS UFF with a Goodix G7s optical sensor | `uff_gx` | `aston` |
| [`goodix`](goodix) | Goodix GUM5228 ultrasonic | `goodixfp64` | `nx809j` |
| [`jiiov`](jiiov) | JIIOV JV0307 optical | `ancapp64` | `roadstr` |

`uff/` also holds what every phone shares: the libfprint driver and its
patches, the helper, the presentation and touch contact clients, the
calibration import and fprintd's drop-in.

## How they fit together

- **libfprint driver** (`uff/libfprint/uff.c`): lets fprintd enroll, verify
  and store prints. It finds the sensor by its kernel driver and starts the
  helper.
- **Helper** (`uff/helper/`): reads the phone's `fingerprint/sensor.conf`,
  runs the worker it names and the contact child, and checks every message
  between them and the driver.
- **Worker** (`uff-worker`, `goodix-worker`, `jiiov-worker`): loads the
  vendor's signed trusted app unchanged through qcomtee and serves its file
  callbacks. Calibration stays read-only.
- **Contact**: `presentation` asks Denial, through `denial-fingerprint-v1`,
  to light the sensor area while a finger is down, with the screen on or
  off; `touch` waits for the touchscreen to report a finger on the mark and
  lights nothing.

A phone's boot image tail says what is its own: `fingerprint/sensor.conf`
(the sensor's id and name, worker, signed app, contact and calibration
files), the signed app under `firmware/`, and Denial's
`denial/fingerprint.json` (the mark, and for an optical sensor the panel and
its lighting). On a phone whose tail names no sensor the driver finds no
device and nothing of this runs.

The kernel side belongs in linux-perpetua: `drivers/misc/oplus-uff-fp.c`
for aston, `drivers/misc/tee-fp.c` (`/dev/tee_fp`) for the Goodix and JIIOV
sensors, and `/dev/fp_illumination` in the panel drivers of the optical
phones (`Documentation/ABI/testing/dev-fp-illumination`).

## Adding a phone

A sensor family already here needs only the kernel side, the tail's
`sensor.conf` and `denial/fingerprint.json`, and the signed app. A new family
needs a worker for its trusted app, which the helper starts as
`<Worker>-worker`, and the kernel driver's entry in `uff.c`.

## Tests

Every build of the recipe runs the host tests of the wire formats
and the JIIOV worker's file service under ASan and UBSan. The helper's
tests run with `cargo test` in `uff/helper/`, the Goodix ones with
`goodix/test_*.py`. `uff/` names the tests that run on a phone.

## Licenses

Most of these files are not MIT. Source files say which license applies in
their SPDX header: the libfprint drivers and workers are LGPL-2.1-or-later,
like libfprint; code built against kernel UAPI headers is GPL-2.0-only. The
`libfprint/*.patch` series modifies libfprint and is LGPL-2.1-or-later. Files
without a header, such as the setup scripts and Python host tests, are MIT
like the rest of Phonewave.
