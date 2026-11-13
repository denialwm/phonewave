# OnePlus Pad 4 (iceland)

The tablet runs Arch Linux on linux-perpetua from slot B, from the image every
phone shares (`image/`), on `denial`, an 80 GiB partition made on 2026-11-11
after `userdata`, which shrank to make room and still holds the root of
2026-11-06 with its Droidloom storage. boot_b holds the kernel and after it the tail: modules, the GPU, touch,
Wi-Fi and Bluetooth firmware, the audio topology and boot/tail/. init_boot_b
holds the init_boot every phone shares (`image/init-boot`). `ssh iceland-usb`
reaches the tablet as root over USB at 10.77.71.2, `ssh iceland` as logix
over Wi-Fi.

The tablet moved to the image on 2026-11-06. What the old root held and
where each piece went is in REMINDERS.md.

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/iceland`:
boot/ the built images, the devices225 release whose boot image they take
their header from, and every release the old root booted; firmware/tail/ the
firmware the tail carries, laid out as its firmware/; stock/ the OPD2514
stock images and OTA; backups/ partition copies; private-rootfs/ root's
authorized_keys, which init_boot also carries, and sudoers.

Packaging a kernel release N: `boot/build-boot.sh N [--name NAME]` builds it
through linux-perpetua-build unless the release exists, and writes the
vault's boot/boot-NAME.img. The ADSP firmware is not in the tail: it comes
from the tablet's own modem partition (`image/stock-firmware`).

## Partitions

Only `boot_b`, `init_boot_b` and `denial` are written, one at a time. Never
set_active, erase, format or EDL: tablet #1 was lost to empty xbl and abl
partitions. slot-success, from the shared image, marks slot B successful
once a boot has run a minute with Denial and SSH up; it writes only those
attribute bits of the partition table.

`sda`, which holds no part of the boot chain, got its `denial` partition from
the rescue: `e2fsck -f` and `resize2fs` on `userdata` (the rescue's clock
must be set first, or resize2fs asks for the check again; 4.5 minutes for
72 GB moved), then `sfdisk /dev/sda` from an edited `sfdisk --dump`, which
keeps every other entry as it was. The tools came from the installed root,
copied to the rescue's /tmp with their libraries. The table before and after
is in the vault, `backups/gpt-sda-20261111-*`.

Write an image with `dd` from the running system: first check that
`androidboot.slot_suffix` in `/proc/bootconfig` is `_b`, and write by
PARTUUID, boot_b `18963643-3a34-5d30-252d-3ce92701780e`, init_boot_b
`b71f7665-41b5-608b-2da4-25618656aad0`. Read the partition back and compare
hashes. The rescue's fastboot writes this slot's `boot` and `init_boot` too.
The rollback for both is vault `boot/devices225/artifacts/`, the old root's
pair.

## Installing

`image/build` builds the shared image; its `userdata.img` goes on
`denial`. With boot-NAME.img on boot_b and the shared init_boot on
init_boot_b, the image's `flash.sh` writes it from the rescue's fastboot.

## Display

The NT36536 LCD is a video-mode panel on both DSI links with DSC 1.2. The
bootloader leaves it running with its picture and the kernel goes on from
there, restarting nothing (linux-perpetua dd400323a583, devices-337): the
boot log is on the bootloader's framebuffer until the display driver has the
controller up, then the driver's first frame goes out on the timing engine,
the DSI links, the PLL, the panel and the backlight as they run. The device
tree asks for it with the MDSS node's `memory-region`, and gives that node
the second power domain the bootloader's pipeline runs on,
`DISP_CC_MDSS_CORE_INT2_GDSC`.

The kernel log says `intf_1: bootloader's stream goes on at frame N` at the
take-over. The frame counter, MDP `0x9801000` + INTF + `0xac`, counts on from
the bootloader's start at 120 a second: measured twice, no frame lost.

## Audio

The ADSP runs the factory firmware from modem_b, split .mdt images, which
the kernel loads by itself once the early userspace binds them under
`qcom/kaanapali/oneplus/iceland`. The tail carries the topology,
`qcom/kaanapali/OnePlus-Pad4-tplg.bin`.

## Sensors

steinerd offers orientation, acceleration, rotation rate, compass heading,
light and proximity from the ADSP's sensor core: bmi3x0 accelerometer and
gyroscope, ak0991x magnetometer, stk36c01 light and proximity, bu52053nvx
hall. The tail turns on `hexagonrpcd.service` (services/sensors/hexagonrpcd),
which serves the ADSP its configuration from the stock `vendor_b`, `odm_b`
and persist. The tablet's own sensors are described in odm's
`etc/sensor/config`; with vendor's alone the core finds none.
The sensor core's x axis points to the screen's top edge, the long one
with the camera's side up in landscape as the panel scans out: the tail's
`steinerd/top` says `+x`.

With the sensor core running, the panel lost part of a frame whenever the
memory fell to its lowest frequency, at the end of each animation, until the
display driver's peak bandwidth vote stopped being lower than its average
(linux-perpetua 183c4eb728a1, devices-322). `underrun:` in
`/sys/kernel/debug/dri/0/encoder-0/status` counts those frames.

## Bluetooth

The WCN7860's Peri takes its factory RAM patch and NVM, `qca/pad4/` in the
tail, through options of the kernel's own driver that the device tree's
command line sets.

## Clock

The PMIC RTC counts from a base Linux cannot set. The tail turns on
rtc-offset (services/clock/rtc-offset), which keeps the offset in
`/var/lib/rtc-offset`.

## Cooling

The tablet cools the SoC through its back alone: its device tree throttles
the GPU from 90 °C instead of the SoC's 105.

## Denial and Droidloom

The image's `denial` package and `denial.service`, as on every phone;
`pacman -Syu` takes a new release. Droidloom is the shared package pair with
the Google apps package; its Android storage is
`/var/lib/droidloom/users/1000/host-storage-20261022`.
