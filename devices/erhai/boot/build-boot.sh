#!/bin/zsh
# Build the Pad 3's own build of linux-perpetua and its boot image: the
# kernel, then the filesystem image/boot-tail packs with its modules, the
# firmware that overrides linux-firmware's and tail/. The boot userspace is
# the init_boot image of image/init-boot; no device access.
# Usage: build-boot.sh N [--name NAME] [BUILD-OPTIONS...]
#   (release <version>-devicesN-erhai, built unless it exists; writes the
#   vault's boot/boot-NAME.img, NAME defaulting to N)
set -eu -o pipefail
N=${1:?number}
shift
NAME=$N
[[ ${1:-} == --name ]] && { NAME=${2:?name}; shift 2 }
P=/mnt/development/denial-os/linux-perpetua-build
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/erhai
K=/mnt/development/denial-os/linux-perpetua
HERE=${0:A:h}
# The stock boot image — the last image the phone's own bootloader accepted,
# the template of the header and AVB footer.
TEMPLATE=$V/stock/boot.img
OUT=$P/releases/devices-$N-erhai
# Vault images are never rewritten: a name is used once.
[[ ! -e $V/boot/boot-$NAME.img ]] || { echo "boot-$NAME.img is already in the vault; take a new name" >&2; exit 1 }
echo "09ad50df7d75d20012e6bfedc20d82423589dc6e05b7ccf432e04c0eef949523  $TEMPLATE" | sha256sum -c --quiet
[[ -e $OUT/manifest.json ]] || $P/tools/devices-build erhai $N "$@"
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' $OUT/manifest.json)

T=$(mktemp -d)
trap 'rm -rf $T' EXIT
mkdir $T/device
cp -a $HERE/tail/. $T/device/
# The Wi-Fi firmware and board data (WCN7850: the cnss2 peach driver
# requests them under peach/), the Bluetooth patch and NVM, the speaker
# amplifier tuning, the touch panel firmware, the wireless charger's
# firmware, and the GPU microcode, laid out as the tail's firmware/. The
# ADSP and CDSP firmware comes from the phone's own modem partition
# (tail/stock-firmware.conf).
(cd $V/firmware/tail && sha256sum -c --quiet SHA256SUMS)
mkdir $T/device/firmware $T/device/firmware/peach
(cd $V/firmware/tail && find amss.bin amss20.bin aux_ucode.elf aux_ucode20.elf \
	phy_ucode.elf phy_ucode20.elf regdb.bin bdwlan.e* -type f -print0 | \
	cpio -0 -pdm --quiet $T/device/firmware/peach)
(cd $V/firmware/tail && find aw882xx_acf.bin 0x2XN242CN00 qca qcom wireless_pen \
	-type f -print0 | cpio -0 -pdm --quiet $T/device/firmware)
${HERE:h:h:h}/image/boot-tail $OUT $T/device $T/tail.erofs

cd $V/boot
[[ -e kernel-Image-$N ]] || cp $OUT/artifacts/Image kernel-Image-$N
cmp -s kernel-Image-$N $OUT/artifacts/Image || { echo "kernel-Image-$N differs from release $N" >&2; exit 1 }
$K/devices/boot/android-gki boot $TEMPLATE kernel-Image-$N boot-$NAME.img --tail $T/tail.erofs
sha256sum boot-$NAME.img >> SHA256SUMS
echo "built boot-$NAME.img ($REL)"
