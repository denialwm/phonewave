#!/bin/sh
# Build one of these packages for the phone (aarch64) on this workstation.
# Usage: build.sh PACKAGE [KERNEL_SUFFIX]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
pkg=${1:?package}
cache=${XDG_CACHE_HOME:-$HOME/.cache}/phonewave/roadstr
[ -n "${2:-}" ] && export MOTO70_KERNEL_SUFFIX=$2
conf=$(mktemp)
trap 'rm -f "$conf"' EXIT
{ cat /etc/makepkg.conf; echo 'CARCH=aarch64'; echo 'CHOST=aarch64-unknown-linux-gnu'
  echo 'PKGEXT=.pkg.tar.zst'; echo "PKGDEST=$cache/packages"
  echo "BUILDDIR=$cache/makepkg"; echo "SRCDEST=$cache/sources"; echo 'BUILDENV=(!distcc color !ccache !check !sign)'; } > "$conf"
mkdir -p "$cache/packages" "$cache/makepkg" "$cache/sources"
cd "$here/$pkg"
makepkg --config "$conf" -f --nodeps --noconfirm
