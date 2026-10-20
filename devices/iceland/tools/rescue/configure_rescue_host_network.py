#!/usr/bin/env python3
"""Assign only the verified Pad4 rescue USB NIC its local transfer address."""
import json
from pathlib import Path
import subprocess

matches = []
for interface in Path('/sys/class/net').iterdir():
    if (interface / 'address').read_text().strip() != '02:50:41:44:34:01':
        continue
    usb = (interface / 'device').resolve().parent
    if (usb / 'serial').read_text().strip() != 'OPD2514-RESCUE':
        continue
    if (usb / 'idVendor').read_text().strip() != '1d6b' or (usb / 'idProduct').read_text().strip() != '0104':
        continue
    matches.append(interface.name)
if len(matches) != 1:
    raise SystemExit(f'Expected exactly one verified tablet interface: {matches}')
nic = matches[0]
# Prevent automatic DHCP retries from clearing this temporary static address.
# This affects only the identity-verified rescue NIC, not the host LAN.
subprocess.run(['/usr/bin/nmcli', 'device', 'set', nic, 'managed', 'no'], check=True)
subprocess.run(['/usr/bin/ip', 'link', 'set', 'dev', nic, 'up'], check=True)
addresses = json.loads(subprocess.check_output(['/usr/bin/ip', '-j', '-4', 'addr', 'show', 'dev', nic]))
if not any(a['local'] == '192.168.77.1' and a['prefixlen'] == 30 for row in addresses for a in row['addr_info']):
    subprocess.run(['/usr/bin/ip', 'addr', 'add', '192.168.77.1/30', 'dev', nic], check=True)
subprocess.run(['/usr/bin/ip', '-br', 'addr', 'show', 'dev', nic], check=True)
subprocess.run(['/usr/bin/ping', '-c', '3', '-W', '2', '192.168.77.2'], check=True)
