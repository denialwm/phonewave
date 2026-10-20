# Services

Daemons and drivers that give Denial one hardware capability, grouped by that
capability. Each project directory is self-contained, with its own build,
tests, packaging and README.

| Path | Purpose | Devices |
|---|---|---|
| [`haptics/hapticd`](haptics/hapticd) | D-Bus service for evdev force-feedback and sysfs vibrators | any |
| [`clock/rtc-offset`](clock/rtc-offset) | Keeps the system clock across reboots when the RTC cannot be set | any |
| [`fingerprint/jiiov`](fingerprint/jiiov) | JIIOV JV0307 optical fingerprint | `roadstr` |
| [`fingerprint/uff`](fingerprint/uff) | OPLUS UFF (Goodix G7s) optical fingerprint | `aston` |

A service that works on any device never hard-codes one. It reads
configuration or discovers hardware through standard interfaces (udev
properties, sysfs, evdev, D-Bus), and the device-specific part goes in
`devices/<codename>/rootfs/`. A service for one chip names it, and the devices
that carry it, in its README.
