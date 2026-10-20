#!/usr/bin/env python3
"""Persist host access only for the identity-verified OPD2514 rescue gadget."""
from pathlib import Path
import os
import pwd
import subprocess

def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()

assert os.geteuid() == 0
owner = pwd.getpwnam('logix')
matches = []
for interface in Path('/sys/class/net').iterdir():
    if (interface / 'address').read_text().strip() != '02:50:41:44:34:01':
        continue
    usb = (interface / 'device').resolve().parent
    if all((usb / k).read_text().strip() == v for k,v in
           [('idVendor','1d6b'),('idProduct','0104'),('serial','OPD2514-RESCUE')]):
        matches.append(interface.name)
assert len(matches) == 1, matches
nic = matches[0]
rule = ('SUBSYSTEM=="tty", ATTRS{idVendor}=="1d6b", ATTRS{idProduct}=="0104", '
        'ATTRS{serial}=="OPD2514-RESCUE", OWNER="logix", MODE="0600"\n')
path = Path('/etc/udev/rules.d/99-opd2514-rescue.rules')
if path.exists():
    assert path.read_text() == rule, 'Existing rule differs; refusing to overwrite'
else:
    path.write_text(rule)
    path.chmod(0o644)
run('/usr/bin/udevadm', 'control', '--reload-rules')
# Apply access to the current matching tty without re-enumerating USB.
for tty in Path('/sys/class/tty').glob('ttyACM*'):
    if usb in (tty / 'device').resolve().parents:
        dev = Path('/dev') / tty.name
        os.chown(dev, owner.pw_uid, -1)
        dev.chmod(0o600)
name = 'pad4-rescue-usb'
names = run('/usr/bin/nmcli', '-t', '-f', 'NAME', 'connection', 'show').splitlines()
if name not in names:
    run('/usr/bin/nmcli', 'connection', 'add', 'type', 'ethernet', 'con-name', name,
        'ifname', '*', '802-3-ethernet.mac-address', '02:50:41:44:34:01',
        'ipv4.method', 'manual', 'ipv4.addresses', '192.168.77.1/30',
        'ipv4.never-default', 'yes', 'ipv6.method', 'disabled',
        'connection.autoconnect', 'yes', 'connection.autoconnect-priority', '100')
else:
    assert run('/usr/bin/nmcli','-g','802-3-ethernet.mac-address','connection','show',name).lower() == '02:50:41:44:34:01'
    assert run('/usr/bin/nmcli','-g','ipv4.addresses','connection','show',name) == '192.168.77.1/30'
run('/usr/bin/nmcli', 'device', 'set', nic, 'managed', 'yes')
print(run('/usr/bin/nmcli', 'connection', 'up', name, 'ifname', nic))
print('Persistent serial access and private USB IPv4 profile installed for OPD2514-RESCUE only.')
