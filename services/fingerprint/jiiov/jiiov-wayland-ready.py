#!/usr/bin/python3
"""Read-only pre-fprintd gate for Moto's inode-pinning Wayland bind mount.

This does not connect to Wayland, request a registry, touch input/HBM, or
activate/restart any service. Run OUTSIDE fprintd's mount namespace.
"""
import argparse
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time

UNIT = 'denial-moto70.service'
# Denial runs in the unprivileged alarm session (uid 1000) since 2026-10-16.
SESSION_UID = 1000
SOCKET = Path(f'/run/user/{SESSION_UID}/wayland-1')
PROC = Path('/proc')


def service_state():
    result = subprocess.run(
        ['/usr/bin/systemctl', 'show', UNIT, '--property=MainPID',
         '--property=ActiveState', '--property=SubState'],
        check=True, capture_output=True, text=True, timeout=1)
    fields = dict(line.split('=', 1) for line in result.stdout.splitlines())
    return int(fields['MainPID']), fields['ActiveState'], fields['SubState']


def process_start(pid, proc=PROC):
    # comm is parenthesized and may contain spaces or ')'; field22=starttime.
    text = (proc / str(pid) / 'stat').read_text()
    return text.rsplit(')', 1)[1].split()[19]


def inspect_listener(pid, proc=PROC, socket_path=SOCKET):
    """Return immutable identity only if CURRENT PID owns the live listener."""
    if pid <= 0:
        raise ValueError('compositor has no current MainPID')
    directory = proc / str(pid)
    start = process_start(pid, proc)
    executable = os.readlink(directory / 'exe')
    if Path(executable).name != 'deniald':
        raise ValueError('MainPID is not deniald')
    host = socket_path.lstat()
    if not stat.S_ISSOCK(host.st_mode) or host.st_uid != SESSION_UID:
        raise ValueError('host path is not a session-owned socket')
    visible = (directory / 'root' / str(socket_path).lstrip('/')).stat()
    identity = (host.st_dev, host.st_ino)
    if not stat.S_ISSOCK(visible.st_mode) or identity != (visible.st_dev, visible.st_ino):
        raise ValueError('host/compositor socket inode differs')

    owned = set()
    for fd in (directory / 'fd').iterdir():
        try:
            match = re.fullmatch(r'socket:\[(\d+)\]', os.readlink(fd))
            if match:
                owned.add(match[1])
        except FileNotFoundError:
            continue  # An unrelated transient fd is not listener evidence.
    listener = None
    for line in (directory / 'net/unix').read_text().splitlines()[1:]:
        fields = line.split(maxsplit=7)
        if (len(fields) == 8 and fields[7] == str(socket_path)
                and fields[6] in owned and int(fields[3], 16) & 0x10000
                and fields[4] == '0001' and fields[5] == '01'):
            listener = fields[6]
            break
    if listener is None:
        raise ValueError('current MainPID does not own this listening UNIX socket')
    # Bracket the /proc snapshot; never accept a replaced pathname/PID.
    after = socket_path.lstat()
    visible_after = (directory / 'root' / str(socket_path).lstrip('/')).stat()
    if (identity != (after.st_dev, after.st_ino)
            or identity != (visible_after.st_dev, visible_after.st_ino)
            or start != process_start(pid, proc)):
        raise ValueError('process/socket identity changed during readiness check')
    return pid, start, *identity, listener


def wait_ready(timeout, state=service_state, inspect=inspect_listener,
               clock=time.monotonic, sleep=time.sleep):
    deadline = clock() + timeout
    last = 'listener not checked'
    while clock() < deadline:
        try:
            current = state()
            if current[1:] != ('active', 'running'):
                raise ValueError('compositor is not active/running')
            identity = inspect(current[0])
            if current != state():
                raise ValueError('current MainPID/service state changed during check')
            if clock() >= deadline:
                raise ValueError('readiness check exceeded its deadline')
            return identity
        except (OSError, ValueError, KeyError, IndexError,
                subprocess.SubprocessError) as error:
            last = str(error)
        sleep(min(0.05, max(0, deadline - clock())))
    raise TimeoutError('Wayland socket readiness timeout: ' + last)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--timeout', type=float, default=10)
    args = parser.parse_args()
    if not 0 < args.timeout <= 10:
        parser.error('timeout must be >0 and <=10 seconds')
    try:
        pid, start, dev, inode, listener = wait_ready(args.timeout)
    except (TimeoutError, ValueError) as error:
        print('JIIOV Wayland gate: ' + str(error), file=sys.stderr, flush=True)
        return 1
    print(f'JIIOV Wayland gate: ready pid={pid} start={start} '
          f'path={SOCKET} dev={dev} inode={inode} listener={listener}', flush=True)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
