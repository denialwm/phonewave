#!/bin/sh
# Checks radio-addr's addresses against sha256sum(1) for fake serials.
set -eu

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export RADIO_ADDR_SERIAL_FILE="$work/serial"

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok   $1"
    else
        echo "FAIL $1: expected '$3', got '$2'"
        failures=$((failures + 1))
    fi
}
# The first six bytes of SHA-256 over LABEL, a NUL and SERIAL, colon separated.
digest() {
    printf '%s\000%s' "$1" "$2" | sha256sum | cut -c1-12 | sed 's/../&:/g; s/:$//'
}
address() {
    "$root/radio-addr" print | sed -n "s/^$1 //p"
}

# The last one fills more than one SHA-256 block.
for serial in 1234567890 3478592817 \
    0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef; do
    printf '%s\n' "$serial" >"$work/serial"
    wifi=$(digest "radio-addr wifi" "$serial")
    first=$(printf '%d' "0x${wifi%%:*}")
    wifi=$(printf '%02x' $(((first & 0xfc) | 0x02)))${wifi#??}
    check "wifi, serial $serial" "$(address wifi)" "$wifi"
    check "bluetooth, serial $serial" "$(address bluetooth)" \
        "$(digest "radio-addr bluetooth" "$serial")"
done

# test343532 lands in the reserved inquiry LAPs 9e8b00-9e8b3f; the address
# moves out of them. Its plain digest is 96:f9:f0:9e:8b:12.
printf 'test343532\n' >"$work/serial"
check "bluetooth, reserved LAP" "$(address bluetooth)" "96:f9:f0:9f:8b:12"

# The same serial without its newline gives the same addresses.
printf '1234567890' >"$work/serial"
plain=$("$root/radio-addr" print)
printf '1234567890\n' >"$work/serial"
check "serial newline" "$("$root/radio-addr" print)" "$plain"

: >"$work/serial"
if "$root/radio-addr" print >/dev/null 2>&1; then
    check "empty serial fails" ok fails
else
    check "empty serial fails" fails fails
fi

[ "$failures" -eq 0 ]
