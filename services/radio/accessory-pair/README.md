# accessory-pair

Pairs a Bluetooth accessory the moment it is docked, without a prompt, as
the stock Android of a tablet does with its stylus and its keyboard cover.

A dock driver in the kernel learns the accessory's Bluetooth address over
the physical link (the stylus charger's coil, the keyboard's pogo pins) and
reports it as a change uevent:

| Variable | Value |
|---|---|
| `ACCESSORY_BT_ADDRESS` | `c0:87:06:07:fe:68` |
| `ACCESSORY_PRESENT` | `1` docked, `0` undocked |
| `ACCESSORY_TYPE` | `stylus`, `keyboard` (for the log) |

The reporting device also has a `bt_address` attribute while the accessory
is docked, which the service reads when it starts, and may have a writable
`bt_capacity` attribute, where the service writes the accessory's Bluetooth
battery level (`org.bluez.Battery1`) while it is docked: at the dock, when
the level changes and when the accessory connects after it docked. The OnePlus Pad 3
drivers are documented in the kernel's
`Documentation/ABI/testing/sysfs-class-power-cps8601` and
`sysfs-driver-oneplus-pogo-kbd`. Nothing in the service names a device.

## What it does

On a docked accessory it registers a `NoInputNoOutput` agent with BlueZ (not
as the default agent: the desktop's agent keeps every other pairing), scans
LE for the address for up to 30 s, pairs (Just Works), sets `Trusted` and
connects. It tries three more times, 20 s apart, when the accessory does not
show up.

An accessory whose input arrives over the dock while docked (its dock device
has input devices, as the pogo keyboard's has) would type every key twice if
it also stayed connected over Bluetooth. Such an accessory is paired and
then `Blocked` while docked: BlueZ keeps the bond and refuses its
connections. When it is undocked the service unblocks and connects it. The
addresses it blocked are kept in `/var/lib/accessory-pair/blocked`, so a
restart never leaves one blocked. An accessory that could not be paired
while docked (it does not advertise then) is paired when it is undocked.

Any other accessory (the stylus) stays connected, docked or not.

If the dock's address is not found but its byte-reversed form is, the
service uses that and logs that the driver reports the address in the wrong
order.

## Build and install

Requires Rust 1.87+, pkg-config, libudev development files, D-Bus and BlueZ.

```sh
make
make check
sudo make install
```

The udev rule starts the service on the first dock event; a device whose
accessories are docked at boot also links the unit into
`bluetooth.target.wants` from its boot image's tail.

## Logs

`journalctl -u accessory-pair` shows each step: `stylus C0:87:06:07:FE:68
docked`, the planned steps, `found as /org/bluez/hci0/dev_...`, `paired (Just
Works, no prompt)`, `kept off Bluetooth while docked`, `battery 87% ->
.../bt_capacity`.

## Docked double-send test

With `ACCESSORY_PAIR_NO_BLOCK` set (e.g. a unit drop-in carrying
`Environment=ACCESSORY_PAIR_NO_BLOCK=1`), a docked keyboard is not blocked:
its Bluetooth link stays connected while its input arrives over the dock. Use
it to check whether the keyboard sends keys over both paths at once; drop the
switch (and the block logic) if it does not.
