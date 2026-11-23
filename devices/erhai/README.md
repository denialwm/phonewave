# OnePlus Pad 3 (erhai)

The tablet runs Arch Linux on linux-perpetua from slot B, from the image every
phone shares (`image/`), on `userdata`: the tablet has no Android data to
keep. boot_b holds the kernel and after it the tail: modules, the Wi-Fi,
touch and GPU firmware, and boot/tail/. init_boot_b holds the init_boot every
phone shares (`image/init-boot`). `ssh erhai-usb` reaches
the tablet as root over USB at 10.77.71.2, `ssh erhai` as logix over Wi-Fi.

The USB address is the image's, not the device's: interface `usb0` at
10.77.71.2, MAC `02:` + the first four bytes of md5("erhai"), so
`02:af:54:95:4d:...`.

Private inputs live in the vault, `${PHONEWAVE_VAULT:-device-vault}/erhai`
(next to phonewave): boot/ the built images; firmware/tail/ the firmware the
tail carries (SHA256SUMS-pinned, origins in ORIGIN-mainline.txt; the
mainline paths ath12k/, novatek/, qca/gng*, qcom/gen80000_* and qcom/sm8750/erhai/ are
what the tail takes, the flat files beside them are the downstream names of
the same stock files); stock/ the stock images and this unit's EDL dump;
backups/ partition copies.

Packaging a kernel release N: `boot/build-boot.sh N [--name NAME]` builds
it through linux-perpetua-build unless the release exists, and writes the
vault's boot/boot-NAME.img. The ADSP and CDSP firmware are not in the tail:
they come from the tablet's own modem partition (`image/stock-firmware`).

## Partitions

Only `boot_b` and `init_boot_b` are written, one at a time. Never
set_active, erase, format, GPT edits or EDL. The tail turns no slot marking
on, so nothing writes the partition table.

Write an image with `dd` from the running system: first check that
`androidboot.slot_suffix` in `/proc/bootconfig` is `_b`, and write by
PARTUUID: `boot_b` `2dd5f7f7-053f-d973-0b6e-03cbf09c249c` and
`init_boot_b` `cff2ee10-7a6a-ba99-16d2-7b79b7b67285`. Read the partition
back and compare hashes. The rescue's fastboot writes this slot's
`boot` and `init_boot`. The GPT was captured once: the storage is UFS,
`/dev/sd[a-f]*` (the vault's stock/gpt-sde-20261112.txt), not mmcblk.

## Installing

`image/build` builds the shared image; its `userdata.img` goes on
`userdata`. With boot-NAME.img on boot_b and the shared init_boot on
init_boot_b, a tablet whose `userdata` holds no image stops in the rescue
(SSH at 10.77.71.2, fastboot), and from there:

```sh
fastboot flash ssh-key ~/.ssh/id_ed25519.pub
fastboot flash userdata userdata.img
fastboot reboot
```

`first-user --zone TIME_ZONE` makes the account when the tablet next starts;
the flashed key becomes the account's SSH key. Publish with
`image/publish HOST erhai=/path/boot-NAME.img`.

## Wi-Fi

The WCN7860 (PCI 17cb:110e) runs on ath12k, which asks for
`ath12k/WCN7860/hw2.0/{amss,aux_ucode,m3,regdb,board}.bin` and `qdss.cfg`.
The tail carries the stock modem partition's `image/peach/` files under
those names: `amss20.bin`, `aux_ucode20.elf`, `phy_ucode20.elf` (m3),
`regdb.bin`, `bdwlan.elf` (board) and `qdss_trace_config_v2.cfg`, release
WLAN.GNG.1.0-03342-...-1.81206.153.

## Bluetooth

The WCN7860 (hci_qca on UART14, powered through the pwrseq of the PMU
node) takes its RAM patch `qca/gngbtfw20.mbn` and NVM `qca/gngbtnv20.bin`
(ROM version 0x200) from the tail. The tail carries the whole `qca/gng*`
set of the vendor partition's /vendor/bt_firmware/image/, 67 files; the
`qca/hmt*` files beside them in the vault are older and unused.

## Audio

The erhai kernel has the microphones, the speakers and the USB-C
headset: the sound card OnePlus-Pad3 with a capture link (WCD9395 on
TX_CODEC_DMA_TX_3), a speaker link (the six AW88261 amplifiers on
SECONDARY_TDM_RX_0, gpio121-124, the stock speaker back end
TDM-LPAIF-RX-SECONDARY: 8 slots of 32 bits at 48 kHz, each amplifier on
the slot of its channel 0-3, 5, 6) and a playback link to the WCD9395's
HPH outputs (RX_CODEC_DMA_RX_0) for the USB-C headset, and the ADSP
topology `qcom/sm8750/OnePlus-Pad3-tplg.bin`, built from
`audio/topology/OnePlus-Pad3-tdm.m4`:

```
T=devices/erhai/audio/topology
m4 -I $T $T/OnePlus-Pad3-tdm.m4 > OnePlus-Pad3.conf
alsatplg -c OnePlus-Pad3.conf -o OnePlus-Pad3-tplg.bin
```

hw,0 is MultiMedia1 capture (microphones), hw,1 MultiMedia2 playback
(speakers, stereo), hw,2 MultiMedia3 playback (the USB-C headset,
stereo). The amplifiers load the stock tuning
(`qcom/sm8750/erhai/aw88261_acf.bin`, the vendor's `aw882xx_acf.bin`).
The speaker graph is the stock graph's without the vendor's Awinic
modules: SAL limiter, MFC with the stock channel mixer table (left on the
even slots, right on the odd ones), TDM sink on data lane SD1 (gpio124,
the stock graph's lane mask 0x2). The stream gain is capped at 8192 (0
dB). Speaker route: `SECONDARY_TDM_RX_0 Audio Mixer MultiMedia2` on;
level `stream1.vol_ctrl1 MultiMedia2 Playback Volume` and the
amplifiers' `Speaker ChN PCM Playback Volume` (driver default 270 = -22.5
dB, the ACF Music level 350 = -2.5 dB). The frame sync is not in the
stock calibration: the topology's default (frame-long, not inverted, no
data delay) follows the amplifiers' tuning; `-D TDM_SYNC_MODE=`,
`TDM_INVERT=`, `TDM_DELAY=` change it. The microphones have the UCM
profile's capture devices: the main microphone (AMIC5 on ADC4, feeding
both decimators, offered as a mono source), the DMIC2 + DMIC3 endfire
pair as a raw stereo capture, the DMIC0 digital mic, and the USB-C
headset's microphone (AMIC2) (`devices/erhai/audio/README.md`); the card
has no `Voice Call` verb: the tablet has no modem, and the kernel has no
q6apm voice backend to load one.

