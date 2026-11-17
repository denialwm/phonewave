#!/bin/zsh
# Build the Pad 3's own build of linux-perpetua and its boot image: the
# kernel, then the filesystem image/boot-tail packs with its modules, the
# firmware that overrides linux-firmware's and tail/. The boot userspace is
# the init_boot image of image/init-boot; no device access.
# Usage: build-boot.sh N [--name NAME] [BUILD-OPTIONS...]
#   (release <version>-devicesN-erhai, built unless it exists; writes the
#   vault's boot/boot-NAME.img, NAME defaulting to N)
#
# The Pad 3's kernel is the erhai kernel of linux-perpetua (the erhai
# commits against the perpetua release), built from a worktree of it
# into an object directory of its own. Where
# linux-perpetua-build has no tools/devices-build, the release is built here
# in the same layout (linux-perpetua devices/README, "Build output").
#   PERPETUA        the source tree (default: the workspace's wt/erhai-pw)
#   PERPETUA_OBJ    its object directory (default: build-erhai-pw)
#   PERPETUA_BUILD  the folder holding releases/ (default: linux-perpetua-build)
#   PHONEWAVE_VAULT the vault (default: the workspace's device-vault)
set -eu -o pipefail
N=${1:?number}
shift
NAME=$N
[[ ${1:-} == --name ]] && { NAME=${2:?name}; shift 2 }
HERE=${0:A:h}
WS=${HERE:h:h:h:h}
P=${PERPETUA_BUILD:-$WS/linux-perpetua-build}
V=${PHONEWAVE_VAULT:-$WS/device-vault}/erhai
K=${PERPETUA:-$WS/wt/erhai-pw}
O=${PERPETUA_OBJ:-$WS/build-erhai-pw}
# This unit's own stock boot_a, the template of the header and AVB
# footer, checked against the vault's stock/SHA256SUMS.
TEMPLATE=$V/stock/boot_a-template.img
OUT=$P/releases/devices-$N-erhai
# Vault images are never rewritten: a name is used once.
[[ ! -e $V/boot/boot-$NAME.img ]] || { echo "boot-$NAME.img is already in the vault; take a new name" >&2; exit 1 }
(cd $V/stock && sha256sum -c --quiet SHA256SUMS)

if [[ ! -e $OUT/manifest.json && -x $P/tools/devices-build ]]; then
	$P/tools/devices-build erhai $N "$@"
elif [[ ! -e $OUT/manifest.json ]]; then
	git -C $K diff --quiet HEAD || { echo "$K has uncommitted changes" >&2; exit 1 }
	mk=(make -s -C $K ARCH=arm64 LLVM=1 O=$O LOCALVERSION=-devices$N-erhai)
	(cd $K && ARCH=arm64 LLVM=1 devices/config erhai $O)
	$mk -j$(nproc) "$@" Image modules qcom/sm8750-oneplus-erhai.dtb
	rel=$(<$O/include/config/kernel.release)
	mkdir -p $OUT/artifacts
	$mk INSTALL_MOD_PATH=$OUT/modules-root INSTALL_MOD_STRIP=1 modules_install
	rm -f $OUT/modules-root/lib/modules/$rel/build
	cp $O/arch/arm64/boot/Image $OUT/artifacts/Image
	cp $O/arch/arm64/boot/dts/qcom/sm8750-oneplus-erhai.dtb $OUT/artifacts/erhai.dtb
	cp $O/.config $OUT/artifacts/kernel.config
	cp $O/System.map $O/Module.symvers $OUT/artifacts/
	(cd $OUT/modules-root/lib/modules/$rel && find . -name '*.ko*' | sort | xargs sha256sum) \
		> $OUT/artifacts/modules.sha256
	python3 - $OUT erhai $rel $(git -C $K rev-parse HEAD) <<'EOF'
import hashlib, json, sys
from pathlib import Path
out, device, release, source = Path(sys.argv[1]), *sys.argv[2:]
names = ['Image', f'{device}.dtb', 'kernel.config', 'System.map', 'Module.symvers', 'modules.sha256']
artifacts = {n: hashlib.sha256((out / 'artifacts' / n).read_bytes()).hexdigest() for n in names}
(out / 'manifest.json').write_text(json.dumps({'device': device, 'release': release,
	'source': source, 'artifacts': artifacts}, indent=2) + '\n')
EOF
fi
REL=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["release"])' $OUT/manifest.json)

T=$(mktemp -d)
trap 'rm -rf $T' EXIT
mkdir $T/device
cp -a $HERE/tail/. $T/device/
# The Wi-Fi firmware and board data of the WCN7860 (ath12k), its Bluetooth
# RAM patch and NVM (hci_qca, qca/gng*), the touch panel's firmware, and
# the GPU microcode and the board's zap shader, the speaker amplifiers'
# tuning and the ADSP's audio topology, laid
# out as the tail's firmware/, at the names the kernel asks for. The
# topology is built from audio/topology/OnePlus-Pad3-tdm.m4; the rest are
# copies of this unit's stock files (the vault's firmware/tail/
# ORIGIN-mainline.txt): the Wi-Fi set from the modem partition's
# image/peach/ under other names, the Bluetooth and GPU files from the
# vendor partition in super, so image/stock-firmware, which binds one folder of one partition
# as it is, cannot provide them. The ADSP and CDSP firmware comes from the
# tablet's own modem partition (tail/stock-firmware.conf).
(cd $V/firmware/tail && sha256sum -c --quiet SHA256SUMS)
mkdir $T/device/firmware
(cd $V/firmware/tail && find ath12k/WCN7860/hw2.0 novatek/erhai qcom/sm8750/erhai \
	qcom/gen80000_sqe.fw qcom/gen80000_gmu.bin qcom/gen80000_aqe.fw qca/gng* \
	qcom/sm8750/OnePlus-Pad3-tplg.bin \
	-type f -print0 | cpio -0 -pdm --quiet $T/device/firmware)
${HERE:h:h:h}/image/boot-tail $OUT $T/device $T/tail.erofs

mkdir -p $V/boot
cd $V/boot
[[ -e kernel-Image-$N ]] || cp $OUT/artifacts/Image kernel-Image-$N
cmp -s kernel-Image-$N $OUT/artifacts/Image || { echo "kernel-Image-$N differs from release $N" >&2; exit 1 }
$K/devices/boot/android-gki boot $TEMPLATE kernel-Image-$N boot-$NAME.img --tail $T/tail.erofs
sha256sum boot-$NAME.img >> SHA256SUMS
echo "built boot-$NAME.img ($REL)"
