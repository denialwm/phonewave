#!/usr/bin/python3
# SPDX-License-Identifier: GPL-2.0-only
"""Roadstr libfprint worker with trusted Denial presentation for verification.

Enrollment retains the separately enabled, unlocked-screen diagnostic path.
"""
import fcntl
import json
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time

PROBE = '/usr/local/libexec/jiiov-native-probe'
CLIENT = '/usr/local/libexec/jiiov-illumination-test'
PRESENTATION = '/usr/local/libexec/jiiov-denial-presentation'
BASE = Path('/var/lib/moto70-fingerprint1')
STORAGE = BASE / 'native-test-templates'
BRIGHTNESS = Path('/sys/class/backlight/panel0-backlight/brightness')
DPMS = Path('/sys/class/drm/card0-DSI-1/dpms')
RUNTIME = Path('/run/jiiov-fprint')
interrupted = False


def interrupt(_signum, _frame):
    global interrupted
    interrupted = True


class Records:
    """Consume status metadata only. Never turn missing output into success."""
    def __init__(self, operation):
        self.operation = operation
        self.ids = []
        self.remaining = 20
        self.final = None
        self.comparison = None
        self.unloaded = False
        self.deleted = False

    def feed(self, line):
        if line == 'unload ancapp64: ioctl=0 errno=0 result=0':
            self.unloaded = True
        match = re.fullmatch(r'Isolated template ID=(\d+)', line)
        if match:
            value = int(match[1])
            if not 0 < value <= 0xffffffff or value in self.ids or len(self.ids) >= 5:
                raise ValueError('Invalid template list')
            self.ids.append(value)
        match = re.fullmatch(r'Enrollment progress status=(400|401) remaining=(\d+)', line)
        if match:
            remaining = int(match[2])
            if self.operation != 'enroll' or not 0 <= remaining <= self.remaining:
                raise ValueError('Invalid enrollment progress')
            if remaining < self.remaining:
                self.remaining = remaining
                print(f'JFP1 progress {20 - remaining}', flush=True)
        match = re.fullmatch(r'Enrollment finalize status=(-?\d+) template_id=(\d+)', line)
        if match:
            if self.final is not None:
                raise ValueError('Repeated enrollment result')
            self.final = tuple(map(int, match.groups()))
        match = re.fullmatch(r'Verification compare status=(-?\d+) matched_id=(\d+)', line)
        if match:
            if self.comparison is not None:
                raise ValueError('Repeated verification result')
            self.comparison = tuple(map(int, match.groups()))
        if re.fullmatch(r'After deletion template count=[0-4]', line):
            self.deleted = True

    def result(self, returncode):
        if not self.unloaded:
            raise RuntimeError('Firmware cleanup was not confirmed')
        if self.operation == 'verify':
            # Stock return strings: 501 "algo match fail", 502 "algo match
            # fail stop retry". Both are nonmatches, not transport errors.
            if self.comparison and self.comparison[0] in (501, 502) and returncode == 1:
                return 1, 0
            if returncode == 0 and self.comparison and self.comparison[0] == 500:
                identifier = self.comparison[1]
                if identifier in self.ids:
                    return 0, identifier
            raise RuntimeError('No valid verification result')
        if returncode:
            raise RuntimeError('Native operation failed')
        if self.operation == 'enroll':
            if (self.remaining != 0 or not self.final or self.final[0] != 0 or
                    not 0 < self.final[1] <= 0xffffffff or self.final[1] in self.ids):
                raise RuntimeError('Template persistence was not confirmed')
            return 0, self.final[1]
        if self.operation == 'delete' and not self.deleted:
            raise RuntimeError('Deletion was not confirmed')
        return 0, 0


