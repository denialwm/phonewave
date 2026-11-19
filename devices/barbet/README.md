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

## Firmware

The phone keeps its firmware in two places. The modem's files are in the
booted slot's `modem` partition (vfat, `image/`), the same files the modem
itself runs. The ADSP and the WLAN firmware the modem boots are in `vendor_a`
inside `super`, which no early mount reaches.

`boot/tail/stock-firmware.conf` names the modem partition, its folder and
the kernel's `.../modem/` target; `image/stock-firmware` mounts it read-only
and binds it there. Its `stock-firmware.sha256` is the tested release's
hashes. The rest comes from the vault at image build time, because those
files have no partition the early userspace can mount. `build-boot.sh`
checks the vault's `SHA256SUMS`, lays the files out where the kernel and
tqftpserv look, and builds ath10k's board file from the phone's own
calibration.

| Files | Comes from | In the tail |
| --- | --- | --- |
| `modem.mdt`, `modem.b00`–`modem.b34` | the booted slot's `modem` partition, `image/` | `firmware/qcom/sm7250/google/barbet/modem/`, bound at boot |
| `adsp.mdt`, `adsp.b00`–`adsp.b33` | `vendor_a/firmware/`, through the vault | `firmware/qcom/sm7250/google/barbet/adsp/` |
| `wlanmdsp.mbn`, `wlanmdsp.otaupdate`, `bdwlan.bin` | `vendor_a/firmware/`, through the vault | `firmware/qcom/sm7250/google/barbet/adsp/`, where tqftpserv finds them beside the ADSP's files |
| `bdwlan-barbet.bin` | `vendor_a/firmware/`, through the vault | ath10k's `board.bin` and the generated `board-2.bin` under `ath10k/WCN3990/hw1.0/` and `hw1.0/barbet/` |
| `firmware-5.bin` | generated, the API-5 non-BMI descriptor | the same ath10k folders |

The modem's `image/` folder is 99,411,980 bytes; the vendor set this PR's
tail carries is 29,584,303 bytes (28.2 MiB) in 44 files. The modules and the
tail units are added to that. The `boot_b` partition is 96 MiB (100,663,296
bytes); `android-v2` refuses an image that does not fit.

The ADSP stays offline at boot because the kernel's `qcom,sm7250-adsp-pas`
resource sets `.auto_boot = false` (`drivers/remoteproc/qcom_q6v5_pas.c`),
and no unit starts it yet. It must start only after the modem has booted the
WLAN firmware, so the order belongs to the modem service; that is
[issue #10](https://github.com/denialwm/phonewave/issues/10). The same issue
covers Bluetooth, which answers only after the WLAN has powered the combo
chip.

## Notes

- The battery gauge is voltage-derived: the percentage is an estimate, not a
  coulomb count.
- The kernel suspends (s2idle) and wakes on the RTC. Nothing suspends the
  phone on its own yet; the idle policy belongs to Denial.
- Microphone capture is mono and still partial: the RT5514P DSP loads and
  streams, but the samples carry a DC offset.
