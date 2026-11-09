# Nubia RedMagic 11 Pro (nx809j)

The phone runs Arch Linux on linux-perpetua from slot B, from the image every
phone shares (`image/`), on a partition of its own named `denial`; Android
keeps `userdata`. The kernel replaces the bootloader's device tree with its
own (FDT handoff). boot_b holds the kernel and after it the tail: modules,
the GPU, Wi-Fi, Bluetooth and fingerprint firmware and boot/tail/. init_boot_b
holds the init_boot every phone shares (`image/init-boot`). `ssh nx809j`
reaches the phone as root over USB at 10.77.71.2.

The phone moved to the image on 2026-11-06. What the old root held and where
each piece went is in REMINDERS.md.

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/nx809j`:
record.md, the add-device record; boot/ the built images and the devices231
boot image they take their header from; firmware/tail/ the firmware the tail
carries, laid out as its firmware/; backups/ this unit's partition copies
(`backups/manifest.json`), the old root (nx809j-before-image-20261106) and
Android's storage (droidloom-20261109, a `phone-backup`); private-rootfs/ root's
authorized_keys, which init_boot also carries, the SSH host keys, the Wi-Fi
connection and sudoers; survey/ what was read from the phone.

Packaging a kernel release N: `boot/build-boot.sh N [--name NAME]` builds it
through linux-perpetua-build unless the release exists, and writes the vault's
boot/boot-NAME.img. The ADSP and modem firmware are not in the tail: they do
not fit beside the rest, and come from the phone's own modem partition
(`image/stock-firmware`), as on the Ace 3.

## Partitions

Only `boot_b` and `init_boot_b` are written, one at a time. Never write
an `_a` partition, never switch the active slot, and never write vbmeta,
dtbo, vendor_boot, bootloader, firmware or calibration partitions. Slot A
is marked unbootable by the bootloader; if slot B ever becomes unbootable
too, ask the user before any slot command. The rollback for each partition
is its stock copy in `backups/slot_b/`.

Native credential storage has one explicit user-authorized exception:
`goodix-secure-storage.service` may update encrypted secure files under
`persist/data` and relay trusted-app authenticated RPMB commits in its
canonical mode (`services/fingerprint`). Factory calibration files remain read-only; RPMB key
programming and raw partition flashing remain forbidden. A fresh persist
backup is held privately, but RPMB monotonic state has no verified rollback.
The user accepted that limitation for native PIN/fingerprint bring-up.

The stock ABL answers `fastboot flash` and `fastboot boot` with "unknown
command", and fastbootd reports the device locked and refuses downloads.
Images are written with `dd` from the running system, or with fastboot from
the init_boot's rescue, which writes this slot's `boot` and `init_boot` and
the `denial` partition. Before each write, check that the running slot is
`_b`; read the partition back and compare hashes afterwards. If nothing
boots, the partitions are restored over EDL.

Slot B's boot attributes live in the GPT entry of `boot_b` on `sde`. This
bootloader leaves the successful bit set on every boot, so rescue boots
do not use up retries.

## Installing

Android's f2fs `userdata` was shrunk online from rooted Android
(`F2FS_IOC_RESIZE_FS`) and the end of `sda` became sda13, GPT PARTUUID
`2225dabe-3937-4455-83a4-58a737b38f2f`; the vault keeps the partition tables
(`backups/gpt*`). It was named `linux` until the move, `denial` since, so
the image boots from it and leaves Android's data alone. Android still boots
from slot B's stock images; formatting `userdata` from Android is its own
recovery and leaves `denial` alone.

`image/build nx809j --droidloom --dev-access` builds the image; its
`userdata.img` goes on `denial`. With boot-NAME.img on boot_b and the shared
init_boot on init_boot_b, a phone whose `denial` holds no image stops in the
rescue (SSH at 10.77.71.2, fastboot), and from there:

```sh
fastboot flash denial userdata.img
fastboot reboot
```

Android's storage goes back with `phone-backup HOST --restore FILE` (or
`droidloomctl restore` on the phone).

## Audio

The speakers are two AW88461 amplifiers on PRIMARY_MI2S, set up from the
stock tuning (`qcom/kaanapali/nubia/nx809j/aw88461_acf.bin`). The UCM
profile is `kaanapali/Nubia-NX809J` in services/audio/ucm, and the image's
WirePlumber rule gives PipeWire software volume. The profile routes
MultiMedia1 to the speakers, keeps the ADSP stream gain at unity and sets
both amplifiers' `Playback Volume` to 959: 1023 is the stock tuning's level,
and each step of 64 is 6 dB. Stock protects the speakers in Android's audio
DSP, which has no counterpart here, so stay at or under 959. PipeWire sets
the volume in software.

Three analog microphones sit on the WCD939x: AMIC1 (stock's handset and
speakerphone mic, the default), AMIC4 (stock's game mic) and AMIC3. AMIC2
and AMIC5 have none. The profile records one at a time through ADC1 or ADC3
into MultiMedia3, at stock's gains (ADC 12, decimators 84). Capture needs
linux-perpetua 9ba37ef1db70 (devices-223).

Calls: the ADSP carries call speech between the modem and the hardware once
the kernel's q6apm voice session has started its two graphs, from the voice
profile `qcom/kaanapali/Nubia-NX809J-voice.bin`. boot/build-boot.sh makes it
with services/audio/voicegen from the stock ACDB, as `audio/voice.toml`
says: both speakers on PRIMARY_MI2S, the speaker mic (AMIC1) on
TX_CODEC_DMA_TX_3, as stock pairs them. The UCM verb "Voice Call" puts
Speaker and Mic on the policy PCM 13, and callaudio switches the card to it
while dmail has a call. "Voice Call Volume" takes the ACDB's ten levels
(9 loudest, the default); "TX_AIF1 Capture Switch" mutes the microphone.

## Denial

The image's `denial` package and `denial.service`, as on every phone;
`pacman -Syu` takes a new release. Denial scales the panel by its pixel
density, 2 here.

## Always-on display

The RM692H5 cannot refresh slower than 60 Hz in normal mode, so its
driver's always-on blank uses the panel's idle mode instead of display-off:
the panel keeps showing its last frame from its own memory, at
`ambient_brightness` (stock 0x0120 = 288), while the display hardware is
off. The tail's `denial/ambient-display.json`, which the image copies to
`/etc/denial`, names the switch, the normal-mode
code whose picture looks like idle mode's (`brightness`, 400) and how long
the finished picture holds still before the switch (`settle_ms`, 1000; a
switch at full rate draws a wave). Before DSI-1 goes dark the lock screen
keeps the time, the date, the wallpaper's subject and the fingerprint mark,
darkens the rest of the wallpaper and lets both fade into black below the
clock, while the backlight fades to that code. Denial sets the switch for
each always-on blank, lights the output once a minute for the clock while
the panel stays in idle mode, and clears it on wake, which leaves idle
mode at the backlight's level. The image lets the video group write it.
Kernels from devices166.

## Modem

dmail (services/modem/dmail) owns it, as on every phone: its `modem-setup`
starts rmtfs on the phone's own EFS partitions, read-only, and tqftpserv
(services/modem/linux-msm). The tail turns dmail on and carries its device
file, `boot/tail/dmail/device.toml`. The data endpoints are those of
Kaanapali's IPA (AP to modem 2, modem to AP 24), the same as the Ace 3. The
modem firmware comes from modem_b (`boot/tail/stock-firmware.conf`).

VoLTE: the `[ims]` section of the device file names the phone as the
stock Android on both slots does (REDMAGICOS11.5.8MR1_EA: Android 16, SDK 36,
build BQ2A.250705.001-BP2A.250605.031.A3), read from `system_a` and
`system_b` in super. The modem firmware comes from that build.

## Sensors

steinerd (services/sensors/steinerd) offers orientation, acceleration,
rotation rate, compass heading, light and proximity from the ADSP's sensor
core: lsm6dsv accelerometer and gyroscope, stk3a7x light and proximity,
ak0991x magnetometer, through the core's AKM rotation vector for the
compass. The ADSP reads its sensor
configuration from the host when it starts, so the tail turns on
`hexagonrpcd.service` (services/sensors/hexagonrpcd), which serves it from
the stock `vendor_b` and persist, copied at each start, read-only.
`steinerctl list accel proximity ambient_light` shows the three sensors.

## Camera

braun (services/camera/braun), in every image, finds the OV50E40 main
camera, the OV50D40 ultrawide, the OV02F10 auxiliary camera and the OV16A1Q
front camera, and develops the Camera app's photos. The tail carries
`braun/ov50e40.toml`, `ov50d40.toml` and `ov02f10.toml`: each
camera's role, the ultrawide's zoom against the main camera (0.5x, 120
against about 84 degrees across the diagonal) and the main camera's focus
travel through its DW9785. None has colour calibration yet. Never run the DW9785's firmware download: it
rewrites the motor's eFlash.

## Battery stats

battery-stats (services/power/battery-stats), in every image, books the
battery per state, hour and build. The kernel release names the build: Denial is the
image's package.

## Fingerprint

The stock Goodix ultrasonic stack uses `goodixfp64`; the kernel provides
power, reset, IRQ and `/dev/tee_fp`. Native PIN setup, fingerprint
enrollment and lock-screen matching have succeeded. Preserve persist's
`goodix/` factory calibration and keep it read-only. Historical power-on
resets and the voltage correction are recorded in the private device vault.

fprintd runs on the `fingerprint-phonewave` packages, as on every phone
(services/fingerprint). The phone's part is in the boot tail:
`fingerprint/sensor.conf` names the sensor, the `goodix` worker, the touch
contact and the warm worker, and points at the signed app,
`firmware/goodix/goodixfp64.elf`, which
`services/fingerprint/goodix/build.py --assemble-only` assembles from the
stock `vendor_firmware_mnt`. `denial/fingerprint.json` places the mark. The
tail also turns on the Goodix PIN and secure storage units, with fprintd's
`70-goodix.conf` drop-in.

| Path | What |
|---|---|
| `/var/lib/phonewave-fingerprint/calibration` | This unit's calibration, copied once from persist's `goodix/` mounted read-only |
| `/var/lib/phonewave-fingerprint/templates` | The worker's private store |
| `/var/lib/goodix-secure-storage` | Secure storage's private overlay, the default mode |

The worker runs one-shot per operation (about 0.8 s for a list). For
verification it brings the sensor up and selects the templates while the
lock screen listens, screen on or off, and keeps it asleep until the touch.
After a rejected finger it stays up for the next attempt, up to 5 s, in
`/run/phonewave-fingerprint`. Contact is the touchscreen reporting one
finger held on stock's circle, centre (608, 2024) px, radius 120, also with
the panel off; nothing is drawn or lit. A finger lifted before the image is
captured fails the attempt; after it, the comparison goes on without it.

Both Settings apps enroll through deniald's PIN-gated fingerprint session.
The fingerprint marker uses the stock circle above.

The native PIN provider and secure storage are the package's
`goodix-gatekeeper.socket` and `goodix-secure-storage.service`
(services/fingerprint). The sensor worker delegates the shared listener so
Gatekeeper can verify an enrollment challenge while the sensor is active.
The user authorized canonical mode for this phone, with the rollback
limitation in Partitions; its drop-ins are in the phone's /etc since
2026-11-06, after persist was saved to the vault again.

The phone's fingerprint runtime uses native executables from the package.
The Python scripts in the development checkout assemble the signed app and
run tests; they are not part of the on-device unlock path. The bridge and worker
log unlock phase durations to fprintd's journal.
