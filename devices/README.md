# Devices

One directory per device, named after the board in the kernel device tree
(`compatible = "oneplus,aston"` is `aston`).

```
<codename>/
├── device.toml   hardware facts
├── rootfs/       files installed on the device, at the same path
├── boot/         boot image packaging and init_boot sources
├── packaging/    pacman packages (roadstr)
├── tools/        host scripts: kernel install, reboot checks, Denial deployment
└── README.md     how to package and install for this device
```

`device.toml` records what the hardware is, not how it is configured. Keep it
to facts you can point to in the device tree, a driver, or the stock firmware.

`rootfs/` holds udev rules, systemd units, modprobe options and service
configuration for that device. Use the packaged locations (`/usr/lib/...`)
rather than `/etc`, which stays the administrator's.
The rootfs/ trees imported on 2026-10-20 mirror the devices byte for byte,
so many of their files still sit under `/etc`; they move to `/usr/lib`
when the device is next reinstalled from here.

A new device needs only `device.toml`. Add `rootfs/` when it has files to
install.
