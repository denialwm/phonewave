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
# The ADSP and the WLAN firmware the modem boots live in vendor_a inside
# super, where no early mount reaches. The vault holds the phone's own copies
# and their hashes; the boot tail puts them where the kernel and tqftpserv
# look. The modem's own files do not come through here: they stay in the
# booted slot's modem partition and image/stock-firmware binds them over
# firmware/qcom/sm7250/google/barbet/modem.
VENDOR_FW=$V/firmware/tail
[ -d "$VENDOR_FW" ] || { echo "$VENDOR_FW is missing" >&2; exit 1; }
(cd "$VENDOR_FW" && sha256sum -c --quiet SHA256SUMS)
FW=$T/device/firmware
ADSP=$FW/qcom/sm7250/google/barbet/adsp
ATH=$FW/ath10k/WCN3990/hw1.0
mkdir -p "$ADSP" "$ATH/barbet"
install -m644 "$VENDOR_FW/adsp.mdt" "$VENDOR_FW"/adsp.b* "$ADSP/"
# tqftpserv scans every remoteproc's firmware folder, so the WLAN firmware
# the modem asks for is found beside the ADSP's files.
install -m644 "$VENDOR_FW/wlanmdsp.mbn" "$ADSP/wlanmdsp.mbn"
install -m644 "$VENDOR_FW/wlanmdsp.mbn" "$ADSP/wlanmdsp.otaupdate"
install -m644 "$VENDOR_FW/bdwlan-barbet.bin" "$ADSP/bdwlan.bin"
# ath10k's board file, built from the phone's own calibration, under the
# board name the device tree sets. ath10k asks for hw1.0/barbet/<file> first
# and falls back to hw1.0/<file>, where the image's linux-firmware copy of
# firmware-5.bin serves.
python3 - "$VENDOR_FW/bdwlan-barbet.bin" "$ATH/barbet/board-2.bin" <<'PY'
import struct
import sys

src, out = sys.argv[1], sys.argv[2]
data = open(src, "rb").read()


def pad4(blob):
    return blob + b"\0" * (-len(blob) % 4)


def ie(kind, value):
    return struct.pack("<II", kind, len(value)) + pad4(value)


blob = pad4(b"QCA-ATH10K-BOARD\0")
name = b"bus=snoc,qmi-board-id=ff"
blob += ie(0, ie(0, name) + ie(1, data))
open(out, "wb").write(blob)
PY
install -m644 "$VENDOR_FW/bdwlan-barbet.bin" "$ATH/barbet/board.bin"
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
