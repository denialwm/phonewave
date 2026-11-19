#!/bin/sh
# Build the Pixel 5a's own build of linux-perpetua and its boot image: the
# kernel, then the filesystem image/boot-tail packs with its modules, the
# firmware that overrides linux-firmware's and tail/. The boot userspace is
# the legacy-lz4 ramdisk image/init-boot writes for a phone that keeps it in
# boot; no device access.
# Usage: build-boot.sh N [--name NAME] [BUILD-OPTIONS...]
#   (release <version>-devicesN-barbet, built unless it exists; writes the
#   vault's boot/boot-NAME.img, NAME defaulting to N)
set -eu
N=${1:?number}
shift
NAME=$N
if [ "${1:-}" = --name ]; then
	NAME=${2:?name}
	shift 2
fi
P=/mnt/development/denial-os/linux-perpetua-build
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/barbet
K=/mnt/development/denial-os/linux-perpetua
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# The verified working image is the template of the version 2 header, the
# AVB footer and the DTB placement.
TEMPLATE=$V/backups/boot_b.img
OUT=$P/releases/devices-$N-barbet
# Vault images are never rewritten: a name is used once.
[ ! -e "$V/boot/boot-$NAME.img" ] ||
	{ echo "boot-$NAME.img is already in the vault; take a new name" >&2; exit 1; }
[ -e "$TEMPLATE" ] || { echo "$TEMPLATE is missing" >&2; exit 1; }
[ -e "$OUT/manifest.json" ] || $P/tools/devices-build barbet "$N" "$@"
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' "$OUT/manifest.json")

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir "$T/device"
[ ! -d "$HERE/tail" ] || cp -a "$HERE/tail/." "$T/device/"
"$HERE/../../../image/boot-tail" "$OUT" "$T/device" "$T/tail.erofs"
# The phone keeps its ramdisk in boot, so the shared early userspace is
# written as the legacy-lz4 ramdisk image/boot-tail does not carry.
"$HERE/../../../image/init-boot" "$T/ramdisk.lz4" --ramdisk

mkdir -p "$V/boot"
cd "$V/boot"
[ -e "kernel-Image-$N" ] || cp "$OUT/artifacts/Image" "kernel-Image-$N"
cmp -s "kernel-Image-$N" "$OUT/artifacts/Image" ||
	{ echo "kernel-Image-$N differs from release $N" >&2; exit 1; }
# The kernel carries the Barbet tree it must boot, so --dtb replaces the
# template's Google tree. The tail follows the DTB, where image/initramfs/init
# looks for it.
"$K/devices/boot/android-v2" "$TEMPLATE" "kernel-Image-$N" "$T/ramdisk.lz4" \
	"boot-$NAME.img" --dtb "$OUT/artifacts/barbet.dtb" --tail "$T/tail.erofs"
sha256sum "boot-$NAME.img" >>SHA256SUMS
echo "built boot-$NAME.img ($REL)"
