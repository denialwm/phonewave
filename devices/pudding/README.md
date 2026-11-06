# Xiaomi 17 (pudding)

Bring-up on the shared devices kernel (linux-perpetua). Android stays on
slot A, Denial goes on slot B. The phone keeps its stock Android boot
chain; the kernel replaces the bootloader's device tree with its own (FDT
handoff).

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/pudding`:
record.md, the add-device record; backups/ this unit's partition copies
(`backups/README.md`, `backups/SHA256SUMS`); survey/ what was read from the
phone; ssh/ the rescue host key.

## Partitions

Only `boot_b` and `init_boot_b` are written, and each write is announced.
Slot A runs Android: write an `_a` partition only when the user asks for
it. Switch the active slot only with the user's go-ahead, each time.
Never write vbmeta, dtbo, vendor_boot, bootloader, firmware or calibration
partitions. The rollback for each partition is its stock copy in
`backups/slot-a-20261101/`; slot B holds the same build, and
`init_boot_a.img` there is the KernelSU image slot A runs.

The ABL is unlocked and accepts `fastboot flash` and `fastboot boot` with
unsigned images. There is no `fastboot fetch`: after a write, read the
partition back from rooted Android or the rescue and compare hashes.
`fastboot set_active a` clears slot A's unbootable flag and resets its
retries.

## Boot slot

Until slot B is marked successful, each boot of it uses one of the
bootloader's retries; with none left, the phone boots Android from slot A.
slot-success (services/boot/slot-success) marks it once Denial and SSH have
run for a minute, as `rootfs/etc/slot-success.conf` sets. A fastboot write
to `boot_b` or `init_boot_b` resets slot B to seven retries and clears the
mark, and boots that stop in the RAM rescue never mark it. Before a run of
test reboots, check the retries left with `slot-success status`.
`fastboot set_active b` brings slot B back, with the user's go-ahead.

## Packaging

Build linux-perpetua through linux-perpetua-build
(`tools/devices-build pudding N`), then:

```sh
boot/package.py RELEASE_DIR \
  --busybox STATIC_ARM64_BUSYBOX --dropbear STATIC_ARM64_DROPBEARMULTI \
  --authorized-keys ~/.ssh/id_ed25519.pub
```

It writes `boot.img` (the kernel) and `init_boot.img` (the rescue
ramdisk) to `~/.cache/phonewave/pudding/rescue`, through Perpetua's
`android-gki` adapter with this unit's stock images as templates, and
records hashes in `manifest.json`. The ramdisk is LZ4 legacy: the
bootloader appends it to the vendor ramdisk, which is LZ4 legacy too and
has no end marker. It also builds `reboot-to` with the workspace cross
compiler. `--root-partuuid UUID` makes the ramdisk start an installed ext4
root from that GPT partition; without it the phone stays in the RAM
rescue. It never formats anything.

The rescue lives in `/pudding-rescue` and the kernel starts it by
`rdinit=/pudding-rescue/load`, which copies it to a tmpfs and switches
into it.

## RAM boot from slot A

`fastboot boot` takes the ramdisk from `init_boot_a`, never from the
uploaded image. To RAM-boot a kernel from slot A, `init_boot_a` carries
the rescue followed by the KernelSU ramdisk:

```sh
boot/package.py RELEASE_DIR ... \
  --android-init-boot VAULT/backups/slot-a-20261101/init_boot_a.img \
  --log-partuuid 90747521-494e-48dc-b5c5-8c47036e7b5d --reboot-after 60
