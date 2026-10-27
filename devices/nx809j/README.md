# Nubia RedMagic 11 Pro (nx809j)

Bring-up on the shared devices kernel (linux-perpetua), from slot B. The
phone keeps its stock Android boot chain; the kernel replaces the
bootloader's device tree with its own (FDT handoff).

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/nx809j`:
record.md, the add-device record; backups/ this unit's partition copies
(`backups/manifest.json`); survey/ what was read from the phone; ssh/ the
rescue host key once one exists.

## Partitions

Only `boot_b` and `init_boot_b` are written, one at a time. Never write
an `_a` partition, never switch the active slot, and never write vbmeta,
dtbo, vendor_boot, bootloader, firmware or calibration partitions. Slot A
is marked unbootable by the bootloader; if slot B ever becomes unbootable
too, ask the user before any slot command. The rollback for each partition
is its stock copy in `backups/slot_b/`.

The stock ABL answers `fastboot flash` and `fastboot boot` with "unknown
command", and fastbootd reports the device locked and refuses downloads.
Images are written with `dd` from a running system: rooted Android, or
the rescue. Before each write, check the image against the packaging
manifest and that the running slot is `_b`; read the partition back and
compare hashes afterwards. If nothing boots, the partitions are restored
over EDL.

Slot B's boot attributes live in the GPT entry of `boot_b` on `sde`. This
bootloader leaves the successful bit set on every boot, so rescue boots
do not use up retries.

## Packaging

Build linux-perpetua through linux-perpetua-build
(`tools/devices-build nx809j N`), then:

```sh
boot/package.py RELEASE_DIR \
  --busybox STATIC_ARM64_BUSYBOX --dropbear STATIC_ARM64_DROPBEARMULTI \
  --authorized-keys ~/.ssh/id_ed25519.pub
```

It writes `boot_b.img` (the kernel) and `init_boot_b.img` (the rescue
ramdisk) to `~/.cache/phonewave/nx809j/rescue`, through Perpetua's
`android-gki` adapter with this unit's stock images as templates, and
records hashes in `manifest.json`. The ramdisk is LZ4 legacy: the
bootloader appends it to the vendor ramdisk, which is LZ4 legacy too and
has no end marker, so the kernel would read any other format as more LZ4
chunks and lose the rescue. `--root-partuuid UUID` makes the
ramdisk start an installed ext4 root from that GPT partition; without it
the phone stays in the RAM rescue. It never formats anything.

## RAM rescue

SSH as root on 10.77.73.2 over USB ECM; the phone's DHCP server gives the
host 10.77.73.1 with no gateway or DNS. An ACM serial console runs on the
same gadget. Without a host key in the vault, dropbear makes one on the
first connection; copy it to `ssh/dropbear_ed25519_host_key` in the vault
so later images keep it.

## Root filesystem

Arch Linux ARM on a partition of its own, following Perpetua's
`devices/arch-linux-arm.md`. Android keeps `userdata`: its f2fs was shrunk
online from rooted Android (`F2FS_IOC_RESIZE_FS`) and the end of `sda`
became a partition named `linux`, GPT PARTUUID
`2225dabe-3937-4455-83a4-58a737b38f2f`, ext4 labelled `ALARM`. The vault
keeps the partition tables from before and after (`backups/gpt*`). Android
still boots from slot B's stock images; formatting `userdata` from Android
is its own recovery and leaves `linux` alone.

Package with `--root-partuuid 2225dabe-3937-4455-83a4-58a737b38f2f` to boot
the installed root; without it the phone stays in the RAM rescue.

`rootfs/` installs the ECM+ACM gadget service and a networkd profile matched
by the device-side MAC, on the rescue's link: the phone is 10.77.73.2 and
leases 10.77.73.1 to the host, without a router, DNS or NTP. Enable `sshd`
and `systemd-networkd`; root and `logix` log in with keys only. Install the
exact release's module tree under `/usr/lib/modules`.
