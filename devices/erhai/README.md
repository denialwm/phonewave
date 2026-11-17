# OnePlus Pad 3 (erhai)

The tablet runs Arch Linux on linux-perpetua from slot A, from the image every
phone shares (`image/`), on `userdata`: the tablet has no Android data to
keep. boot_a holds the kernel and after it the tail: modules, the Wi-Fi,
touch and GPU firmware, and boot/tail/. init_boot_a holds the init_boot every
phone shares (`image/init-boot`). `ssh erhai-usb` reaches
the tablet as root over USB at 10.77.71.2, `ssh erhai` as logix over Wi-Fi.

The USB address is the image's, not the device's: interface `usb0` at
10.77.71.2, MAC `02:` + the first four bytes of md5("erhai"), so
`02:af:54:95:4d:...`. The old per-device USB files (erhai-usb's network,
link and drop-in, 02:66:09:00:15:02 at 10.77.74.2) are gone with the old
rootfs.

Private inputs live in the vault, `${PHONEWAVE_VAULT:-device-vault}/erhai`
(next to phonewave): boot/ the built images; firmware/tail/ the firmware the
tail carries (SHA256SUMS-pinned, origins in ORIGIN-mainline.txt; the
mainline paths ath12k/, novatek/, qcom/gen80000_* and qcom/sm8750/erhai/ are
what the tail takes, the flat files beside them are the downstream names of
the same stock files); stock/ the stock images and this unit's EDL dump;
backups/ partition copies.

Packaging a kernel release N: `boot/build-boot.sh N [--name NAME]` builds
linux-perpetua's erhai/display branch (a worktree, `PERPETUA`) in its own
object directory (`PERPETUA_OBJ`) unless the release exists, packs the tail
with image/boot-tail and writes the vault's boot/boot-NAME.img over this
unit's stock boot_a. The tail holds every module of the release. The ADSP
and CDSP firmware are not in the tail: they come from the tablet's own modem
partition (`image/stock-firmware`).

## Partitions

Only `boot_a` and `init_boot_a` are written, one at a time. Never
set_active, erase, format, GPT edits or EDL. The tail turns no slot marking
on, so nothing writes the partition table.

The rescue's fastboot writes this slot's `boot` and `init_boot`. Writing from
a running system by PARTUUID needs the GPT captured once: the storage is
UFS, `/dev/sd[a-f]*` (the vault's stock/gpt-sd*.txt), not mmcblk.

## Installing

`image/build` builds the shared image; its `userdata.img` goes on
`userdata`. With boot-NAME.img on boot_a and the shared init_boot on
init_boot_a, a tablet whose `userdata` holds no image stops in the rescue
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

Not in the tail yet. The WCN7860's hci_qca driver takes its RAM patch and NVM from the tail's
`qca/hmtbtfw10.tlv`, `qca/hmtnv10.b02` and `qca/hmtnv10.b03`, the stock
bluetooth partition's `brhbtfw10.tlv` and `brhbtnv10.b02`/`.b03` under the
names the driver asks for (ROM version 0x10).

## Audio

The stock audio tuning is captured from the unit's stock images and
pinned in the vault under `erhai/reference/stock-audio/` (SHA256SUMS-pinned,
with its README); the board ACDB
(`vendor/etc/acdbdata/sun_mtp/MTP_acdb_cal.acdb`) among them. The speaker
amplifier's tuning (`aw882xx_acf.bin`) and the voice call profile
(`services/audio/voicegen/voiceprofile.py` with `audio/voice.toml`, from the
pinned capture `91d231beee9d4a9e35fd38fe5b35a6e3dde4d5fa92d0ab331672a1b0efd9fe63`)
are not in the tail: the erhai/display kernel has neither the amplifier
driver nor a q6apm voice backend to load them. The stock speaker backend is
TDM-LPAIF-RX-SECONDARY (TDM1, MI2S-Secondary) at 48 kHz / 32 bit /
8 slots; which slot drives which AW882xx amplifier is the first-boot
measurement.

## Touch

The NT36536 on SPI4 takes its firmware from the tail's
`novatek/erhai/novatek_nt36536_fw.bin`, the vendor's `0x2XN242CN00`
(embedded in its device tree) byte for byte.

## GPU

The Adreno 830 takes `qcom/gen80000_{sqe.fw,gmu.bin,aqe.fw}` and the board's
zap shader `qcom/sm8750/erhai/gen80000_zap.mbn` from the tail, copies of this
unit's vendor partition's /vendor/firmware. The vendor partition is a logical
partition in super, which image/stock-firmware cannot mount.

## Charging

The battery is read through the ADSP (qcom_battmgr, pmic_glink). The
erhai/display kernel has no oplus_chg and no CPS8601 driver, so the pen
charger's firmware (`wireless_pen/24976/cps8601_firmware.bin` in the vault)
is not in the tail.

## Clock

No RTC in the device tree: the kernel keeps time and timesyncd corrects it;
there is no rtc-offset to carry.

## Needs the maintainer

- Bluetooth (hci_qca patch and NVM in the tail) once the kernel has it.
