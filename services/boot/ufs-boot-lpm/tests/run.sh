#!/bin/sh
# Runs ufs-boot-lpm hold and release against a fake sysfs with one UFS host
# and one platform device that is not one.
set -eu

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

export UFS_BOOT_LPM_SYSFS="$work/sys" UFS_BOOT_LPM_STATE="$work/run/ufs-boot-lpm"
ufs="$work/sys/bus/platform/devices/1d84000.ufs"
other="$work/sys/bus/platform/devices/a600000.usb"

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok   $1"
    else
        echo "FAIL $1: expected '$3', got '$2'"
        failures=$((failures + 1))
    fi
}

mkdir -p "$ufs" "$other" "$work/run"
echo 1 >"$ufs/clkscale_enable"
echo 1 >"$ufs/clkgate_enable"
echo 150000 >"$ufs/auto_hibern8"
echo 1 >"$other/clkgate_enable"

sh "$root/ufs-boot-lpm" hold
check "hold clears clock scaling" "$(cat "$ufs/clkscale_enable")" 0
check "hold clears clock gating" "$(cat "$ufs/clkgate_enable")" 0
check "hold clears auto-hibern8" "$(cat "$ufs/auto_hibern8")" 0
check "hold leaves other devices" "$(cat "$other/clkgate_enable")" 1

# A second hold keeps the first values.
sh "$root/ufs-boot-lpm" hold 2>/dev/null
sh "$root/ufs-boot-lpm" release
check "release restores clock scaling" "$(cat "$ufs/clkscale_enable")" 1
check "release restores clock gating" "$(cat "$ufs/clkgate_enable")" 1
check "release restores auto-hibern8" "$(cat "$ufs/auto_hibern8")" 150000
check "release drops its state" "$([ -e "$UFS_BOOT_LPM_STATE" ] && echo kept || echo gone)" gone

# Release without hold changes nothing.
echo 5000 >"$ufs/auto_hibern8"
sh "$root/ufs-boot-lpm" release
check "release without hold" "$(cat "$ufs/auto_hibern8")" 5000

# A host without clock scaling (no attribute) is still held and released.
rm "$ufs/clkscale_enable"
sh "$root/ufs-boot-lpm" hold
check "hold without clkscale_enable" "$(cat "$ufs/clkgate_enable")" 0
sh "$root/ufs-boot-lpm" release
check "release without clkscale_enable" "$(cat "$ufs/auto_hibern8"):$(cat "$ufs/clkgate_enable")" 5000:1

[ "$failures" -eq 0 ]
