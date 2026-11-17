# OnePlus Pad 3 (erhai)

The tablet runs Arch Linux on linux-perpetua from slot B, from the image every
phone shares (`image/`), on `userdata`: the tablet has no Android data to
keep. boot_b holds the kernel and after it the tail: modules, the Wi-Fi,
Bluetooth, touch, GPU and speaker firmware, and boot/tail/. init_boot_b holds
the init_boot every phone shares (`image/init-boot`). `ssh erhai-usb` reaches
the tablet as root over USB at 10.77.71.2, `ssh erhai` as logix over Wi-Fi.

The USB address is the image's, not the device's: interface `usb0` at
10.77.71.2, MAC `02:` + the first four bytes of md5("erhai"), so
`02:af:54:95:4d:...`. The old per-device USB files (erhai-usb's network,
link and drop-in, 02:66:09:00:15:02 at 10.77.74.2) are gone with the old
rootfs.

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/erhai`:
boot/ the built images; firmware/tail/ the firmware the tail carries, laid
out as its firmware/ (SHA256SUMS-pinned); stock/ the stock boot and init_boot
images; backups/ partition copies; private-rootfs/ root's authorized_keys,
which init_boot also carries, and sudoers.

Packaging a kernel release N: `boot/build-boot.sh N [--name NAME]` builds it
through linux-perpetua-build unless the release exists, and writes the
vault's boot/boot-NAME.img. The ADSP and CDSP firmware are not in the tail:
they come from the tablet's own modem partition (`image/stock-firmware`).

## Partitions

Only `boot_b` and `init_boot_b` are written, one at a time. Never
set_active, erase, format, GPT edits or EDL. The tail turns no slot marking
on, so nothing writes the partition table.

The rescue's fastboot writes this slot's `boot` and `init_boot`. Writing from
a running system by PARTUUID needs the GPT captured once (see
FIRST_BOOT.md, step 0): `blkid` lists the PARTUUIDs of `/dev/mmcblk0p*`.

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

The WCN7850's cnss2 driver (the device tree's `qcom,cnss-peach` node,
supported id 0x110E) requests its firmware under the driver's `peach/`
path prefix — the node's `qcom,same-dt-multi-dev` property turns the
driver's prefix on — so the tail stages the stock modem partition's
`image/peach/` files under the tail's firmware/peach/: `amss.bin`/
`amss20.bin` with their aux and PHY ucodes, `regdb.bin`, and the split
board data `bdwlan.elf` plus `bdwlan.eXX`, one per board id, the driver
picking the one the chip reports.

## Bluetooth

The WCN7850's hci_qca driver takes its RAM patch and NVM from the tail's
`qca/hmtbtfw10.tlv`, `qca/hmtnv10.b02` and `qca/hmtnv10.b03`, the stock
bluetooth partition's `brhbtfw10.tlv` and `brhbtnv10.b02`/`.b03` under the
names the driver asks for (ROM version 0x10).

## Audio

The sun ASoC machine loads no topology or ACDB on the host: the sound
configuration lives in the DSP-side factory firmware. The tail carries the
speaker amplifier's tuning, `aw882xx_acf.bin`. If tuning the voice call
profile is ever needed, the nx809j's pattern applies: generate it from the
stock ACDB (in the stock vendor image under `/etc/acdbdata/sun_mtp/`) with
`services/audio/voicegen/voiceprofile.py`.

## Touch

The NT36536 on SPI4 takes its firmware, staged as `0x2XN242CN00` — the
exact name the driver asks for — from the vendor's own device tree, which
embeds the panel firmware in-DT; it is byte-identical to what the vendor
ships.

## Charging

SuperVOOC runs through the in-tree oplus_chg driver; its module option for a
separate pad4-supervooc supervisor is gone, the kernel regulates SuperVOOC
itself. The wireless charger's (CPS8601) firmware rides in the tail at
`wireless_pen/24976/cps8601_firmware.bin`.

## Clock

No RTC in the device tree: the kernel keeps time and timesyncd corrects it;
there is no rtc-offset to carry.

## Needs the maintainer

- The `--tail` writer in `devices/boot/android-gki` (see FIRST_BOOT.md,
  MAINTAINER-NEEDED). Until it lands, the trial boot image is assembled by
  hand with the same layout.
- The release's kernel config must be trimmed for the tail to fit the
  96 MiB boot partition: the trial image is 288 MiB, 191 MiB over, the
  214 MiB module set of a distro-wide config against a budget of about
  58 MiB (the RedMagic's field tail fits). Keep cnss2 (the device tree's
  WLAN driver), cfg80211, mac80211 and the lpass chain; drop the other
  Wi-Fi stacks — ath10k, ath11k, ath12k, iwlwifi, mwifiex (the ath12k PCI
  table also claims the chip's supported id 0x110E and would fight cnss2
  for it) — and xfs, bridge, nf_tables.
- The kernel's DTS asks the modem partition for `qcom/sm8750/*.mbn`, but
  this stock partition holds only the split `*.mdt` + `*.bNN` form (verified
  in the stock payload; the vendor's own DTS names them `*.mdt`). The
  stock-firmware bind and the required-file guard use the names that exist;
  if the ADSP or CDSP fails to load, the DTS `firmware-name` entries need
  the `.mdt` names.