def stop(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
        raise RuntimeError('Worker required forced termination; firmware cleanup is unconfirmed')


def lines(process, timeout, watch=None, watch_fd=None):
    deadline = time.monotonic() + timeout
    pending = b''
    total = 0
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ, 'native')
        if watch_fd is not None:
            selector.register(watch_fd, selectors.EVENT_READ, 'presentation')
        while True:
            if watch:
                watch()
            if interrupted:
                raise InterruptedError('Cancelled')
            if time.monotonic() >= deadline:
                raise TimeoutError('Native operation timed out')
            events = selector.select(0.1)
            if watch:
                watch()
            if not any(key.data == 'native' for key, _ in events):
                continue
            chunk = os.read(process.stdout.fileno(), 4096)
            if not chunk:
                if pending:
                    raise ValueError('Truncated worker record')
                return
            total += len(chunk)
            if total > 512 * 1024:
                raise ValueError('Excessive worker output')
            pending += chunk
            while b'\n' in pending:
                line, pending = pending.split(b'\n', 1)
                if len(line) > 1024:
                    raise ValueError('Oversized worker record')
                yield line.decode('ascii', errors='strict')
            if len(pending) > 1024:
                raise ValueError('Oversized worker record')


class ContactLost(RuntimeError):
    pass


def process_identity(pid):
    value = Path(f'/proc/{pid}/stat').read_text()
    fields = value[value.rfind(')') + 2:].split()
    return {'pid': int(pid), 'parent': int(fields[1]), 'start': int(fields[19])}


def retained_path(pid):
    return RUNTIME / f'suspend-{pid}.json'


def retained_session():
    """Read-only resume-hook check; stale files cannot preserve a new daemon."""
    if os.geteuid() != 0 or 'ZY22MMG59D' not in Path('/proc/bootconfig').read_text():
        return False
    daemon = subprocess.check_output(['systemctl', 'show', 'fprintd.service',
                                      '-p', 'MainPID', '--value'], text=True).strip()
    if not daemon.isdecimal() or int(daemon) == 0:
        return False
    daemon_identity = process_identity(daemon)
    for path in RUNTIME.glob('suspend-*.json'):
        try:
            metadata = path.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid or metadata.st_mode & 0o077:
                continue
            record = json.loads(path.read_text())
            if record['boot_id'] != Path('/proc/sys/kernel/random/boot_id').read_text().strip():
                continue
            if record['daemon'] != daemon_identity:
                continue
            worker = process_identity(record['worker']['pid'])
            presentation = process_identity(record['presentation']['pid'])
            if (worker == record['worker'] and worker['parent'] == int(daemon)
                    and presentation == record['presentation'] and presentation['parent'] == worker['pid']):
                return True
        except (OSError, ValueError, KeyError, TypeError):
            continue
    return False


