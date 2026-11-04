# Cross-build settings for the phones (Arch Linux ARM, aarch64). Sourced by
# the recipes. Links against the full Arch Linux ARM sysroot, the libraries
# the image ships, so no newer glibc symbols from this workstation leak in:
# gcc searches its own /usr/aarch64-linux-gnu before --sysroot, and -B puts
# the sysroot first. -B names usr/lib, not the lib symlink: ld maps the
# absolute paths in glibc's linker scripts into the sysroot only for scripts
# it found under the sysroot's real path.
# PHONEWAVE_SYSROOT names another copy, as on the package builder.
sysroot=${PHONEWAVE_SYSROOT:-/mnt/development/denial-os/cross/alarm-sysroot}
export TARGET=aarch64-unknown-linux-gnu
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=--sysroot=$sysroot -C link-arg=-B$sysroot/usr/lib -C link-arg=-Wl,-rpath-link,$sysroot/usr/lib"
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
export CFLAGS_aarch64_unknown_linux_gnu="--sysroot=$sysroot"
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_SYSROOT_DIR=$sysroot
export PKG_CONFIG_LIBDIR=$sysroot/usr/lib/pkgconfig:$sysroot/usr/share/pkgconfig
# Leave two cores to the workstation.
export CARGO_BUILD_JOBS=-2
