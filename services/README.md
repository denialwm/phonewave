# Services

Daemons and drivers that give Denial one hardware capability, grouped by that
capability. Each project directory is self-contained, with its own build,
tests, packaging and README.

| Path | Purpose | Devices |
|---|---|---|
| [`haptics/hapticd`](haptics/hapticd) | D-Bus service for evdev force-feedback and sysfs vibrators | any |
| [`clock/rtc-offset`](clock/rtc-offset) | Keeps the system clock across reboots when the RTC cannot be set | any |
| [`memory/zram-swap`](memory/zram-swap) | Compressed swap in RAM, sized from the installed memory | any |
| [`pin/phonewave-gatekeeper`](pin/phonewave-gatekeeper) | Keeps the lock PIN behind root where the hardware has no PIN provider | any |
| [`boot/slot-success`](boot/slot-success) | Marks the Linux slot booted successfully once the boot has proved itself, so the bootloader keeps booting it | any A/B Qualcomm |
| [`wallpaper/wallpaper-depth`](wallpaper/wallpaper-depth) | Cuts the wallpaper's subject out so the lock screen clock sits behind it | any with Vulkan |
| [`screen/screen-record`](screen/screen-record) | Records the screen; Denial's shade starts and stops it | any with GBM and EGL |
| [`power/battery-stats`](power/battery-stats) | Books where the battery goes, per screen state, hour and build | any with a battery |
| [`fingerprint/jiiov`](fingerprint/jiiov) | JIIOV JV0307 optical fingerprint | `roadstr` |
| [`fingerprint/uff`](fingerprint/uff) | OPLUS UFF (Goodix G7s) optical fingerprint | `aston` |
| [`modem/qmi-tools`](modem/qmi-tools) | Small QMI clients for cellular bring-up | `aston` |
| [`camera/braun`](camera/braun) | Finds the cameras and gives apps previews and photos, raw to JPEG in its own pipeline | any with qcom-camss |
| [`sensors/steinerd`](sensors/steinerd) | Orientation, motion, compass, light and proximity from the Snapdragon Sensor Core | `nx809j`, `pudding` |
| [`sensors/hexagonrpcd`](sensors/hexagonrpcd) | Serves the ADSP's sensor drivers their configuration: linux-msm's hexagonrpcd with Phonewave's patches | `nx809j`, `pudding` |
| [`modem/dmail`](modem/dmail) | Modem service replacing ModemManager: SIM, registration, data, SMS | `aston`, `roadstr` |

A service that works on any device never hard-codes one. It reads
configuration or discovers hardware through standard interfaces (udev
properties, sysfs, evdev, D-Bus), and the device-specific part goes in
`devices/<codename>/rootfs/`. A service for one chip names it, and the devices
that carry it, in its README.
