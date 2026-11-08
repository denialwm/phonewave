#!/bin/zsh
# Build the RedMagic's own build of linux-perpetua and its boot image: the
# kernel, then the filesystem image/boot-tail packs with its modules, the
# firmware that overrides linux-firmware's and tail/. The boot userspace is
# the init_boot image of image/init-boot; no device access.
# Usage: build-boot.sh N [--name NAME] [BUILD-OPTIONS...]
#   (release <version>-devicesN-nx809j, built unless it exists; writes the
#   vault's boot/boot-NAME.img, NAME defaulting to N)
set -eu -o pipefail
N=${1:?number}
shift
NAME=$N
[[ ${1:-} == --name ]] && { NAME=${2:?name}; shift 2 }
P=/mnt/development/denial-os/linux-perpetua-build
V=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/nx809j
K=/mnt/development/denial-os/linux-perpetua
HERE=${0:A:h}
# The devices231 boot image, the last the old root booted, is the template of
# the header and AVB footer.
TEMPLATE=$V/boot/boot_b-devices231.img
OUT=$P/releases/devices-$N-nx809j
# Vault images are never rewritten: a name is used once.
[[ ! -e $V/boot/boot-$NAME.img ]] || { echo "boot-$NAME.img is already in the vault; take a new name" >&2; exit 1 }
echo "17c1d805c346fb06089498e8dc2b0232bd3ea638024b588f66a4b2e68e90182c  $TEMPLATE" | sha256sum -c --quiet
[[ -e $OUT/manifest.json ]] || $P/tools/devices-build nx809j $N "$@"
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' $OUT/manifest.json)

T=$(mktemp -d)
trap 'rm -rf $T' EXIT
mkdir $T/device
cp -a $HERE/tail/. $T/device/
# The GPU microcode, the board's zap shader and speaker tuning (Android's
# vendor holds them, where no early mount reaches), the audio topology,
# Bluetooth patch and NVM, Wi-Fi firmware and board data, and the fingerprint
# sensor's signed app, laid out as the tail's firmware/. The ADSP, CDSP, modem
# and IPA firmware come from the phone's own modem partition
# (tail/stock-firmware.conf).
(cd $V/firmware/tail && sha256sum -c --quiet SHA256SUMS)
mkdir $T/device/firmware
(cd $V/firmware/tail && find qcom/gen80200_* qcom/kaanapali/Nubia-NX809J-tplg.bin qcom/kaanapali/nubia/nx809j-vendor \
	ath12k qca goodix -type f -print0 | cpio -0 -pdm --quiet $T/device/firmware)
# The voice call profile, made from the stock ACDB as audio/voice.toml says;
# the card reads it when it probes, beside the topology.
python3 ${HERE:h:h:h}/services/audio/voicegen/voiceprofile.py $HERE/../audio/voice.toml \
	$T/device/firmware/qcom/kaanapali/Nubia-NX809J-voice.bin
${HERE:h:h:h}/image/boot-tail $OUT $T/device $T/tail.erofs

cd $V/boot
[[ -e kernel-Image-$N ]] || cp $OUT/artifacts/Image kernel-Image-$N
cmp -s kernel-Image-$N $OUT/artifacts/Image || { echo "kernel-Image-$N differs from release $N" >&2; exit 1 }
$K/devices/boot/android-gki boot $TEMPLATE kernel-Image-$N boot-$NAME.img --tail $T/tail.erofs
sha256sum boot-$NAME.img >> SHA256SUMS
echo "built boot-$NAME.img ($REL)"