class Presentation:
    """Consume contact revocation even when firmware emits no status output."""
    def __init__(self, environment):
        self.process = subprocess.Popen([PRESENTATION], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=None,
                                        env={**environment, 'XDG_RUNTIME_DIR': str(RUNTIME),
                                             'WAYLAND_DISPLAY': 'wayland-1'})
        os.set_blocking(self.process.stdout.fileno(), False)
        self.pending = b''
        self.ready = False
        self.down = False
        self.down_at = None
        self.finishing = False
        self.stopped = False
        self.eof = False
        self.cancelling = False
        self.control_fd = sys.stdin.fileno() if environment.get('JIIOV_SUSPEND_PROTOCOL') == '1' else None
        self.control_pending = b''
        self.control_request = None
        self.suspended = self.suspend_rejected = False
        if self.control_fd is not None:
            os.set_blocking(self.control_fd, False)

    def control(self):
        if self.control_fd is None:
            return
        while True:
            try:
                data = os.read(self.control_fd, 128)
            except BlockingIOError:
                break
            if not data:
                raise InterruptedError('Fingerprint driver disconnected')
            self.control_pending += data
            if len(self.control_pending) > 128:
                raise ValueError('Excessive suspend control data')
            while b'\n' in self.control_pending:
                line, self.control_pending = self.control_pending.split(b'\n', 1)
                if self.control_request is not None or line not in (b'suspend', b'resume'):
                    raise ValueError('Invalid suspend control request')
                if (line == b'suspend') == self.suspended:
                    raise ValueError('Out-of-order suspend control request')
                self.control_request = line
                self.process.stdin.write(line + b'\n')
                self.process.stdin.flush()

    def suspension_reply(self, line):
        if self.control_fd is None:
            return False
        if line == b'DFP1 suspended' and self.control_request == b'suspend' and not self.down:
            self.suspended = True
            worker = process_identity(os.getpid())
            record = {'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                      'worker': worker, 'daemon': process_identity(worker['parent']),
                      'presentation': process_identity(self.process.pid)}
            path = retained_path(os.getpid())
            temporary = path.with_suffix('.tmp')
            with temporary.open('w') as stream:
                os.fchmod(stream.fileno(), 0o600)
                json.dump(record, stream)
            temporary.replace(path)
            print('JFP1 suspended', flush=True)
        elif line == b'DFP1 resumed' and self.control_request == b'resume' and self.suspended:
            self.suspended = False
            retained_path(os.getpid()).unlink(missing_ok=True)
            print('JFP1 resumed', flush=True)
        elif line == b'DFP1 suspend-busy' and self.control_request == b'suspend':
            self.suspend_rejected = True
            print('JFP1 suspend-busy', flush=True)
        else:
            return False
        self.control_request = None
        return True

    def poll(self):
        if interrupted:
            raise InterruptedError('Cancelled')
        self.control()
        while True:
            # Drain already buffered records before reading again. A loss
            # exception can leave the stopped record in this same pipe chunk.
            if b'\n' not in self.pending:
                if self.eof:
                    break
                try:
                    data = os.read(self.process.stdout.fileno(), 4096)
                except BlockingIOError:
                    break
                if not data:
                    self.eof = True
                    continue
                self.pending += data
                if len(self.pending) > 8192:
                    raise ValueError('Excessive presentation output')
                continue
            line, self.pending = self.pending.split(b'\n', 1)
            if line.startswith(b'DFP1 ') and len(line) <= 80:
                print('JIIOV presentation: ' + line.decode('ascii', errors='strict'), file=sys.stderr)
            if line == b'DFP1 waiting':
                continue
            if self.suspension_reply(line):
                continue
            if line == b'DFP1 down 1' and not self.down:
                self.down = True
                self.down_at = time.monotonic()
                continue
            if line == b'DFP1 ready 1' and not self.ready and not self.finishing:
                self.ready = True
                self.timing('presentation-ready')
            elif line == b'DFP1 lost 1':
                if not self.cancelling:
                    raise ContactLost('Fingerprint contact or presentation was lost')
            elif line == b'DFP1 stopped 1 0' and (self.finishing or self.cancelling):
                self.stopped = True
            else:
                raise RuntimeError('Fingerprint contact or presentation was lost')
        if self.process.poll() is not None and not ((self.finishing or self.cancelling) and self.stopped):
            raise RuntimeError('Fingerprint presentation worker exited')

    def timing(self, phase):
        if self.down_at is not None:
            elapsed = (time.monotonic() - self.down_at) * 1000
            print(f'JIIOV timing {phase} down_ms={elapsed:.3f}', file=sys.stderr)

    def wait_event(self):
        if b'\n' in self.pending:
            return
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            if self.control_fd is not None:
                selector.register(self.control_fd, selectors.EVENT_READ)
            selector.select(0.1)

    def wait_down(self):
        while not self.down or self.control_request or self.suspended or self.suspend_rejected:
            self.poll()
            if not self.down or self.control_request or self.suspended or self.suspend_rejected:
                self.wait_event()

    def cancelled_finish(self):
        self.cancelling = True
        deadline = time.monotonic() + 4
        while self.process.poll() is None:
            self.poll()
            if time.monotonic() >= deadline:
                raise TimeoutError('Fingerprint cancellation cleanup timed out')
            self.wait_event()
        self.poll()
        if not self.stopped or self.process.returncode != 2 or self.pending:
            raise RuntimeError('Fingerprint cancellation cleanup was not confirmed')

    def wait_ready(self):
        while not self.ready:
            self.poll()
            if not self.ready:
                self.wait_event()

    def finish(self):
        self.poll()
        self.finishing = True
        self.process.stdin.write(b'finish\n')
        self.process.stdin.flush()
        deadline = time.monotonic() + 4
        while self.process.poll() is None:
            self.poll()
            if time.monotonic() >= deadline:
                raise TimeoutError('Fingerprint illumination cleanup timed out')
            self.wait_event()
        self.poll()
        if self.process.returncode != 0 or not self.stopped or self.pending:
            raise RuntimeError('Fingerprint illumination cleanup was not confirmed')


