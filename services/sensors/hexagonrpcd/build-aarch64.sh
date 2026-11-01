#!/bin/sh
# Cross-builds hexagonrpcd for the phones: upstream at $commit with the
# patches beside this script, installed for /usr/local into BUILD/stage.
# SOURCE is a clone of linux-msm/hexagonrpc; it is only read.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
commit=da7a3742ab8d5273a281b70ee11f16fb2cb6a690
src=${1:?usage: build-aarch64.sh SOURCE BUILD}
out=${2:?usage: build-aarch64.sh SOURCE BUILD}

mkdir -p "$out"
rm -rf "$out/src" "$out/stage"
mkdir "$out/src"
git -C "$src" archive "$commit" | tar x -C "$out/src"
for patch in "$here"/*.patch; do
    patch -d "$out/src" -p1 -s <"$patch"
done
if [ -d "$out/build" ]; then
    meson setup --reconfigure "$out/build" "$out/src" >/dev/null
else
    meson setup --cross-file "$here/cross-aarch64.ini" --prefix /usr/local \
        --buildtype release "$out/build" "$out/src" >/dev/null
fi
ninja -C "$out/build" -j "$(($(nproc) - 2))"
DESTDIR="$out/stage" meson install -C "$out/build" >/dev/null
