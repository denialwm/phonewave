# hexagonrpcd

linux-msm's [hexagonrpc](https://github.com/linux-msm/hexagonrpc), the
FastRPC server that hands the ADSP's sensor drivers their configuration when
the ADSP starts, built for the phones with Phonewave's patches. steinerd
needs it running from boot on every phone whose sensors live on the ADSP.

## Patches

| Patch | Why |
| --- | --- |
| `0001-rpcd-Serve-the-sensor-configuration-from-odm-too.patch` | Xiaomi's ADSP firmware reads its sensor JSON files from `/odm/etc/sensors/config`. hexagonrpcd serves that from `odm/sensors/config/` under its `-R` directory, and an empty directory where that is missing, so phones without it see no change. |

## Build and install

Needs git, meson, ninja and the `aarch64-linux-gnu` cross compiler; the cross
file links against the workspace sysroot.

```sh
git clone https://github.com/linux-msm/hexagonrpc ~/.cache/phonewave/src/hexagonrpc
./build-aarch64.sh ~/.cache/phonewave/src/hexagonrpc ~/.cache/phonewave/<codename>/hexagonrpc
```

The script exports upstream commit `da7a374` from the clone, applies the
patches and installs into `stage/` under the build directory, for prefix
`/usr/local`. Copy `stage/usr/local/bin/hexagonrpcd` and
`stage/usr/local/lib/libhexagonrpc.so*` to the same places on the phone.
The device's `rootfs/` carries the unit that runs it.

## License

GPL-3.0-or-later, as hexagonrpc.
