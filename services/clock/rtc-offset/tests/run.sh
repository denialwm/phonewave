#!/bin/sh
# Runs rtc-offset against a fake RTC, state directory and date(1).
set -eu

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/rtc" "$work/bin" "$work/state"

# date +%s answers $FAKE_NOW, date -u -s @N records N; anything else is real.
real_date=$(command -v date)
cat >"$work/bin/date" <<EOF
#!/bin/sh
if [ "\$*" = "+%s" ]; then echo "\$FAKE_NOW"; exit; fi
if [ "\${1-}" = "-u" ] && [ "\${2-}" = "-s" ]; then echo "\${3#@}" >"$work/set"; exit; fi
exec "$real_date" "\$@"
EOF
chmod +x "$work/bin/date"

export PATH="$work/bin:$PATH"
export RTC_OFFSET_DEVICE="$work/rtc"
export STATE_DIRECTORY="$work/state"
export RTC_OFFSET_SYNCHRONIZED="$work/synchronized"

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok   $1"
    else
        echo "FAIL $1: expected '$3', got '$2'"
        failures=$((failures + 1))
    fi
}
run() {
    rm -f "$work/set"
    "$root/rtc-offset" "$@" >/dev/null
}
set_value() {
    if [ -f "$work/set" ]; then cat "$work/set"; else echo none; fi
}

echo 1000 >"$work/rtc/since_epoch"
export FAKE_NOW=500

run restore
check "restore without a saved offset" "$(set_value)" none

run save
check "save before synchronization" "$(test -f "$work/state/offset" && echo saved || echo none)" none

touch "$work/synchronized"
export FAKE_NOW=1792511149
run save
check "save after synchronization" "$(cat "$work/state/offset")" 1792510149

# Powered off for an hour: the RTC kept counting, the clock restarts behind.
echo 4600 >"$work/rtc/since_epoch"
export FAKE_NOW=1792511200
run restore
check "restore steps the clock forward" "$(set_value)" 1792514749

# The RTC restarted from zero: RTC plus offset lands behind the clock.
echo 10 >"$work/rtc/since_epoch"
run restore
check "restore never steps backwards" "$(set_value)" none

echo garbage >"$work/state/offset"
run restore
check "restore ignores a malformed offset" "$(set_value)" none

printf '%s\n' -100 >"$work/state/offset"
echo 1792511400 >"$work/rtc/since_epoch"
run restore
check "restore accepts a negative offset" "$(set_value)" 1792511300

if "$root/rtc-offset" bogus 2>/dev/null; then status=0; else status=$?; fi
check "unknown command exits 2" "$status" 2

[ "$failures" -eq 0 ]
