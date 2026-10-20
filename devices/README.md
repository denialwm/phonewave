# Devices

One directory per device, named after the board in the kernel device tree
(`compatible = "oneplus,aston"` is `aston`).

```
<codename>/
├── device.toml   hardware facts
└── rootfs/       files installed on the device, at the same path
```

`device.toml` records what the hardware is, not how it is configured. Keep it
to facts you can point to in the device tree, a driver, or the stock firmware.

`rootfs/` holds udev rules, systemd units, modprobe options and service
configuration for that device. Use the packaged locations (`/usr/lib/...`)
rather than `/etc`, which stays the administrator's.

A new device needs only `device.toml`. Add `rootfs/` when it has files to
install.
