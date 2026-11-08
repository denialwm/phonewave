# Shared by the recipes: aarch64 packages built from this checkout. Each
# PKGBUILD sets _src first. The version is the date of the last commit that
# touched _src and the number of commits that did; 0 before the first.
pkgrel=1
arch=(aarch64)
options=(!strip !debug !lto)
_date=$(git -C "$_src" log -1 --format=%cd --date=format:%Y.%m.%d -- .)
pkgver=${_date:-0}.r$(git -C "$_src" rev-list --count HEAD -- .)
# makepkg sets SOURCE_DATE_EPOCH to the time of each run, and Cargo rebuilds a
# crate whose build script reads it (ring, wayland-backend) and everything
# above it. The last commit's time keeps it, and the build cache, the same.
_epoch=$(git -C "$_src" log -1 --format=%ct -- .)
[ -z "$_epoch" ] || export SOURCE_DATE_EPOCH=$_epoch
. "$startdir/../../cross.sh"
