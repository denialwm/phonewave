#!/bin/sh
# Runs zram-swap against a fake sysfs, procfs and /dev, with fake modprobe,
# mkswap, swapon and swapoff.
set -eu

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

export ZRAM_SWAP_SYSFS="$work/sys" ZRAM_SWAP_PROC="$work/proc" ZRAM_SWAP_DEV="$work/dev"
export PATH="$work/bin:$PATH"
mkdir -p "$work/bin"

# swapon and swapoff edit the fake /proc/swaps; every call is logged.
cat >"$work/bin/modprobe" <<EOF
#!/bin/sh
echo "modprobe \$*" >>"$work/log"
mkdir -p "$work/sys/class/zram-control"
EOF
cat >"$work/bin/mkswap" <<EOF
#!/bin/sh
echo "mkswap \$*" >>"$work/log"
EOF
cat >"$work/bin/swapon" <<EOF
#!/bin/sh
echo "swapon \$*" >>"$work/log"
for last; do :; done
echo "\$last partition 1000 0 100" >>"$work/proc/swaps"
EOF
cat >"$work/bin/swapoff" <<EOF
#!/bin/sh
echo "swapoff \$*" >>"$work/log"
grep -v "^\$1 " "$work/proc/swaps" >"$work/swaps.new" || true
mv "$work/swaps.new" "$work/proc/swaps"
EOF
chmod +x "$work/bin/"*

# A machine with 8 GB, zram loaded or not, zram0 free or taken.
reset() {
    rm -rf "$work/sys" "$work/proc" "$work/dev" "$work/log"
    mkdir -p "$work/proc/sys/vm" "$work/dev"
    printf 'MemTotal:        8000000 kB\nMemFree:         1000 kB\n' >"$work/proc/meminfo"
    printf 'Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n' >"$work/proc/swaps"
    for n in 0 1; do
        mkdir -p "$work/sys/block/zram$n"
        echo 0 >"$work/sys/block/zram$n/initstate"
        echo "lzo lzo-rle lz4 [zstd]" >"$work/sys/block/zram$n/comp_algorithm"
        touch "$work/dev/zram$n"
    done
    touch "$work/log"
}

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok   $1"
    else
        echo "FAIL $1: expected '$3', got '$2'"
        failures=$((failures + 1))
    fi
}
calls() { grep -c "^$1 " "$work/log" || true; }

reset
"$root/zram-swap" start >/dev/null
check "loads zram when it is missing" "$(calls modprobe)" 1
check "sizes half of the memory" "$(cat "$work/sys/block/zram0/disksize")" 4000000K
check "picks zstd" "$(cat "$work/sys/block/zram0/comp_algorithm")" zstd
check "swaps on zram0 first" "$(grep '^swapon ' "$work/log")" "swapon --discard --priority 100 $work/dev/zram0"
check "tunes swappiness" "$(cat "$work/proc/sys/vm/swappiness")" 180
check "reads one page at a time" "$(cat "$work/proc/sys/vm/page-cluster")" 0

"$root/zram-swap" start >/dev/null
check "a second start leaves it alone" "$(calls swapon)" 1

"$root/zram-swap" stop >/dev/null
check "stop swaps off" "$(calls swapoff)" 1
check "stop resets the device" "$(cat "$work/sys/block/zram0/reset")" 1
check "stop leaves no zram swap" "$(grep -c zram "$work/proc/swaps" || true)" 0

reset
mkdir -p "$work/sys/class/zram-control"
echo 1 >"$work/sys/block/zram0/initstate"
echo 1 >"$work/sys/class/zram-control/hot_add"
"$root/zram-swap" start >/dev/null
check "zram already loaded is not loaded again" "$(calls modprobe)" 0
check "a taken zram0 gets a new device" "$(grep '^swapon ' "$work/log")" "swapon --discard --priority 100 $work/dev/zram1"

reset
echo "lzo lzo-rle [lz4]" >"$work/sys/block/zram0/comp_algorithm"
"$root/zram-swap" start >/dev/null 2>&1
check "keeps the default without zstd" "$(cat "$work/sys/block/zram0/comp_algorithm")" "lzo lzo-rle [lz4]"

reset
ZRAM_SWAP_PERCENT=100 "$root/zram-swap" start >/dev/null
check "ZRAM_SWAP_PERCENT sets the size" "$(cat "$work/sys/block/zram0/disksize")" 8000000K

reset
status=0
ZRAM_SWAP_PERCENT=half "$root/zram-swap" start >/dev/null 2>&1 || status=$?
check "a malformed percent fails" "$status" 1
check "a malformed percent swaps nothing" "$(calls swapon)" 0

if [ "$failures" -gt 0 ]; then
    echo "$failures failed"
    exit 1
fi
echo "all passed"
