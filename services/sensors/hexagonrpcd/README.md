# hexagonrpcd

linux-msm's [hexagonrpc](https://github.com/linux-msm/hexagonrpc), the
FastRPC server that hands the ADSP's sensor drivers their configuration when
the ADSP starts, built for the phones with Phonewave's patches. steinerd
needs it running from boot on every phone whose sensors live on the ADSP.

## Patches

| Patch | Why |
| --- | --- |
| `0001-rpcd-Serve-the-sensor-configuration-from-odm-too.patch` | Xiaomi's ADSP firmware reads its sensor JSON files from `/odm/etc/sensors/config`. hexagonrpcd serves that from `odm/sensors/config/` under its `-R` directory, and an empty directory where that is missing, so phones without it see no change. |
| `0002-rpcd-Serve-odm-etc-sensor-config-too.patch` | OnePlus's ADSP firmware reads them from `/odm/etc/sensor/config`, without the s: served from `odm/sensor/config/` the same way. |

## On the phones

`image/recipes/hexagonrpcd` builds upstream `da7a374` with the patches. A
phone whose sensors live on the ADSP turns on `hexagonrpcd.service` from its
boot image's tail. Before each start, `hexagonrpcd-stock-sensors.service`
runs `stock-sensors`, which copies what the ADSP reads from the phone's own
partitions, read-only, into `/run/hexagonrpcd`, the folder hexagonrpcd
serves: the booted slot's vendor (`etc/sensors/config`,
`etc/sensors/sns_reg_config`) and odm (`etc/sensors/config` or
`etc/sensor/config`, where the phone has one) inside super, and persist (`sensors/registry/registry`), read where
it is already mounted, as under the fingerprint secure storage. The files
are the phone's own and never enter this repo. The ADSP reads them only when
it starts.

## License

GPL-3.0-or-later, as hexagonrpc.