```

The stock kernel still starts KernelSU's `/init`; ours starts the rescue.
`--log-partuuid` writes the kernel log to the start of the `linux`
partition every two seconds, and `--reboot-after` reboots into Android at
that uptime unless `/run/hold` exists. Use both only while `linux` holds
no filesystem.

## RAM rescue

SSH as root on 10.77.74.2 over USB ECM; the phone's DHCP server offers
the host 10.77.74.1. Give the host a NetworkManager profile for the
rescue's MAC, 02:66:09:00:00:01, with the static address 10.77.74.1/30
and no gateway; otherwise a generic profile can take the interface. An
ACM serial console runs on the same gadget. Without a packaged host key,
dropbear makes one each boot.

`reboot-to bootloader` restarts into fastboot, the way back to Android on
slot A without the keys.

## Wi-Fi

The Wi-Fi/BT chip is Qualcomm's WCN7860 ("peach", PCI 17cb:110e) on
PCIe0, run by ath12k behind the WCN7850 power sequencer. Its firmware
comes from the stock modem partition's `image/peach/` and goes to
`/usr/lib/firmware/ath12k/WCN7860/hw2.0/`:

| Stock file | Installed as |
| --- | --- |
| `amss20.bin` | `amss.bin` |
| `aux_ucode20.elf` | `aux_ucode.bin` |
| `phy_ucode20.elf` | `m3.bin` |
| `qdss_trace_config_v2.cfg` | `qdss.cfg` |
| `regdb.bin` | `regdb.bin` |
| `bd_p3gl.elf` | `board.bin` |

`bd_p3gl.elf` is this model's global board data. `bdwlan.elf` beside it is
Qualcomm's reference data, not tuned for this phone.

The Wi-Fi address is `wlan/wlan_mac.bin` on the persist partition. A
`.link` file in `/etc/systemd/network/`, matched on
`Driver=ath12k_wifi7_pci`, sets it on the phone only; without it the
interface reports 00:00:00:00:00:00.

NetworkManager runs Wi-Fi: install `networkmanager`, `wpa_supplicant`,
`wireless-regdb` and `iw`, and enable `NetworkManager`. `rootfs/` keeps it
off `usb0`, which systemd-networkd runs.

The board data starts the firmware in China's rules. Set the phone's country
as `WIRELESS_REGDOM` in `/etc/conf.d/wireless-regdom`: wireless-regdb's udev
rule passes it on when cfg80211 loads, and the kernel (devices-202 on)
hands it to the firmware as the Wi-Fi radio registers.

## Bluetooth

The combo's Bluetooth half runs on hci_qca over SE18's high-speed UART
(`ttyHS1`). Xiaomi's stock `bluetooth_a` carries Brahma firmware rather
than the Ganges files the driver asks for by default, and the controller
reports board 0, so the device tree names the files: the patch
`brhbtfw20.tlv` and board 0x3e's RF settings `brhbtnv20.b3e`.
`tools/stock-files` copies both to `/usr/lib/firmware/qca/`. With the
generic `brhbtnv20.bin` the receiver is some 50 dB deaf.

The controller has no address of its own: install radio-addr
(services/radio/radio-addr, built with the workspace cross compiler), which
`rootfs/` wants from `bluetooth.target`, and BlueZ (`bluez`,
`bluez-utils`), and enable `bluetooth.service`.

## Haptics

The PMIH0108's HAP530 drives the LRA through `qcom-spmi-haptics`
(devices-203 on), with stock's settings: vmax 1250 mV, LRA period 5715 us.
hapticd runs it: the udev rule in `rootfs/` tags the input device as
`builtin`, and `/etc/hapticd/config.json` makes it the default.

## ADSP and sensors

The ADSP's firmware goes to `/usr/lib/firmware/qcom/kaanapali/xiaomi/pudding/`
beside the modem's: `adsp.mbn` and `adsp_dtb.mbn` are joined from `adsp.mdt`,
`adsp_dtb.mdt` and their `.bNN` segments in the stock modem partition's
`image/`, and `adspr.jsn`, `adspua.jsn` and `adsps.jsn` are copied from there
as they are. The firmware spans the stock 85.5 MiB carve-out, which the
device tree reserves.

steinerd (services/sensors/steinerd) runs the sensors: lsm6dsv accelerometer
and gyroscope, qmc6309h magnetometer, Goodix light sensor, proximity and
bmp5 pressure. The ADSP reads its sensor configuration at start from
`pudding-sensors-hexagonrpcd.service`, which runs Phonewave's hexagonrpcd
build (services/sensors/hexagonrpcd) from `/usr/local`: Xiaomi's firmware
reads its JSON files from `/odm/etc/sensors/config`. `tools/stock-files`,
run on the phone as root, copies them and the rest of the configuration from
the stock `vendor_a`, `odm_a` and persist, all read-only, into
`/usr/share/qcom/kaanapali/Xiaomi/pudding`. Enable
`pudding-sensors-hexagonrpcd.service` and `steinerd.service`, then restart
the phone: the ADSP reads the files only when it starts.

## Speakers

Two Cirrus CS35L43 amplifiers sit on the primary MI2S: the top one (`T`,
i2c 0x42) on the left channel, the bottom one (`B`, 0x40) on the right, as
on stock. GPIO 174, the top amplifier's receiver switch, stays low for the
speaker. Each amplifier runs stock's speaker protection on its own DSP:
`cirrus,firmware-uid = "xiaomi-pudding"` makes the driver load
`cirrus/cs35l43-dsp1-spk-prot-xiaomi-pudding.wmfw` and a tuning per
amplifier, `...-xiaomi-pudding-b.bin` and `...-t.bin`. `tools/stock-files`
copies them from the stock `odm_a` (`firmware/cs35l43-dsp1-spk-prot.wmfw`,
`B-` and `T-cs35l43-dsp1-spk-prot.bin`), and the factory calibration from
persist's `audio/crus_calr.bin` to `/var/lib/pudding-speakers/`.

`/usr/lib/phonewave/pudding-speakers` sets the amplifiers up once the sound
card registers (`91-pudding-speakers.rules` starts
`pudding-speakers.service`): it loads the protection firmware, hands each
amplifier its calibration through its `CAL_DATA` control, and unmutes them
only when both read it back. The kernel log then says "Calibration applied"
for each. The amplifiers stay at stock's 0 dB; PipeWire (`pipewire`,
`pipewire-pulse`, `wireplumber`, `pipewire-audio`, `rtkit`) sets the volume
in software, as `51-pudding-audio.conf` asks, through the UCM profile
`kaanapali/Xiaomi-17` in services/audio/ucm.

The ADSP's AudioReach topology is `audio/Xiaomi-17.m4`, built with the
macros of infiniti-mainline's audioreach-topology:

```
L=~/.cache/phonewave/src/infiniti-audioreach-topology
O=~/.cache/phonewave/pudding/audio
m4 -I $L -D SPEAKER_SD_LINE=SD_LINE_IDX_I2S_SD1 audio/Xiaomi-17.m4 > $O/Xiaomi-17.conf
alsatplg -c $O/Xiaomi-17.conf -o $O/Xiaomi-17-tplg.bin
```

It goes to `/usr/lib/firmware/qcom/kaanapali/Xiaomi-17-tplg.bin`.

## Modem

dmail (services/modem/dmail) owns it. rootfs/ carries
`/usr/lib/dmail/modem-setup`, the `pudding-modem-rmtfs` and
`pudding-modem-tqftpserv` units it starts and `pudding-dmail.conf`, which
puts logix in the `dmail` group. The device file is
`boot/tail/dmail/device.toml`, in the boot image's tail; a boot image without
a tail leaves it to `/etc/dmail/device.toml`, a copy on the phone. The data
endpoints are those of Kaanapali's IPA (AP to modem 2, modem to AP 24), as
on the RedMagic 11 Pro.

The firmware goes to `/usr/lib/firmware/qcom/kaanapali/xiaomi/pudding/`.
`modem.mbn` and `modem_dtb.mbn` are joined from `modem.mdt` and its
`.bNN` segments in the stock modemfirmware partition's `image/`, and
`ipa_fws.mbn` from `ipa_fws.mdt` in the modem partition; `modem_pr/`,
`modem_oem/` and `modemr.jsn` are copied from modemfirmware as they are.
`rmtfs`, `tqftpserv` and `qrtr-lookup` live in `/usr/local/bin`.

The EFS copy never enters this repo: on the phone it lives in
`/var/lib/pudding-modem/efs` (`modem_fs1`, `modem_fs2`, `modem_fsc`,
`modem_fsg` and their `SHA256SUMS`, from the stock partitions), and
`efs.handset` beside it holds the SoC serial, written on the phone from
`/sys/devices/soc0/serial_number` and never printed. rmtfs serves it
read-only.

Enable `dmail.service`: it starts the modem at boot through `modem-setup`.
IPA has to be up before the modem starts; its module loads at boot, and
`modem-setup` waits for it and starts no modem without it. Leave a running
modem alone: stopping it, loading IPA and starting it again reset the
phone, as stopping and starting the modem does on the RedMagic 11 Pro.
After a forced reset the modem's files in `/var/lib/tqftpserv` can be left
empty; the modem then stops partway through its start, retrying every 5 s.
Move the empty files aside and it finishes.
