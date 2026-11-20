#!/bin/zsh
# Pack the Tab S7 FE's boot image from a built linux-perpetua tree: kernel,
# the shared early userspace as its ramdisk (the tablet has no init_boot),
# and after the DTB the filesystem image/boot-tail would pack: modules and
# tail/. No device access.
# Usage: build-boot.sh INIT_BOOT OUTPUT
#   INIT_BOOT  an init_boot.img from image/init-boot (or a published image):
#              its busybox, dropbearmulti, fastboot and key-held are kept,
#              /init and etc/ come from this checkout's image/initramfs.
# Environment: KSRC (linux-perpetua checkout), KBUILD (its O= build, already
# built), TEMPLATE (the tablet's stock boot.img: header, DTB, signer trailer,
# AVB footer), DEV_KEY (a public key the rescue's SSH lets in; optional),
# FIRMWARE (a folder with tsp_focaltech/ft8203_gts7xllite.bin, the touch
# firmware: Samsung builds it into the stock kernel, so the tablet has no copy
# once this boot replaces it; tools/pull-touch-firmware.sh in the port
# workspace takes it from Samsung's kernel source).
set -eu -o pipefail
IB=${1:?init_boot.img}
OUT=${2:?output boot.img}
HERE=${0:A:h}
PW=${HERE:h:h:h}
W=${PW:h}
KSRC=${KSRC:-$W/linux-perpetua-SM736B}
KBUILD=${KBUILD:-$W/build-gts7xllite}
TEMPLATE=${TEMPLATE:-$W/samsung/img/boot.img}
FIRMWARE=${FIRMWARE:-$W/proprietary/touch}
TOUCH_FW=tsp_focaltech/ft8203_gts7xllite.bin
# Without it the tablet boots with no touch: refuse.
[[ -f $FIRMWARE/$TOUCH_FW ]] || { echo "no $FIRMWARE/$TOUCH_FW: set FIRMWARE" >&2; exit 1 }
[[ ! -e $OUT ]] || { echo "$OUT exists" >&2; exit 1 }
REL=$(<$KBUILD/include/config/kernel.release)

T=$(mktemp -d)
trap 'rm -rf $T' EXIT
# Tail: one folder named after the release, mounted at /usr/lib/modules.
make -s -C $KSRC O=$KBUILD ARCH=arm64 LLVM=1 INSTALL_MOD_PATH=$T/m INSTALL_MOD_STRIP=1 modules_install
mkdir -m 755 $T/tail
mv $T/m/lib/modules/$REL $T/tail/
rm -f $T/tail/$REL/{build,source}
install -m644 $KBUILD/.config $T/tail/$REL/config
install -m644 $KBUILD/System.map $T/tail/$REL/System.map
cp -a $HERE/tail/. $T/tail/$REL/
install -Dm644 $FIRMWARE/$TOUCH_FW $T/tail/$REL/firmware/$TOUCH_FW
# The checkout may sit on a filesystem without modes (NTFS): set them.
find $T/tail/$REL -type d -exec chmod 755 {} + -o -type f -exec chmod 644 {} +
mkfs.erofs --quiet -b4096 --all-root -T0 $T/tail.erofs $T/tail >/dev/null

# Ramdisk: the shared init_boot's, with this checkout's init, as gzip.
unpack_bootimg --boot_img $IB --out $T/ib >/dev/null
mkdir $T/rd
(cd $T/rd && lz4 -dc $T/ib/ramdisk | cpio -id --quiet)
cp -a $PW/image/initramfs/. $T/rd/
chmod 755 $T/rd/init
chmod 600 $T/rd/etc/shadow
if [[ -n ${DEV_KEY:-} ]]; then
	install -d -m700 $T/rd/root/.ssh
	install -m600 $DEV_KEY $T/rd/root/.ssh/authorized_keys
fi
(cd $T/rd && find . | LC_ALL=C sort | cpio -o -H newc -R 0:0 --reproducible --quiet) | gzip -9n > $T/ramdisk.gz

python3 $KSRC/devices/boot/android-v2 $TEMPLATE $KBUILD/arch/arm64/boot/Image $T/ramdisk.gz $OUT --tail $T/tail.erofs >/dev/null
echo "built $OUT ($REL, tail $(( $(stat -c %s $T/tail.erofs) >> 20 )) MiB)"
