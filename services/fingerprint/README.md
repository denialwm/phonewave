# Fingerprint

Under-display fingerprint sensors that are only reachable through a signed
trusted app in the Qualcomm TEE. Each directory holds one sensor's sources,
tests, packaging and README.

| Path | Hardware | Device |
|---|---|---|
| [`jiiov`](jiiov) | JIIOV JV0307 optical under-display sensor, `ancapp64` trusted app | `roadstr` |
| [`uff`](uff) | OPLUS UFF with a Goodix G7s optical sensor, `uff_gx` trusted app | `aston` |

## How they fit together

Both have the same three parts:

- **libfprint driver** (`libfprint/`): lets fprintd enroll, verify and store
  prints. It talks to the worker over a private pipe protocol.
- **Native worker**: loads the vendor's signed trusted app unchanged and
  serves its file-system callbacks. Calibration stays read-only.
- **Presentation client**: asks Denial, through `denial-fingerprint-v1`, to
  light the sensor area while a finger is down, with the screen on or off.

The kernel side (sensor power, reset, IRQ, panel illumination) belongs in
linux-perpetua, not here. For `aston` that is `drivers/misc/oplus-uff-fp.c`.

## Licenses

Most of these files are not MIT. Source files say which license applies in
their SPDX header: the libfprint drivers and workers are LGPL-2.1-or-later,
like libfprint; code built against kernel UAPI headers is GPL-2.0-only. The
`libfprint/*.patch` series modifies libfprint and is LGPL-2.1-or-later. Files
without a header, such as the setup scripts and Python host tests, are MIT
like the rest of Phonewave.

## Building

No standalone build yet. The bring-up scripts in each device's workspace
build, package and deploy from here; each README names them.
