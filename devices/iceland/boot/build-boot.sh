#!/bin/zsh
# Build the Pad 4's own build of linux-perpetua and its boot image: the
# kernel, then the filesystem image/boot-tail packs with its modules, the
# firmware that overrides linux-firmware's and tail/. The boot userspace is
# the init_boot image of image/init-boot; no device access.
# Usage: build-boot.sh N [--name NAME] [BUILD-OPTIONS...]
#   (release <version>-devicesN-iceland, built unless it exists; writes the
#   vault's boot/boot-NAME.img, NAME defaulting to N)
set -eu -o pipefail
N=${1:?number}
shift
NAME=$N
[[ ${1:-} == --name ]] && { NAME=${2:?name}; shift 2 }
P=/mnt/development/denial-os/linux-perpetua-build
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/iceland
K=/mnt/development/denial-os/linux-perpetua
HERE=${0:A:h}
# The devices225 boot image, the last the old root booted, is the template of
# the header and AVB footer.
TEMPLATE=$V/boot/devices225/artifacts/boot.img
OUT=$P/releases/devices-$N-iceland
# Vault images are never rewritten: a name is used once.
[[ ! -e $V/boot/boot-$NAME.img ]] || { echo "boot-$NAME.img is already in the vault; take a new name" >&2; exit 1 }
echo "4fc0e208c50a03a3c5092857d241a7fad6443680a99a6c54c44e8343de93d5d0  $TEMPLATE" | sha256sum -c --quiet
[[ -e $OUT/manifest.json ]] || $P/tools/devices-build iceland $N "$@"
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' $OUT/manifest.json)

T=$(mktemp -d)
trap 'rm -rf $T' EXIT
mkdir $T/device
cp -a $HERE/tail/. $T/device/
# The GPU microcode and the board's zap shader, the touchscreen's firmware,
# the audio topology, the Bluetooth and Peri patches and NVM, and the Wi-Fi
# firmware and board data, laid out as the tail's firmware/. The ADSP
# firmware comes from the tablet's own modem partition
# (tail/stock-firmware.conf).
(cd $V/firmware/tail && sha256sum -c --quiet SHA256SUMS)
mkdir $T/device/firmware
(cd $V/firmware/tail && find qcom qca novatek ath12k -type f -print0 | cpio -0 -pdm --quiet $T/device/firmware)
${HERE:h:h:h}/image/boot-tail $OUT $T/device $T/tail.erofs

cd $V/boot
[[ -e kernel-Image-$N ]] || cp $OUT/artifacts/Image kernel-Image-$N
cmp -s kernel-Image-$N $OUT/artifacts/Image || { echo "kernel-Image-$N differs from release $N" >&2; exit 1 }
$K/devices/boot/android-gki boot $TEMPLATE kernel-Image-$N boot-$NAME.img --tail $T/tail.erofs
sha256sum boot-$NAME.img >> SHA256SUMS
echo "built boot-$NAME.img ($REL)"
