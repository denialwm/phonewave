# Nothing Phone (2) (pong)

Model A065, Qualcomm SM8475 (Snapdragon 8+ Gen 1). The kernel is
linux-perpetua's `pong` device
(`arch/arm64/boot/dts/qcom/sm8475-nothing-pong.dts`). What works and what does
not is in [FEATURES.md](../../FEATURES.md).

## Boot chain

Android 13 on a 5.10 kernel: boot (header version 4, kernel and generic
ramdisk), vendor_boot (vendor ramdisk, device tree, bootconfig), dtbo,
recovery. There is no init_boot partition, so the early userspace every phone
shares rides in boot as its ramdisk; the bootloader passes it after the
vendor_boot one, and the kernel's FDT handoff replaces the stock tree
(`stock-compatible = "qcom,cape"`). The modules, the firmware and `boot/tail/`
follow the ramdisk, in the filesystem `image/boot-tail` packs.

## Build

The vault, `${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/pong`,
holds what is not public:

- `stock/boot.img`, the phone's own boot image, the template of the header and
  the AVB footer;
- `firmware/tail/` with `SHA256SUMS`: the GPU's `a730_sqe.fw`,
  `gmu_gen70000.bin` and `a730_zap.mbn`, laid out as the tail's `firmware/`,
  from the phone's vendor partition. The build stops without them.

The speakers' topology is built from `audio/` with a checkout of
[linux-msm/audioreach-topology](https://github.com/linux-msm/audioreach-topology)
at `~/.cache/phonewave/src/audioreach-topology`.

    boot/build-boot.sh RELEASE boot-pong.img

RELEASE is a linux-perpetua-build release for pong. To try the image without
writing anything to the phone:

    adb reboot bootloader
    fastboot boot boot-pong.img

## Install

The image goes on the boot partition of the slot the bootloader starts, with
`fastboot flash boot boot-pong.img`; the other slot is left alone and the
active slot is never changed. The root and the rest follow `image/README.md`.
`tools/phone-reboot pong` checks that the running slot is marked successful and
that its boot partition carries its kernel's modules before a reboot.

Never write: the other slot's partitions, the partition table, or `persist`,
`modemst1`, `modemst2`, `fsg` and `fsc`, which hold the phone's own
calibration and radio data.

## Notes

- The ADSP, CDSP, SLPI and modem run from the phone's own `modem` partition
  (`boot/tail/stock-firmware.conf`); the GPU's zap shader sits beside them,
  in `qcom/sm8475/pong`.
- Sensors run on the SLPI, whose FastRPC node is `fastrpc-sdsp`:
  `boot/tail/` starts hexagonrpcd on it with the stock configuration, which
  `stock-sensors` takes from the slot that holds the phone's Android vendor.
  The screen's top is the sensors' default `+y`.
- dmail brings the modem up from `boot/tail/dmail/device.toml`. The `[data]`
  values are the OnePlus 12R's; mobile data needs the IPA, which is not
  described yet.
- Audio: two TFA9873 amplifiers on I2C13 take stereo from the tertiary MI2S
  through `audio/`'s topology; the stream volume is capped (see there).
