#!/usr/bin/env python3
"""Read sysfs until a tablet candidate or new USB device appears; send no USB I/O.

Exits immediately on detection so the waiting agent receives the result.
Unknown USB IDs are also reported rather than silently excluded.
"""
import datetime
import json
import os
from pathlib import Path
import time

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_VENDORS = {'05c6', '2a70', '22d9', '18d1'}
OUTPUT = ROOT / 'out' / ('usb-watch-' + datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ'))


def scan():
    found = {}
    for path in Path('/sys/bus/usb/devices').iterdir():
        try:
            if path.name.startswith('usb'):
                continue
            vid = (path / 'idVendor').read_text().strip()
            pid = (path / 'idProduct').read_text().strip()
            bus, address = (int((path / key).read_text()) for key in ('busnum', 'devnum'))
            node = f'/dev/bus/usb/{bus:03d}/{address:03d}'
            item = dict(path=path.name, vid=vid, pid=pid, node=node,
                        rw=os.access(node, os.R_OK | os.W_OK))
            for key in ('product', 'manufacturer', 'serial'):
                if (path / key).exists():
                    item[key] = (path / key).read_text().strip()
            item['interfaces'] = []
            for interface in path.glob(path.name + ':*'):
                item['interfaces'].append({key: (interface / key).read_text().strip()
                    for key in ('bInterfaceClass', 'bInterfaceSubClass', 'bInterfaceProtocol')
                    if (interface / key).exists()})
            found[(path.name, bus, address)] = item
        except (FileNotFoundError, NotADirectoryError, ValueError):
            continue
    return found


def candidate(item):
    return (item['vid'] in EXPECTED_VENDORS or 'e30ffb36' in item.get('serial', '').lower()
            or 'e30ffb36' in item.get('product', '').lower()
            or any(i.get('bInterfaceClass') == 'ff' and i.get('bInterfaceSubClass') == '42'
                   for i in item['interfaces']))


OUTPUT.mkdir()
baseline = scan()
(OUTPUT / 'baseline.json').write_text(json.dumps(list(baseline.values()), indent=2) + '\n')
(OUTPUT / 'pid').write_text(str(os.getpid()) + '\n')
print('WATCHING: every 0.2 seconds; no USB commands; log ' + str(OUTPUT), flush=True)
last = baseline
heartbeat = 0
with (OUTPUT / 'events.jsonl').open('x') as log:
    while True:
        current = scan()
        detected = [item for key, item in current.items() if key not in baseline or candidate(item)]
        now = datetime.datetime.now(datetime.timezone.utc).isoformat()
        if current != last:
            log.write(json.dumps(dict(time=now, devices=list(current.values()))) + '\n')
            log.flush()
            last = current
        if detected:
            result = dict(time=now, event='USB_DEVICE_DETECTED', devices=detected,
                          note='Candidate only; confirm identity before device commands')
            (OUTPUT / 'detected.json').write_text(json.dumps(result, indent=2) + '\n')
            print(json.dumps(result, indent=2), flush=True)
            break
        if time.monotonic() - heartbeat >= 5:
            (OUTPUT / 'status.json').write_text(json.dumps(dict(time=now, status='watching', candidates=0)) + '\n')
            heartbeat = time.monotonic()
        time.sleep(0.2)
