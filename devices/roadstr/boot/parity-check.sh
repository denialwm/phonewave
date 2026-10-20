#!/bin/sh
# Read-only parity check, run on the phone: parity-check.sh | ssh roadstr sh
echo "== kernel"; uname -r; cat /proc/cmdline
echo "== handoff"; [ -e /proc/device-tree/chosen/linux,fdt-handoff ] && echo linux,fdt-handoff || echo none
tr '\0' ' ' </proc/device-tree/compatible; echo
tr -d '\0' </proc/device-tree/model; echo
echo "== systemd"; systemctl is-system-running; systemctl --failed --no-legend --plain
echo "== denial"; pidof deniald || echo "deniald not running"
echo "== remoteproc"
for r in /sys/class/remoteproc/remoteproc*; do echo "$(cat $r/name) $(cat $r/state)"; done
echo "== bluetooth"; for h in /sys/class/bluetooth/hci*; do [ -e "$h" ] && echo "$h"; done
busctl get-property org.bluez /org/bluez/hci0 org.bluez.Adapter1 Address 2>&1
busctl get-property org.bluez /org/bluez/hci0 org.bluez.Adapter1 Powered 2>&1
echo "== wifi"; nmcli -t -f DEVICE,TYPE,STATE,CONNECTION dev 2>&1
cat /sys/class/net/wlan0/address 2>/dev/null
echo "== gpu"
d=/sys/class/devfreq/3d00000.gpu
[ -d $d ] && echo "gov $(cat $d/governor) cur $(cat $d/cur_freq) min $(cat $d/min_freq) max $(cat $d/max_freq)"
ls /dev/dri
echo "== power"
for p in /sys/class/power_supply/*; do echo "$(basename $p): $(cat $p/status 2>/dev/null) $(cat $p/capacity 2>/dev/null) $(cat $p/current_now 2>/dev/null)"; done
echo "== built-in trees"
ls /sys/firmware/devicetree/base >/dev/null && echo "live tree ok"
echo "== modules"; lsmod | wc -l
echo "== dmesg warnings"
dmesg --level=emerg,alert,crit,err,warn | tail -n 60
