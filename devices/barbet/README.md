# Google Pixel 5a (barbet)

Qualcomm SM7250 (Snapdragon 765G), 6.34-inch 1080x2400 Samsung SOFEF01 at
60 Hz, ST FTS touchscreen, TI DRV2624 haptics, UFS storage. The WCN3990 Wi-Fi
and Bluetooth sit behind the modem; the RT5514P DSP and the TAS2562 pair
carry the microphones and the speakers.

The phone runs linux-perpetua from slot B. The bootloader uses a version 2
boot image with the kernel, the ramdisk and the device tree in the boot
partition; init_boot is not used. The kernel carries its own Barbet tree and
hands the bootloader's Google tree over to it. Slot A keeps stock Android and
is never written.

## Boot image

`build-boot.sh N` builds release `devices-N-barbet` unless it already exists,
packs its modules and `boot/tail/` with `image/boot-tail`, writes the shared
early userspace with `image/init-boot --ramdisk`, and packs the result with
linux-perpetua's `devices/boot/android-v2`.

```sh
devices/barbet/boot/build-boot.sh N [--name NAME] [BUILD-OPTIONS...]
```

The vault's `barbet/backups/boot_b.img` is the verified working image whose
version 2 header, command line and AVB properties are the template. The
output goes to `$PHONEWAVE_VAULT/barbet/boot/boot-NAME.img` and is never
rewritten: take a new name for every image.

`android-v2` keeps the template's header and unsigned AVB footer and replaces
the kernel, ramdisk and DTB. The template's kernel is an LZ4 frame, so the
built Image is recompressed with the template's settings and its arm64
`text_offset` becomes the template's `0x80000`. `--tail` writes the
`image/boot-tail` filesystem at the page after the DTB; the early userspace
finds it from the kernel, ramdisk and DTB sizes in the header.

## Installing

From fastboot, or from the rescue's fastboot over USB:

```sh
fastboot flash boot_b boot-N.img
```

The boot userspace is the shared rescue from `image/init-boot`, so it can
also write the root image; `image/README.md` describes that step.

## Notes

- The modem boots the WLAN firmware only while the ADSP is offline, and the
  Bluetooth side answers only after the WLAN has powered the combo chip. The
  order those need is tracked in [issue #10](https://github.com/denialwm/phonewave/issues/10),
  where it will move into the modem service.
- The battery gauge is voltage-derived: the percentage is an estimate, not a
  coulomb count.
- The kernel suspends (s2idle) and wakes on the RTC. Nothing suspends the
  phone on its own yet; the idle policy belongs to Denial.
- Microphone capture is mono and still partial: the RT5514P DSP loads and
  streams, but the samples carry a DC offset.