The stock audio tuning is captured from the unit's stock images and
pinned in the vault under `erhai/reference/stock-audio/` (SHA256SUMS-pinned,
with its README); the board ACDB
(`vendor/etc/acdbdata/sun_mtp/MTP_acdb_cal.acdb`) among them. The stock's
voice call profile is not in the tail: the kernel has no q6apm voice
backend to load it. The stock speaker backend is
TDM-LPAIF-RX-SECONDARY (TDM1, MI2S-Secondary) at 48 kHz / 32 bit / 8
slots; each amplifier's ACF slot is its sound channel.

## Touch

The NT36536 on SPI4 takes its firmware from the tail's
`novatek/erhai/novatek_nt36536_fw.bin`, the vendor's `0x2XN242CN00`
(embedded in its device tree) byte for byte.

## GPU

The Adreno 830 takes `qcom/gen80000_{sqe.fw,gmu.bin,aqe.fw}` and the board's
zap shader `qcom/sm8750/erhai/gen80000_zap.mbn` from the tail, copies of this
unit's vendor partition's /vendor/firmware. The vendor partition is a logical
partition in super, which image/stock-firmware cannot mount.

## Video

Iris decodes H.264, HEVC, VP9 and AV1 on `/dev/video0`; its encoder is
`/dev/video1`. It loads `qcom/sm8750/erhai/vpu35_4v.mbn` from the tail:
vendor's `firmware/vpu35_4v.mbn`, the file the vendor driver loads on
SM8750. OnePlus signs it, so linux-firmware's `vpu35_p4.mbn` does not load
here. Like the GPU's files it is a copy of this unit's vendor partition.

Firefox decodes H.264 and HEVC on Iris with `ffmpeg-denial`
(`image/recipes`) in place of Arch's ffmpeg, as on the Pad 4.

## Charging

The battery is read through the ADSP (qcom_battmgr, pmic_glink):
the kernel's qcom_battmgr driver with the OnePlus SM8750 firmware
support carries the stock charger firmware's data. The kernel drives
the SuperVOOC stage itself (sc8547_svooc, the board's stock charge
strategy from the device tree: the 80 W session and the top up after
a full one) and the stylus charger (cps8601, from the pogo dock). The
pen charger's firmware (`wireless_pen/24976/cps8601_firmware.bin` in
the vault) is not in the tail.

## Sensors

steinerd offers orientation, acceleration, rotation rate, compass heading,
light and proximity from the ADSP's sensor core: bmi3x0 accelerometer and
gyroscope, mxg4300 magnetometer, stk3b6x light and proximity, bu52053nvx
hall, as odm's `etc/sensor/config` lists them (`json_list`). The tail
turns on `hexagonrpcd.service` (services/sensors/hexagonrpcd), which
serves the ADSP its configuration from the booted slot's stock vendor and
odm and from persist (`sensors/registry/registry`), and attaches the
sensors PD (stock: `sscrpcd sensorspd adsp`). The kernel side is
mainline: the ADSP's FastRPC node with its compute banks is in sm8750.dtsi
(`/dev/fastrpc-adsp-secure`, banks 3-8 on the stock SIDs), the in-kernel
PD mapper lists `msm/adsp/sensor_pd` for SM8750, and the ADSP firmware is
the stock one from the modem partition.

The tail's `steinerd/top` says `+x`: the panel scans out landscape
(3392x2400) while the stock tablet's natural orientation is portrait,
`ro.surface_flinger.primary_display_orientation` ORIENTATION_270, so the
scan-out top edge is the core's +x, as on the Pad 4. Derived from the
stock orientation, not yet read on the tablet (Needs the maintainer).

## Haptics

None: the tablet has no vibration motor. Its PMIH0108's HAP530 block is
turned off in every erhai device tree (`oplus-misc-24924.dtsi`, and the
running Lineage tree), so the kernel describes no haptics and hapticd
finds no device.

## Clock

No RTC in the device tree: the kernel keeps time and timesyncd corrects it;
there is no rtc-offset to carry.

## Needs the maintainer

The `steinerd/top` derivation in the Sensors section: confirm it with
`steinerctl raw accel 25` while the tablet stands upright as the screen's
own picture (unrotated, landscape) — about +9.8 on the axis keeps the
tail's `+x`, about -9.8 takes `-x`.
