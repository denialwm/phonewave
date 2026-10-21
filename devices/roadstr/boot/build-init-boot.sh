#!/bin/zsh
# Pack the early userspace into an init_boot image for slot B; no device access.
# The ABL decompresses legacy-lz4 ramdisks and passes the vendor_boot ramdisk
# followed by this one, so this /init is the one that runs.
# The initramfs is initramfs/ next to this script (init and etc) plus, from the
# vault, boot/initramfs-bin/{busybox,dropbearmulti} and root's authorized_keys.
# Usage: build-init-boot.sh NAME   (writes the vault's boot/init_boot-NAME.img)
set -eu -o pipefail
N=${1:?name}
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/roadstr
I=$V/boot
umask 022
R=$(mktemp -d)
trap 'rm -rf $R' EXIT
chmod 755 $R
cp -a ${0:A:h}/initramfs/. $R/
chmod 600 $R/etc/shadow
mkdir -p $R/bin $R/dev $R/etc/dropbear $R/newroot $R/proc $R/run $R/sys $R/tmp
install -d -m 700 $R/root $R/root/.ssh
install -m 755 $I/initramfs-bin/busybox $I/initramfs-bin/dropbearmulti $R/bin/
install -m 600 $V/private-rootfs/root/.ssh/authorized_keys $R/root/.ssh/
cd $R
find . | LC_ALL=C sort | cpio -o -H newc -R 0:0 --reproducible --quiet > $I/ramdisk-$N.cpio
lz4 -q -l -12 --favor-decSpeed -f $I/ramdisk-$N.cpio $I/ramdisk-$N.cpio.lz4
mkbootimg --header_version 4 --ramdisk $I/ramdisk-$N.cpio.lz4 -o $I/init_boot-$N.img
avbtool add_hash_footer --image $I/init_boot-$N.img --partition_name init_boot \
	--partition_size 8388608 --algorithm NONE
cd $I
sha256sum init_boot-$N.img ramdisk-$N.cpio >> SHA256SUMS
echo "built init_boot-$N.img"
