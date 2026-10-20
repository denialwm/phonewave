#!/usr/bin/env python3
"""Authorized, bounded Roadstr optical tests over USB SSH; no authentication changes."""
import argparse
import hashlib
import os
from pathlib import Path
import shlex
import subprocess

CACHE = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache') / 'phonewave/roadstr'
SSH = ['ssh', '-o', 'StrictHostKeyChecking=yes', '-o', 'ConnectTimeout=5', 'roadstr']


def stage(local, remote):
    data = local.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    command = ('set -eu; grep -q ZY22MMG59D /proc/bootconfig; '
               f'cat > {shlex.quote(remote)}; chmod 700 {shlex.quote(remote)}; '
               f'test "$(sha256sum {shlex.quote(remote)} | cut -d " " -f 1)" = {digest}')
    subprocess.run(SSH + [command], input=data, check=True)
    return digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['capture', 'sample', 'enroll', 'verify'])
    parser.add_argument('--log', required=True, type=Path)
    parser.add_argument('--target', action='store_true', help='Show the sensor target and timed cues')
    args = parser.parse_args()
    probe = '/tmp/tee-probe-optical'
    client = '/tmp/fingerprint-illumination-optical'
    hashes = {
        'probe': stage(CACHE / 'fingerprint-native1/tee-probe', probe),
        'client': stage(CACHE / 'fingerprint-native2/illumination-test', client),
    }
    mode = {'capture': '--capture-diagnostic-with-files', 'sample': '--enroll-sample-with-files',
            'enroll': '--enroll-with-files', 'verify': '--verify-with-files'}[args.mode]
    storage = '/var/lib/moto70-fingerprint1/native-test-templates'
    persistent = args.mode in ('enroll', 'verify')
    storage_setup = f'install -d -m 700 {storage}' if persistent else ':'
    storage_argument = storage if persistent else ''
    levels = ('16180' if args.target else '8089') if persistent else '8089 16180'
    target_argument = '--target "$session_dir/progress"' if args.target else ''
    if args.mode == 'enroll' and not args.target:
        parser.error('Full enrollment requires --target for placement and timing cues')
    command = f'''set -eu
grep -q ZY22MMG59D /proc/bootconfig
{storage_setup}
session_dir=$(mktemp -d /run/fingerprint-optical.XXXXXX)
printf 'PLACE 20\\n' > "$session_dir/progress"
brightness=/sys/class/backlight/panel0-backlight/brightness
saved_brightness=$(cat "$brightness")
illumination_pid=
cleanup() {{
 printf '%s\\n' "$saved_brightness" > "$brightness"
 if test -n "$illumination_pid"; then
  kill "$illumination_pid" 2>/dev/null || true
  wait "$illumination_pid" || true
 fi
 printf 'Restored brightness='; cat "$brightness"
 rm -f "$session_dir/progress"
 rmdir "$session_dir"
}}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
XDG_RUNTIME_DIR=/run/user/0 WAYLAND_DISPLAY=wayland-1 {client} {target_argument} > /tmp/fingerprint-optical-surface.log 2>&1 &
illumination_pid=$!
for n in $(seq 1 30); do
 if grep -q 'First frame callback' /tmp/fingerprint-optical-surface.log; then break; fi
 sleep 0.1
done
grep -q 'First frame callback' /tmp/fingerprint-optical-surface.log
cat /tmp/fingerprint-optical-surface.log
for level in {levels}; do
 kill -0 "$illumination_pid"
 test "$(cat /sys/class/drm/card0-DSI-1/dpms)" = On
 printf '%s\\n' "$level" > "$brightness"
 sleep 4
 date -u
 printf 'Brightness='; cat "$brightness"
 ANC_DIAGNOSTIC_PROGRESS_FD=3 {probe} {mode} /tmp/ancapp64.elf /var/lib/moto70-fingerprint1/calibration {storage_argument} 3<>"$session_dir/progress" || test "$?" = 1
done
'''
    with args.log.open('w') as log:
        log.write(f'Binaries SHA256: {hashes}\n')
        log.flush()
        process = subprocess.Popen(SSH + [command], stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True)
        for line in process.stdout:
            print(line, end='', flush=True)
            log.write(line)
            log.flush()
        return process.wait()


if __name__ == '__main__':
    raise SystemExit(main())
