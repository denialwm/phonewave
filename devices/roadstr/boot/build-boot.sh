#!/bin/zsh
# Build the Moto's own build of linux-perpetua and its boot image: the kernel,
# then the filesystem image/boot-tail packs with its modules, the firmware
# that overrides linux-firmware's and the units in tail/, if any. The boot userspace
# is the init_boot image of build-init-boot.sh; no device access.
# Usage: build-boot.sh N [--name NAME] [BUILD-OPTIONS...]
#   (release <version>-devicesN-roadstr, built unless it exists; writes the
#   vault's boot/boot-NAME.img, NAME defaulting to N)
set -eu -o pipefail
N=${1:?number}
shift
NAME=$N
[[ ${1:-} == --name ]] && { NAME=${2:?name}; shift 2 }
P=/mnt/development/denial-os/linux-perpetua-build
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/roadstr
K=/mnt/development/denial-os/linux-perpetua
HERE=${0:A:h}
# The factory boot image is the template of the header and AVB footer.
TEMPLATE=$V/stock/firmware-W1WR36.39-115/boot.img
OUT=$P/releases/devices-$N-roadstr
# Vault images are never rewritten: a name is used once.
[[ ! -e $V/boot/boot-$NAME.img ]] || { echo "boot-$NAME.img is already in the vault; take a new name" >&2; exit 1 }
echo "403a6078b0130b9413a33985f8b3e0648925762b1e565faa1dba07b7089242a4  $TEMPLATE" | sha256sum -c --quiet
[[ -e $OUT/manifest.json ]] || $P/tools/devices-build roadstr $N "$@"
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' $OUT/manifest.json)

T=$(mktemp -d)
trap 'rm -rf $T' EXIT
mkdir $T/device
[[ ! -d $HERE/tail ]] || cp -a $HERE/tail/. $T/device/
# The board's GPU microcode, Bluetooth patch and Wi-Fi board data, in place
# of linux-firmware's generic copies. Without the board data under the device
# tree's qcom,calibration-variant, ath11k takes the generic entry for board
# ID 255 and the radio fails to authenticate.
F=$V/firmware
install -Dm644 -t $T/device/firmware/qcom $F/qcom/gen71700_sqe.fw $F/qcom/gen71700_gmu.bin
install -Dm644 -t $T/device/firmware/qca $F/bt/*
mkdir -p $T/device/firmware/ath11k/WCN6750/hw1.0
python3 $HERE/ath11k-board2 'bus=ahb,qmi-chip-id=1,qmi-board-id=255,variant=Motorola_roadstr' \
	$F/ath11k/board.bin $F/ath11k/regdb.bin $T/device/firmware/ath11k/WCN6750/hw1.0/board-2.bin
${HERE:h:h:h}/image/boot-tail $OUT $T/device $T/tail.erofs

cd $V/boot
[[ -e kernel-Image-$N ]] || cp $OUT/artifacts/Image kernel-Image-$N
cmp -s kernel-Image-$N $OUT/artifacts/Image || { echo "kernel-Image-$N differs from release $N" >&2; exit 1 }
$K/devices/boot/android-gki boot $TEMPLATE kernel-Image-$N boot-$NAME.img --tail $T/tail.erofs
sha256sum boot-$NAME.img >> SHA256SUMS
echo "built boot-$NAME.img ($REL)"
