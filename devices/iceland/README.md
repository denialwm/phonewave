# OnePlus Pad 4 (iceland)

The tablet runs Arch Linux on linux-perpetua from slot B, from the image every
phone shares (`image/`), on `userdata`: the tablet has no Android data to
keep. boot_b holds the kernel and after it the tail: modules, the GPU, touch,
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

Only `boot_b` and `init_boot_b` are written, one at a time. Never
set_active, erase, format, GPT edits or EDL: tablet #1 was lost that way. The
tail turns no slot marking on, so nothing writes the partition table.

Write an image with `dd` from the running system: first check that
`androidboot.slot_suffix` in `/proc/bootconfig` is `_b`, and write by
PARTUUID, boot_b `18963643-3a34-5d30-252d-3ce92701780e`, init_boot_b
`b71f7665-41b5-608b-2da4-25618656aad0`. Read the partition back and compare
hashes. The rescue's fastboot writes this slot's `boot` and `init_boot` too.
The rollback for both is vault `boot/devices225/artifacts/`, the old root's
pair.

## Installing

`image/build` builds the shared image; its `userdata.img` goes on
`userdata`. With boot-NAME.img on boot_b and the
shared init_boot on init_boot_b, a tablet whose `userdata` holds no image
stops in the rescue (SSH at 10.77.71.2, fastboot), and from there:

```sh
fastboot flash userdata userdata.img
fastboot reboot
```

## Audio

The ADSP runs the factory firmware from modem_b, split .mdt images, which
the kernel loads by itself once the early userspace binds them under
`qcom/kaanapali/oneplus/iceland`. The tail carries the topology,
`qcom/kaanapali/OnePlus-Pad4-tplg.bin`.

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