class CaptureGate:
    """Own the single-use capture permission; preparation may run beforehand."""
    def __init__(self, presentation):
        self.presentation = presentation
        self.read_fd, self.write_fd = os.pipe2(os.O_CLOEXEC)
        self.sent = False

    def close_read(self):
        if self.read_fd >= 0:
            os.close(self.read_fd)
            self.read_fd = -1

    def poll(self):
        # Drain every pending contact record before granting permission: a
        # READY followed by UP in one read must not authorize capture.
        self.presentation.poll()
        if self.presentation.ready and not self.sent:
            if os.write(self.write_fd, b'\x01') != 1:
                raise RuntimeError('Capture permission write failed')
            os.close(self.write_fd)
            self.write_fd = -1
            self.sent = True
            self.presentation.timing('capture-authorized')

    def close(self):
        self.close_read()
        if self.write_fd >= 0:
            os.close(self.write_fd)
            self.write_fd = -1


def run(operation, identifier):
    if os.geteuid() != 0 or 'ZY22MMG59D' not in Path('/proc/bootconfig').read_text():
        raise RuntimeError('This experimental worker requires the validated Roadstr device')
    presentation = PRESENTATION if operation == 'verify' and os.environ.get('JIIOV_DENIAL_UI') == '1' else CLIENT
    for path in (Path(PROBE), Path(presentation), BASE / 'ancapp64.elf', STORAGE):
        metadata = path.lstat()
        if metadata.st_uid != 0 or metadata.st_mode & 0o022 or stat.S_ISLNK(metadata.st_mode):
            raise RuntimeError('Untrusted worker installation')
    if STORAGE.stat().st_mode & 0o077:
        raise RuntimeError('Template directory must be private')
    lock = os.open(RUNTIME / 'lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return run_locked(operation, identifier)
    finally:
        os.close(lock)


def run_locked(operation, identifier):
    modes = {'probe': '--identify-with-files', 'open': '--list-with-files',
             'list': '--list-with-files', 'enroll': '--enroll-with-files',
             'verify': '--verify-with-files', 'delete': '--delete-with-files'}
    command = [PROBE, modes[operation], str(BASE / 'ancapp64.elf'), str(BASE / 'calibration')]
    if operation != 'probe':
        command.append(str(STORAGE))
    if identifier is not None:
        command.append(str(identifier))
    optical = operation in ('enroll', 'verify')
    denial = operation == 'verify' and os.environ.get('JIIOV_DENIAL_UI') == '1'
    records = Records(operation)
    saved = None
    native = ui = None
    presentation = None
    gate = None
    progress = -1
    with tempfile.TemporaryDirectory(prefix='jiiov-fprint-', dir=RUNTIME) as temporary:
        progress_path = Path(temporary) / 'progress'
        try:
            environment = {key: value for key, value in os.environ.items()
                           if key not in ('LD_PRELOAD', 'LD_LIBRARY_PATH', 'ANC_DIAGNOSTIC_PROGRESS_FD', 'ANC_CAPTURE_READY_FD')}
            if denial:
                presentation = Presentation(environment)
                ui = presentation.process
                print('JFP1 power 0', flush=True)
                presentation.wait_down()
                gate = CaptureGate(presentation)
                command[1] = '--verify-gated-with-files'
                environment['ANC_CAPTURE_READY_FD'] = str(gate.read_fd)
                print('JFP1 power 1', flush=True)
                presentation.timing('native-start')
            elif optical:
                if os.environ.get('JIIOV_DIAGNOSTIC_UI') != '1' or DPMS.read_text().strip() != 'On':
                    raise RuntimeError('Diagnostic capture requires an awake, unlocked test session')
                progress = os.open(progress_path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
                os.write(progress, b'PLACE 20\n')
                ui = subprocess.Popen([CLIENT, '--target', str(progress_path)], stdout=subprocess.PIPE,
                                      stderr=subprocess.STDOUT, env={**environment,
                                      'XDG_RUNTIME_DIR': str(RUNTIME), 'WAYLAND_DISPLAY': 'wayland-1'})
                ready = False
                for line in lines(ui, 5):
                    print(f'JIIOV diagnostic surface: {line}', file=sys.stderr)
                    if line.startswith('First frame callback received;'):
                        ready = True
                        break
                if not ready or ui.poll() is not None or DPMS.read_text().strip() != 'On':
                    raise RuntimeError('Diagnostic surface did not become ready')
                saved = BRIGHTNESS.read_text()
                BRIGHTNESS.write_text('16180\n')
                environment['ANC_DIAGNOSTIC_PROGRESS_FD'] = str(progress)
            native = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                      env=environment, pass_fds=(gate.read_fd,) if gate else ((progress,) if progress >= 0 else ()))
            if gate:
                gate.close_read()
            for line in lines(native, 9 if denial else 125,
                              gate.poll if gate else None,
                              presentation.process.stdout if presentation else None):
                if optical and not denial and (ui.poll() is not None or DPMS.read_text().strip() != 'On'):
                    raise RuntimeError('Diagnostic presentation was lost')
                records.feed(line)
                if denial and re.fullmatch(r'JTIM1 (firmware-loaded|sensor-ready|templates-ready|prepared|capture-start|capture-end|extract-end|compare-end|cleanup-done) [0-9]+', line):
                    print('JIIOV native timing ' + line, file=sys.stderr)
                if denial and line.startswith(('Verification extraction status=', 'Verification compare status=')):
                    # Status only; no pixels, templates, or matched identifier.
                    print('JIIOV ' + line.split(' matched_id=', 1)[0], file=sys.stderr)
            returncode = native.wait(timeout=5)
            result = records.result(returncode)
            if presentation:
                presentation.finish()
                presentation.timing('cleanup-complete')
                print('JFP1 power 0', flush=True)
        except ContactLost:
            # An early lift is a retry, not a reader failure. Do not return it
            # until both firmware and illumination have confirmed cleanup.
            if gate:
                gate.close()
            if native is not None:
                native.terminate()
                for line in lines(native, 5):
                    records.feed(line)
                native.wait(timeout=1)
                if not records.unloaded:
                    raise RuntimeError('Cancelled firmware cleanup was not confirmed')
            presentation.cancelled_finish()
            print('JFP1 power 0', flush=True)
            result = (3, 0)
        finally:
            retained_path(os.getpid()).unlink(missing_ok=True)
            if gate:
                gate.close()
            # Restore light first, even if native cleanup has stalled.
            try:
                if saved is not None:
                    BRIGHTNESS.write_text(saved)
            finally:
                try:
                    stop(ui)
                finally:
                    try:
                        stop(native)
                    finally:
                        if progress >= 0:
                            os.close(progress)
                        for process in (ui, native):
                            if process is not None:
                                for stream in (process.stdin, process.stdout, process.stderr):
                                    if stream is not None:
                                        stream.close()
        if interrupted:
            raise InterruptedError('Cancelled')
        if operation == 'list':
            for value in records.ids:
                print(f'JFP1 item {value}', flush=True)
        return result


def main():
    args = sys.argv[1:]
    if args == ['resume-retained']:
        try:
            return 0 if retained_session() else 1
        except (OSError, ValueError, subprocess.SubprocessError):
            return 1
    if not args or args[0] not in ('probe', 'open', 'list', 'enroll', 'verify', 'delete'):
        return 2
    identifier = None
    if args[0] == 'delete':
        if len(args) != 2 or not re.fullmatch(r'[1-9][0-9]{0,9}', args[1]):
            return 2
        identifier = int(args[1])
        if identifier > 0xffffffff:
            return 2
    elif len(args) != 1:
        return 2
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    try:
        result, identifier = run(args[0], identifier)
        print(f'JFP1 result {result} {identifier}', flush=True)
        return 0
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f'JIIOV worker: {error}', file=sys.stderr)
        print('JFP1 result 2 0', flush=True)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
