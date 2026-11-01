#!/bin/sh
# Runs slot-success against a fake GPT, bootconfig, systemd and qbootctl.
set -eu

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/by-partlabel"
touch "$work/sde14" "$work/sde57"
ln -s "$work/sde14" "$work/by-partlabel/boot_a"
ln -s "$work/sde57" "$work/by-partlabel/boot_b"

# sfdisk lists boot_a with the bits in $work/bits-a and boot_b with those in
# $work/bits; lsblk names their disk.
cat >"$work/bin/sfdisk" <<FAKE
#!/bin/sh
echo 'label: gpt'
echo "/dev/sde14 : start=123558, size=24576, type=77036CD4, name=\"boot_a\", attrs=\"GUID:\$(cat "$work/bits-a")\""
echo "/dev/sde57 : start=362062, size=24576, type=20117F86, name=\"boot_b\", attrs=\"GUID:\$(cat "$work/bits")\""
FAKE
cat >"$work/bin/lsblk" <<'FAKE'
#!/bin/sh
echo sde
FAKE
# systemctl answers from $FAKE_STATE and the units listed in $FAKE_ACTIVE.
cat >"$work/bin/systemctl" <<'FAKE'
#!/bin/sh
case $1 in
is-system-running) echo "$FAKE_STATE"; [ "$FAKE_STATE" = running ] ;;
is-active) case " $FAKE_ACTIVE " in *" $3 "*) exit 0 ;; *) exit 3 ;; esac ;;
esac
FAKE
cat >"$work/bin/sleep" <<FAKE
#!/bin/sh
echo "\$1" >>"$work/slept"
FAKE
# qbootctl records its arguments and, unless $FAKE_QBOOTCTL_NOOP, sets bit 54.
cat >"$work/qbootctl" <<FAKE
#!/bin/sh
echo "\$*" >"$work/qbootctl-args"
[ -n "\${FAKE_QBOOTCTL_NOOP-}" ] || sed -i 's/\$/,54/' "$work/bits"
FAKE
chmod +x "$work/bin/sfdisk" "$work/bin/lsblk" "$work/bin/systemctl" "$work/bin/sleep" "$work/qbootctl"

export PATH="$work/bin:$PATH"
export SLOT_SUCCESS_CONF="$work/slot-success.conf"
export SLOT_SUCCESS_QBOOTCTL="$work/qbootctl"
export SLOT_SUCCESS_BOOTCONFIG="$work/bootconfig"
export SLOT_SUCCESS_CMDLINE="$work/cmdline"
export SLOT_SUCCESS_UPTIME="$work/uptime"
export SLOT_SUCCESS_PARTLABELS="$work/by-partlabel"
export FAKE_STATE=running FAKE_ACTIVE="denial.service sshd.service"

printf 'slot=b\nunits=denial.service sshd.service\nsettle=60\n' >"$work/slot-success.conf"
echo 'androidboot.slot_suffix = "_b"' >"$work/bootconfig"
echo 'console=tty0' >"$work/cmdline"
echo '75.20 300.00' >"$work/uptime"
echo 49,51,52,53,54 >"$work/bits-a"

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok   $1"
    else
        echo "FAIL $1: expected '$3', got '$2'"
        failures=$((failures + 1))
    fi
}
# run CMD: sets $out to its output and $status to its exit code.
run() {
    rm -f "$work/qbootctl-args" "$work/slept"
    if out=$("$root/slot-success" "$1" 2>&1); then status=0; else status=$?; fi
}
called() {
    if [ -f "$work/qbootctl-args" ]; then cat "$work/qbootctl-args"; else echo none; fi
}

echo 48,49,50,51 >"$work/bits"
run status
check "status counts the tries left" "$status $out" "1 Slot b: not marked successful, 1 boot try left."
echo 48,49,50,51,52,53 >"$work/bits"
run status
check "status reads seven tries" "$out" "Slot b: not marked successful, 7 boot tries left."
echo 48,49,50,54 >"$work/bits"
run status
check "status of a marked slot exits 0" "$status $out" "0 Slot b: marked successful."
echo 48,49,51,55 >"$work/bits"
run status
check "status of an unbootable slot exits 1" "$status $out" "1 Slot b: unbootable."

echo 48,49,50,51 >"$work/bits"
run mark
check "mark marks a settled boot" "$status $(called)" "0 -m 1"
check "mark reads the mark back" "$(cat "$work/bits")" "48,49,50,51,54"

run mark
check "mark leaves a marked slot alone" "$status $(called)" "0 none"

echo 48,49,50,51 >"$work/bits"
echo 'androidboot.slot_suffix = "_a"' >"$work/bootconfig"
run mark
check "mark never touches the other slot" "$status $(called)" "0 none"
: >"$work/bootconfig"
echo 'console=tty0 androidboot.slot_suffix=_b' >"$work/cmdline"
run mark
check "mark reads the slot from the command line" "$status $(called)" "0 -m 1"
echo 'console=tty0' >"$work/cmdline"
echo 48,49,50,51 >"$work/bits"
run mark
check "mark takes the active slot when nothing names one" "$status $(called)" "0 -m 1"
run running
check "running names the active slot" "$out" b
echo 48,49,50,51 >"$work/bits-a"
echo 48,49,51 >"$work/bits"
run mark
check "mark leaves the slots alone when slot a is active" "$status $(called)" "0 none"
echo 48,49,50,51 >"$work/bits"
run running
check "running gives up when both slots read active" "$status" 2
echo 49,51,52,53,54 >"$work/bits-a"
echo 'androidboot.slot_suffix = "_b"' >"$work/bootconfig"

echo 48,49,50,51 >"$work/bits"
echo '20.50 40.00' >"$work/uptime"
run mark
check "mark waits out the settle time" "$(head -n 1 "$work/slept")" 40
echo '75.20 300.00' >"$work/uptime"

echo 48,49,50,51 >"$work/bits"
FAKE_STATE=degraded run mark
check "mark skips a degraded system" "$status $(called)" "1 none"
FAKE_ACTIVE=sshd.service run mark
check "mark skips when a unit is down" "$status $(called)" "1 none"
FAKE_STATE=starting run mark
check "mark gives up on a system that never finishes starting" "$status $(called) $(wc -l <"$work/slept")" "1 none 60"
FAKE_QBOOTCTL_NOOP=1 run mark
check "mark fails when the mark does not read back" "$status" 1

printf 'slot=c\n' >"$work/slot-success.conf"
run status
check "a bad slot in the configuration exits 2" "$status" 2
run bogus
check "unknown command exits 2" "$status" 2

[ "$failures" -eq 0 ]
