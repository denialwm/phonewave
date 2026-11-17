#!/usr/bin/env bash
# Build the Phone (2)'s boot image: the kernel, the early userspace, then the
# filesystem image/boot-tail packs with the kernel's modules, the firmware
# that overrides linux-firmware's and tail/. The phone has no init_boot
# partition, so the ramdisk image/init-boot packs rides in boot, behind the
# kernel. devices/boot/android-gki in linux-perpetua writes the image from the
# phone's own boot image; no device access.
# Usage: build-boot.sh RELEASE OUTPUT [INIT-BOOT-OPTIONS...]
#   RELEASE  a linux-perpetua-build release for pong (linux-perpetua's
#            devices/README, "Build output")
# PHONEWAVE_VAULT names the vault, PHONEWAVE_KERNEL the linux-perpetua source.
set -euo pipefail
release=${1:?usage: build-boot.sh RELEASE OUTPUT [INIT-BOOT-OPTIONS...]}
out=${2:?usage: build-boot.sh RELEASE OUTPUT [INIT-BOOT-OPTIONS...]}
shift 2
here=$(cd "$(dirname "$0")" && pwd)
top=$here/../../..
vault=${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/pong
kernel=${PHONEWAVE_KERNEL:-/mnt/development/denial-os/linux-perpetua}
# The phone's own boot image is the template of the header and AVB footer.
template=$vault/stock/boot.img
echo "1815971fc90a6fc8f9b6437e76daa6cca1b72b0c7b4e38c3752d0ba5972cd316  $template" | sha256sum -c --quiet
[ "$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["device"])' "$release/manifest.json")" = pong ] ||
  { echo "$release is not a pong release" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
device=$work/device
mkdir "$device"
cp -a "$here/tail/." "$device/"
# The GPU's microcode and zap shader, which the phone's own vendor partition
# holds and which are not public, laid out as the tail's firmware/. Without
# them the GPU does not start: stop.
fw=$vault/firmware
[ -d "$fw/tail" ] || { echo "the GPU firmware is missing from $fw/tail" >&2; exit 1; }
(cd "$fw/tail" && sha256sum -c --quiet ../SHA256SUMS)
mkdir -p "$device/firmware"
cp -a "$fw/tail/." "$device/firmware/"
# The speakers' topology, built from ../audio/Nothing-Phone-2.m4 with the macros
# of linux-msm's audioreach-topology (a checkout of
# github.com/linux-msm/audioreach-topology); the sound card does not start
# without it. The amplifiers have no speaker protection: the stream volume is
# capped at -6 dB (4096; 8192 is 0 dB) in the generated file, so no build
# lacks the cap. alsatplg reports the DAI widgets the kernel adds as undefined.
macros=$HOME/.cache/phonewave/src/audioreach-topology/src
(cd "$here/../audio" && m4 -I "$macros" Nothing-Phone-2.m4) | sed 's/max "65535"/max "4096"/' > "$work/Nothing-Phone-2.conf"
grep -q 'max "4096"' "$work/Nothing-Phone-2.conf" || { echo "the topology's volume cap did not apply" >&2; exit 1; }
mkdir -p "$device/firmware/qcom/sm8450"
alsatplg -c "$work/Nothing-Phone-2.conf" -o "$device/firmware/qcom/sm8450/Nothing-Phone-2-tplg.bin" 2>/dev/null
"$top/image/boot-tail" "$release" "$device" "$work/tail.erofs"
"$top/image/init-boot" "$work/ramdisk.lz4" --ramdisk "$@"
mkdir -p "$(dirname "$out")"
"$kernel/devices/boot/android-gki" boot "$template" "$release/artifacts/Image" "$out" \
  --ramdisk "$work/ramdisk.lz4" --tail "$work/tail.erofs"
