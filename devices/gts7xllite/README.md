# Samsung Galaxy Tab S7 FE 5G (gts7xllite)

SM-T736B. Qualcomm SM7225 (Snapdragon 750G, downstream `lagoon`), Adreno 619,
12.4" BOE TS124QDM panel (1600x2560, used as landscape), 5G modem. The
shared image runs from the internal UFS `userdata`; `boot` carries the
kernel, the shared early userspace and the modules' tail.

Works: display (60 fps), GPU, touch, Wi-Fi, Bluetooth, battery and charging
status, suspend (s2idle), RTC, auto-rotation and the ADSP sensors, the Book
Cover Keyboard (EF-DT730) and the S Pen in apps. Not yet: audio, the modem,
cameras. What is tested, and when: `FEATURES.md`.

## Access

The shared early userspace's USB gadget: ECM network and ACM serial. The
tablet is 10.77.71.2 and hands the computer 10.77.71.1; SSH as on every
phone.

## Flashing

- Samsung has no fastboot. Download mode: power off, hold Vol-Up + Vol-Down,
  plug in USB, then Vol-Up on the warning screen (or `adb reboot download`
  from stock). `odin4` writes a ustar tar in the AP slot:
  `tar --format=ustar -cf boot.tar boot.img && odin4 -a boot.tar --reboot`.
- Only `boot` is ever written. Never `vbmeta*`, `dtbo`, bootloader or
  firmware partitions. Never flash firmware below bootloader revision 9.
- The bootloader must be unlocked: the image keeps stock's signed vbmeta,
  which no longer matches.

## Recovery

Download mode, then the stock `boot` from T736BXXS9DYF1.

## Boot image

Android boot header v2, no `vendor_boot` or `init_boot`: kernel, ramdisk and
DTB all in the 96 MiB `boot`. Board-id 05 (dtbo index 3) is the overlay the
bootloader applies.

`boot/build-boot.sh INIT_BOOT OUTPUT` packs it from a built linux-perpetua
tree through its `devices/boot/android-v2`: the kernel, the ramdisk of an
`init_boot.img` from `image/init-boot` (with this checkout's `/init`) as
gzip, and the tail (modules and `boot/tail/`) at the page after the DTB,
before Samsung's signer trailer. The early userspace finds the tail from the
v2 header. The tail also carries the touch firmware, from `FIRMWARE`
(Samsung's; not here). `DEV_KEY` adds a key to the rescue's SSH.

## Device files

- `gts7-touchd` (`services/input/gts7-touchd`): the FT8203 touchscreen has
  no kernel driver. The tail turns the daemon on; the tty1 login and Denial
  wait for its input device. Its firmware is Samsung's, built into the stock
  kernel, so `build-boot.sh` puts it in the tail (`FIRMWARE`).
- `logind.conf.d/gts7xllite.conf`: Denial owns the power key
  (`pm8941_pwrkey`).
- `zram-swap` is enabled, as on every device. The kernel builds zram in.
- `rmtfs.service.d/10-gts7xllite.conf`: Wi-Fi's firmware service runs on the
  modem DSP, which needs rmtfs, and this modem does not boot by itself:
  `-s` starts it. The file system is the tablet's own `modemst1`,
  `modemst2` and `fsg`, read-only.
- `sleep.conf.d/80-gts7xllite-sleep.conf`: `AllowSuspend=yes` over the image's
  `70-phone-no-sleep.conf`, so logind lets Denial suspend the tablet.
  Hibernation stays off.
- `boot/tail/`: `gts7-touchd`, `hexagonrpcd` and `rtc-offset` turned on,
  hexagonrpcd waiting for its node, and `steinerd/top` (Sensors).

## Sensors

The kernel holds the sensor rails `vreg_l10a` (3.304 V) and `vreg_l8e`
(1.8 V) always-on, as stock does: without them the ADSP's sensor drivers
time out and its sensors process restarts every few seconds. The tail turns
on `hexagonrpcd.service`; `stock-sensors` works unchanged (vendor, odm and
persist; the tablet has no A/B slots). hexagonrpcd waits for
`/dev/fastrpc-adsp`, the only FastRPC node the ADSP has here. steinerd then
offers the lsm6dso accelerometer and gyroscope, the ak0991x magnetometer and
the veml3235 light sensor.

The panel scans out portrait and Denial turns it to landscape
(`DSI-1,270`), adding the sensor's rotation on top. So steinerd's screen
frame is the landscape one: the sensor core's +x points to its top edge
(docked on the Book Cover Keyboard, accel x reads +8.4, y about 0), and the
tail's `steinerd/top` says `+x`. With the default `+y`, steinerd reports
`right-up` when docked and Denial turns 270+270, to portrait.

The RTC is read-only: the tail turns on `rtc-offset`.

## Suspend

s2idle is the only sleep state (`/sys/power/mem_sleep` = `[s2idle]`). It
resumes on battery: the power key and an RTC alarm wake it, Wi-Fi, USB and
SSH come back. With the USB cable in, the gadget keeps the tablet awake
(dwc3 'HS-PHY not in L2'), so test it unplugged.

Denial suspends on idle, but the mobile shell has no setting for it (the
image leaves Denial's desktop Settings out). Set it in
`~/.config/denial/settings.json`:

```json
"power": {
  "idleDpmsEnabled": true,
  "idleDpmsTimeoutSeconds": 30,
  "idleSuspendEnabled": true,
  "idleSuspendTimeoutSeconds": 300,
  "suspendMode": "s2idle"
}
```

- `suspendMode` must be `s2idle`: Denial's default is `deep`, which this
  tablet does not have.
- deniald applies the file live, but only with a `"version"` (30) and a
  `"revision"` higher than before; otherwise it logs "rejected externally
  edited Denial settings".
- A short press of the power key only blanks the screen; it does not suspend.

The battery shows in Denial and UPower once the fuel gauge reports
`charge_full` (10090 mAh, stock `battery,battery_full_capacity`).

## Display settings

The output transform (`DSI-1,270`) and Denial's gamut correction off (with
it on, landscape drops below 60 fps) are set on the tablet; the image does
not write them yet.

## Known issues

- About 2 boots in 5 reset at ~0.8 s, during the display and GPU bring-up
  (the DSI PLL taken over from the bootloader's splash); the next try boots.
