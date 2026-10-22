# Phonewave

Userspace companion to [linux-perpetua](https://github.com/denialwm/linux-perpetua).
It holds the services and per-device configuration that let Denial drive
phone and tablet hardware. The kernel tree stays kernel-only; whatever a device
needs in userspace lives here.

Named after Future Gadget #8 from Steins;Gate, a microwave wired to a phone.

<p align="center">
  <img src=".github/phonewave.webp" width="640" alt="Synthia, chin in her hands, watching a banana turn into a green gel banana">
  <br>
  <em>Phonewave with Synthia.</em>
</p>

## Layout

| Path | Contents |
|---|---|
| `devices/<codename>/` | Everything specific to one device. The codename is the board name in the kernel device tree. |
| `devices/<codename>/device.toml` | Hardware facts: model, SoC, DTB, display, touch, haptics. |
| `devices/<codename>/rootfs/` | Files installed on that device, at the same path. `rootfs/usr/lib/udev/rules.d/x.rules` installs to `/usr/lib/udev/rules.d/x.rules`. |
| `devices/<codename>/boot/` | Packs a linux-perpetua release into the device's boot images, and the init_boot sources. |
| `devices/<codename>/packaging/` | Pacman packages for the device, where it has them. |
| `devices/<codename>/tools/` | Host scripts: install a kernel release, reboot checks, Denial deployment. They reach the device as `ssh <codename>`. |
| `services/<capability>/<name>/` | Daemons and drivers for one hardware capability: haptics, fingerprint, clock. Each is a self-contained project. |

Programs go under `services/`; the configuration that ties them to one device
goes under `devices/`. Example: hapticd drives any Linux vibration motor, so it
lives in `services/haptics/hapticd/`. The udev rule that hands the Ace 3's
PM8550B actuator to hapticd names a chip, so it lives in
`devices/aston/rootfs/usr/lib/udev/rules.d/`.

Apps and their libraries are not part of Phonewave; they live in
[denial-apps](https://github.com/denialwm/denial-apps).

## Devices

| Codename | Device | SoC |
|---|---|---|
| `aston` | OnePlus Ace 3 (OnePlus 12R) | SM8550 |
| `roadstr` | Motorola Edge 70 | Eliza |
| `iceland` | OnePlus Pad 4 | Kaanapali (SM8850) |

## Services

| Capability | Name | Purpose | Devices |
|---|---|---|---|
| haptics | [`hapticd`](services/haptics/hapticd) | D-Bus service for evdev force-feedback and sysfs vibrators | any |
| clock | [`rtc-offset`](services/clock/rtc-offset) | Keeps the system clock across reboots when the RTC cannot be set | any |
| memory | [`zram-swap`](services/memory/zram-swap) | Compressed swap in RAM, sized from the installed memory | any |
| radio | [`radio-addr`](services/radio/radio-addr) | Wi-Fi and Bluetooth addresses from the SoC serial, for bootloaders that pass none | `aston` |
| wallpaper | [`wallpaper-depth`](services/wallpaper/wallpaper-depth) | Cuts the wallpaper's subject out so the lock screen clock sits behind it | any with Vulkan |
| fingerprint | [`jiiov`](services/fingerprint/jiiov) | JIIOV JV0307 optical fingerprint | `roadstr` |
| fingerprint | [`uff`](services/fingerprint/uff) | OPLUS UFF (Goodix G7s) optical fingerprint | `aston` |
| modem | [`qmi-tools`](services/modem/qmi-tools) | Small QMI clients for cellular bring-up | `aston` |
| audio | [`callaudio`](services/audio/callaudio) | Puts the sound card on its voice call profile while the modem has a call; call volume and mute | any with a `Voice Call` UCM verb |
| audio | [`voicegen`](services/audio/voicegen) | Builds a voice call profile for the kernel from the stock ACDB, on the workstation | Qualcomm AudioReach |

## Install

Device files:

```sh
sudo make install-device DEVICE=aston
```

This installs the services every device runs (`zram-swap`), then the
device's rootfs/, whose links enable them. `DESTDIR` installs into a mounted
root filesystem instead. A new device gets the same: its
`rootfs/usr/lib/systemd/system/swap.target.wants/zram-swap.service` links to
`../zram-swap.service`.

Each other service builds from its own directory; see each README.

Stock firmware, partition backups, firmware blobs and the boot images a
release is packed on are not public. The scripts read them from
`$PHONEWAVE_VAULT/<codename>` (default `/mnt/development/denial-os/device-vault`).

hapticd is a `git subtree`; pull updates with
`--prefix=services/haptics/hapticd`.

## License

MIT. See [LICENSE](LICENSE). Most of the fingerprint drivers keep their
upstream licenses (LGPL-2.1-or-later or GPL-2.0-only); see
[services/fingerprint](services/fingerprint/README.md#licenses).
