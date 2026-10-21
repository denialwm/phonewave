# zram-swap

Compressed swap in RAM for phones and tablets, which have no swap partition.
Memory the kernel would otherwise reclaim by killing a process goes to a zram
device instead, compressed with zstd, typically to a third of its size.

## How it works

`zram-swap start`, run by `zram-swap.service` before `swap.target`:

1. Loads the zram module if needed and takes `zram0`, or a new zram device
   when something else already set `zram0` up.
2. Chooses zstd when the kernel has it, then sizes the device to half of
   `MemTotal`. The size counts uncompressed data, so half of memory as swap
   takes about a sixth of memory when full.
3. Formats it and turns it on with priority 100 and discard, so freed swap
   returns its memory at once.
4. Tunes the VM for swap that costs a compression instead of a disk read:
   `swappiness` 180 (swap before dropping file cache), `page-cluster` 0 (no
   readahead), `watermark_boost_factor` 0 and `watermark_scale_factor` 125
   (wake kswapd earlier instead of boosting watermarks).

It does nothing when a zram swap is already on. `zram-swap stop` turns the
zram swaps off and resets their devices; the VM tuning stays until reboot.

Settings, in a drop-in for `zram-swap.service`:

| Variable | Default | Meaning |
|---|---|---|
| `ZRAM_SWAP_PERCENT` | 50 | Device size as a share of memory, 1 to 200 |
| `ZRAM_SWAP_ALGORITHM` | zstd | Compression algorithm, if the kernel has it |

The kernel needs `CONFIG_ZRAM` (a module is fine) and, for zstd,
`CONFIG_ZRAM_BACKEND_ZSTD`.

## Install

```sh
make check
sudo make install
```

`make install-device` at the top of Phonewave installs it on every device.
Each device enables it in its rootfs with the link
`usr/lib/systemd/system/swap.target.wants/zram-swap.service ->
../zram-swap.service`; elsewhere:

```sh
sudo systemctl enable zram-swap.service
```

On Qualcomm devices, the device rootfs also keeps the GPU's buffers out of
swap (`msm.enable_eviction=N`): the Pad 4 had its first GPU lockups in the
first boot with swap. The cause is not proven; it is a mitigation.
