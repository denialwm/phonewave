#!/bin/zsh
# Build the Pixel 6's own build of linux-perpetua and its boot image: the
# kernel, the early userspace as its ramdisk, then the filesystem
# image/boot-tail packs with its modules, the firmware that overrides
# linux-firmware's and tail/. The Pixel 6 has no init_boot partition, so the
# ramdisk is the one of an init_boot image of image/init-boot, the one
# image/build wrote beside the image the phone runs; no device access.
# Usage: build-boot.sh N INIT_BOOT [--name NAME] [BUILD-OPTIONS...]
#   (release <version>-devicesN-oriole, built unless it exists; writes the
#   vault's boot/boot-NAME.img, NAME defaulting to N)
set -eu -o pipefail
N=${1:?number}
INIT_BOOT=${2:?init_boot image}
shift 2
NAME=$N
[[ ${1:-} == --name ]] && { NAME=${2:?name}; shift 2 }
P=/mnt/development/denial-os/linux-perpetua-build
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/oriole
K=/mnt/development/denial-os/linux-perpetua
HERE=${0:A:h}
# The factory boot image of the tested release (CP1A.260405.005), the
# template of the header and AVB footer, checked against the vault's
# stock/SHA256SUMS.
TEMPLATE=$V/stock/boot.img
OUT=$P/releases/devices-$N-oriole
# Vault images are never rewritten: a name is used once.
[[ ! -e $V/boot/boot-$NAME.img ]] || { echo "boot-$NAME.img is already in the vault; take a new name" >&2; exit 1 }
(cd $V/stock && sha256sum -c --quiet SHA256SUMS)
[[ -e $OUT/manifest.json ]] || $P/tools/devices-build oriole $N "$@"
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' $OUT/manifest.json)

T=$(mktemp -d)
trap 'rm -rf $T' EXIT
unpack_bootimg --boot_img $INIT_BOOT --out $T/init_boot > /dev/null
[[ -s $T/init_boot/ramdisk ]] || { echo "$INIT_BOOT holds no ramdisk" >&2; exit 1 }
mkdir $T/device
cp -a $HERE/tail/. $T/device/
# The Wi-Fi firmware of the BCM4389 (brcmfmac), laid out as the tail's
# firmware/, at the names the kernel asks for: copies of the stock vendor
# image's fw_bcmdhd.bin, bcmdhd_clm.blob and bcmdhd.cal (the vault's
# firmware/tail/), which sit in the vendor partition in super, so
# image/stock-firmware, which binds one folder of one partition as it is,
# cannot provide them.
(cd $V/firmware/tail && sha256sum -c --quiet SHA256SUMS)
mkdir $T/device/firmware
(cd $V/firmware/tail && find brcm -type f -print0 | cpio -0 -pdm --quiet $T/device/firmware)
${HERE:h:h:h}/image/boot-tail $OUT $T/device $T/tail.erofs

mkdir -p $V/boot
cd $V/boot
[[ -e kernel-Image-$N ]] || cp $OUT/artifacts/Image kernel-Image-$N
cmp -s kernel-Image-$N $OUT/artifacts/Image || { echo "kernel-Image-$N differs from release $N" >&2; exit 1 }
# The kernel as legacy LZ4, as the stock image carries it, then the ramdisk
# and the tail, in the page after it.
$K/devices/boot/android-gki boot $TEMPLATE kernel-Image-$N $T/boot.img \
	--lz4 --ramdisk $T/init_boot/ramdisk --tail $T/tail.erofs > /dev/null
mv $T/boot.img boot-$NAME.img
sha256sum boot-$NAME.img >> SHA256SUMS
echo "built boot-$NAME.img ($REL)"
