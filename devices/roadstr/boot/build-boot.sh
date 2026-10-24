#!/bin/zsh
# Build the Moto's own build of linux-perpetua and its boot image (kernel
# only; the boot userspace is the init_boot image of build-init-boot.sh); no
# device access.
# Usage: build-boot.sh N [BUILD-OPTIONS...]   (release <version>-devicesN-roadstr)
set -eu -o pipefail
N=${1:?number}
shift
P=/mnt/development/denial-os/linux-perpetua-build
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/roadstr
K=/mnt/development/denial-os/linux-perpetua
# The factory boot image is the template of the header and AVB footer.
TEMPLATE=$V/stock/firmware-W1WR36.39-115/boot.img
OUT=$P/releases/devices-$N-roadstr
# Vault images are never rewritten: a number is used once.
[[ ! -e $V/boot/boot-$N.img ]] || { echo "boot-$N.img is already in the vault; take a new number" >&2; exit 1 }
echo "403a6078b0130b9413a33985f8b3e0648925762b1e565faa1dba07b7089242a4  $TEMPLATE" | sha256sum -c --quiet
$P/tools/devices-build roadstr $N "$@"
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' $OUT/manifest.json)
cd $V/boot
cp $OUT/artifacts/Image kernel-Image-$N
$K/devices/boot/android-gki boot $TEMPLATE kernel-Image-$N boot-$N.img
sha256sum boot-$N.img kernel-Image-$N >> SHA256SUMS
echo "built boot-$N.img ($REL)"
