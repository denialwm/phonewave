# Lenovo Idea Tab Pro (tb373fu)

MediaTek MT6897 (Dimensity 8300, labelled MT8792), Mali-G615, 8 GB RAM,
256 GB UFS, Wi-Fi only. Perpetua's first MediaTek device; the port has not
booted yet. The tablet runs the reference kernel,
`6.1.138-tb373fu-syncobj1`, a Motorola MediaTek 6.1 tree with our Panthor
and DRM syncobj backports, and Arch Linux ARM from `userdata`.

Private backups, the bootloader survey, the device record and the port plan
live in `$PHONEWAVE_VAULT/tb373fu` (default
`/mnt/development/denial-os/device-vault`). The reference source is in
`baseline-kernels/tb373fu/mtk-6.1`.

## Access

`ssh tb373fu` reaches the tablet over USB at 10.0.0.2;
`ssh tb373fu-wifi` over Wi-Fi while the reference kernel runs.

## Flashing

- The tablet runs from slot `_b` and stays there. Never run
  `fastboot --set-active`; never write an `_a` partition.
- The bootloader ignores the A/B data in `misc`. Never write `misc`.
- Test kernels go to `boot_b` only, written with `dd` over SSH and read
  back to compare hashes. `fastboot boot` is unsupported, and
  `fastboot flash` runs at about 320 KB/s (64 MiB takes 3.5 minutes).
- Never write `vbmeta*`, `dtbo*`, `vendor_boot_a`, `init_boot*`, bootloader
  or firmware partitions while porting. `vendor_boot_b` is written only
  as part of a full reference-kernel release.
- The reference kernel's `boot_b` image stays at hand in the vault; it is
  where the tablet charges until a perpetua build drives the charger.

## Fastboot

From Linux on the reference kernel, the reboot-mode driver has to be loaded
by hand, because the rootfs module allowlist blocks it:

```sh
M=/usr/lib/modules/$(uname -r)/kernel/drivers/power/reset
insmod $M/reboot-mode.ko
insmod $M/syscon-reboot-mode.ko
systemctl reboot --no-block --reboot-argument=bootloader
```

Fastboot shows up on the host after about 15 seconds (`0e8d:201c`);
`fastboot reboot` returns SSH after about 30 seconds.

## Recovery

If a `boot_b` image does not come back:

1. Hold Vol-Down + Power until fastboot appears.
2. `fastboot flash boot_b` with the vault's `backups/slot_b/boot_b.img`.
3. `fastboot reboot`.

The bootloader may fall back to slot A on its own; that is not relied on.

## Boot chain

Android boot header v4. `boot` holds only the kernel (legacy LZ4).
`vendor_boot` holds the ramdisk, the DTB (in a DT table) and
`bootopt=64S3,32N2,64N2`; `dtbo` holds one overlay. `boot` carries two AVB
layers: an inner GKI vbmeta whose content hash the bootloader enforces even
with verification disabled in `vbmeta_b`, and an outer Lenovo one.
