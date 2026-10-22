# radio-addr

Gives a device's Wi-Fi and Bluetooth radios addresses of their own, for
devices whose bootloader passes none.

Without one, a Qualcomm WCN7850 reports the shared default MAC its firmware
carries, and its Bluetooth controller stays an "Unconfigured controller,
missing public-address" that BlueZ cannot use. Where the bootloader does pass
addresses, linux-perpetua's FDT handoff carries them into the device tree and
this service is not needed.

## Addresses

Each address is the first six bytes of SHA-256 over a label, a NUL byte and
the SoC serial number in `/sys/devices/soc0/serial_number`. The labels are
`radio-addr wifi` and `radio-addr bluetooth`, so the two radios get unrelated
addresses, and a device gets the same ones on every boot and every install.

- Wi-Fi: the first byte is made locally administered and unicast
  (`x2:`, `x6:`, `xa:`, `xe:`), because no vendor prefix is ours.
- Bluetooth: a public address. If its lower half lands on the reserved
  inquiry access codes `9e8b00`-`9e8b3f`, its fourth byte has its lowest bit
  flipped.

`radio-addr print` shows both.

## How it works

| Hook | Runs | Does |
|---|---|---|
| udev rule, from the device's rootfs | `radio-addr wifi IFNAME` when the interface appears | sets the MAC while the link is still down; NetworkManager waits for udev, so it only sees this address |
| `radio-addr-bluetooth.service`, wanted by `bluetooth.target` | `radio-addr bluetooth` | listens on the Bluetooth management socket and sets the public address of every controller that is missing one: at boot, and again when a controller is registered anew |

The Wi-Fi rule names the device's Wi-Fi driver, so it belongs to the device.
bluetoothd ignores unconfigured controllers and takes each one up once its
address is set.

## Install

```sh
make check
sudo make install
```

The tests need `sha256sum`; `make check` also runs `shellcheck`. Enable it on
devices that need it. Phonewave device trees do this under
`devices/<codename>/rootfs/`: the Wi-Fi udev rule and a
`bluetooth.target.wants/radio-addr-bluetooth.service` link.
