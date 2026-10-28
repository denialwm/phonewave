# Motorola Edge 70 (roadstr)

The phone runs Arch Linux on linux-perpetua from slot B. Slot A keeps stock
Android, and the bootloader falls back to it while slot B is not marked
successful. boot_b holds the kernel with its device tree built in; init_boot_b a
busybox and dropbear userspace that mounts the root. rootfs/ holds the device's
own files at their install path, byte for byte as on the phone; packaging/ the
pacman packages; boot/ the boot image scripts; tools/ runs on the host.
`ssh roadstr` reaches the phone as root over USB, `ssh roadstr-wifi` over Wi-Fi.

Private inputs live in the vault, `${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/roadstr`:
stock/ the W1WR36.39-115 firmware, whose boot.img is the boot header template;
boot/ the built images, the initramfs busybox and dropbear, and rollback-6.6/;
firmware/ the GPU, Wi-Fi, Bluetooth and fingerprint firmware and the static
qbootctl that moto70-mainline packages; private-rootfs/ root's authorized_keys,
which init_boot also carries, and sudoers; backups/ partition copies.

Packaging a kernel release N: `boot/build-boot.sh N` builds it through
linux-perpetua-build and writes the vault's boot/boot-N.img, then
`packaging/build.sh linux-moto70-mainline N` packs its modules together with
that image's checksum. Packages land in ~/.cache/phonewave/roadstr/packages.
init_boot rarely changes; `boot/build-init-boot.sh NAME` writes init_boot-NAME.img.

Installing it, by hand: copy the package and boot-N.img to the phone, pacman -U
the package, write the image to /dev/disk/by-partlabel/boot_b and compare its
sha256. `tools/moto70-reboot` then checks that boot_b is the kernel of an
installed module tree and that slot B is marked successful; `--reboot` reboots.
mainline-boot-success.timer marks slot B successful once the new kernel is up.
`boot/parity-check.sh | ssh roadstr sh` prints a read-only health report.
Rollback writes the vault's rollback-6.6 boot_b and init_boot_b images; the 6.6
modules are still installed and vendor_boot_b and dtbo_b were never changed.

Deploying Denial: the phone runs denial-mainline.service from a set under
/var/lib/moto70-denial chosen by the mainline-current symlink.
`tools/moto70-build-denial` builds an AArch64 candidate from a Denial checkout; agents
run it through the workspace's `tools/build start denial roadstr`, which builds the
dev branch into /mnt/development/denial-os/builds/roadstr/denial/latest.
`DENIAL_CANDIDATE=DIR tools/moto70-deploy-denial` stages it beside the running
set, selects it and restarts the service. `tools/moto70-shell-loop` rebuilds and
reloads Dart-only shell changes in place. Neither rolls back by itself; both
print the previous set. They use roadstr-wifi; set MOTO_HOST=roadstr for USB.
tools/fingerprint/ drives the JIIOV sensor, which works only on the 6.6 kernel.
