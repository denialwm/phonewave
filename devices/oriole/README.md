# Google Pixel 6 (oriole)

Google Tensor gs101, Mali-G78 MP20, 8 GB RAM, UFS. The phone runs Arch
Linux on linux-perpetua from slot B, from the image every phone shares
(`image/`), on `userdata`. boot_b holds the kernel, the early userspace as
its ramdisk, and after them the tail: modules, the Wi-Fi firmware and
boot/tail/. Charging, the display, the touchscreen, the GPU and Wi-Fi work;
Bluetooth, audio, the modem, the cameras and suspend are not there yet.
`ssh 10.77.71.2` reaches the phone over USB.

The kernel replaces the bootloader's device tree with its own (FDT
handoff). Android boot header v4, and no init_boot partition: the
bootloader loads the vendor_boot ramdisk and then boot's, so boot carries
the shared early userspace. `boot/build-boot.sh` takes the ramdisk of the
init_boot image `image/build` wrote (`image/init-boot`) and packs it with
the kernel on the stock boot image's header (linux-perpetua's
`devices/boot/android12-gki`), then puts the tail in the page after the
ramdisk, where the early userspace looks for it, and the AVB footer after
that.

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/oriole`:
stock/ the factory boot image of the tested release (CP1A.260405.005), the
template of the header and footer; firmware/tail/ the Wi-Fi firmware, laid
out as the tail's firmware/ (the stock vendor image's fw_bcmdhd.bin,
bcmdhd_clm.blob and bcmdhd.cal under brcmfmac's names); boot/ the built
images.

Packaging a kernel release N: `boot/build-boot.sh N INIT_BOOT [--name
NAME]` builds it through linux-perpetua-build unless the release exists,
and writes the vault's boot/boot-NAME.img. A new init_boot needs a new boot
image.

## Partitions

Stock Android CP1A.260405.005 is on slot B, the active one; slot A is
untouched.

- Never write an `_a` partition, never switch the active slot, and never
  write vbmeta, dtbo, vendor_boot, bootloader, firmware, modem, persist or
  calibration partitions.
- `boot_b` (PARTUUID `93f8d242-1761-4911-83f1-cdefb87ef4e9`) is the only
  boot-chain partition a kernel may go to, stated before the write and read
  back afterwards. Its rollback is the stock `boot.img` of the factory
  image.
- `userdata` (PARTUUID `a73b8825-c087-4c16-b100-552da42e9027`) holds the
  root. Stock Android cannot use it any more: it asks for a factory reset,
  which would erase the install. Do not confirm it.

## Installing

`image/build` builds the shared image and its init_boot; `boot/build-boot.sh`
packs that init_boot into boot-NAME.img. From the bootloader (Power + Volume
Down, held from power-off until its menu appears):

```sh
fastboot flash boot_b boot-NAME.img
fastboot reboot
```

With no image on `userdata` the phone stops in the rescue, whose fastboot
takes the root, and then the SSH key, which it keeps in the root it finds
installed, so after the root:

```sh
fastboot flash userdata userdata.img
fastboot flash ssh-key ~/.ssh/id_ed25519.pub
fastboot reboot
```

The phone starts in setup; the flashed key becomes the account's SSH key.
`fastboot boot boot-NAME.img` starts a kernel without writing it, but the
early userspace then finds no modules: it mounts them from the boot
partition that holds the running kernel's.

## Fastboot and recovery

The early userspace stops in its rescue (its own fastboot, a shell on the
USB serial port) when a volume key is held while it starts, when the root
holds `/etc/phone-rescue`, or when no root is found. Leave the rescue with
`sync; reboot -f`.

Back to stock Android: flash the factory image's `boot.img` to `boot_b`, and
its `userdata` handling as Google's flashing instructions say (a factory
reset from Android's recovery does the same).

## Wi-Fi

A BCM4389 on PCIe, on brcmfmac. The address is the bootloader's, handed off
from its `/chosen/config`. WPA2 and WPA3-Personal work on 2.4 and 5 GHz;
the firmware leaves SAE to wpa_supplicant.

## Denial

exynosdrm drives the panel and has no render node; panfrost renders. The
tail's `denial.service.d/oriole.conf` points Denial at panfrost's render
node.
