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
| udev rule, Phonewave's `85-phone-radio-addr.rules` | `radio-addr wifi IFNAME` when a Wi-Fi interface appears | sets the MAC while the link is still down, when the device tree describes the radio and gives it no `local-mac-address` or `mac-address`; NetworkManager waits for udev, so it only sees this address |
| `radio-addr-bluetooth.service`, wanted by `bluetooth.target` | `radio-addr bluetooth` | listens on the Bluetooth management socket and sets the public address of every controller that is missing one: at boot, and again when a controller is registered anew |

A radio the device tree does not describe, such as a USB adapter, keeps its
own address. bluetoothd ignores unconfigured controllers and takes each one up once its
address is set.

## Install

```sh
make check
sudo make install
```

The tests need `sha256sum`; `make check` also runs `shellcheck`. Phonewave's
image installs it on every phone with the Wi-Fi rule and turns on
`radio-addr-bluetooth.service`, which leaves a controller with an address
alone.
