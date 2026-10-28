#!/usr/bin/python3
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Root-only UFF1 bridge. Secure operations and contact are separate children.

No result is trusted until both children have finished their respective cleanup.
The native child cannot commit an enrollment until contact has withdrawn.

One helper serves every UFF1 phone: installed as uff-fprint-helper on the
Aston, whose optical sensor needs Denial to light it, and as
goodix-fprint-helper on the NX809J, whose ultrasonic sensor needs only the
touchscreen to say a finger is on it.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import sys
import time

STOP = False

# Each phone: its configuration, its lock, the child that reports contact,
# and whether its worker stays resident between requests.
PHONES = {
    'aston': {'config': '/etc/aston-fingerprint/backend.json',
              'lock': '/run/lock/aston-fingerprint.lock',
              'contact': 'presentation', 'resident': True},
    'nx809j': {'config': '/etc/nx809j-fingerprint/backend.json',
               'lock': '/run/lock/nx809j-fingerprint.lock',
               'contact': 'contact', 'resident': False},
}


class ProtocolError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProtocolError(message)


def emit(line):
    print('UFF1 ' + line, flush=True)


def protected(path, digest=None, limit=32*1024*1024):
    require(os.path.isabs(path), 'absolute artifact path required')
    # Every ancestor must also resist replacement by an unprivileged process.
    for parent in Path(path).parents:
        s = parent.lstat()
        require(stat.S_ISDIR(s.st_mode) and s.st_uid == 0 and not s.st_mode & 0o022,
                'unprotected artifact directory')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        s = os.fstat(stream.fileno())
        require(stat.S_ISREG(s.st_mode) and s.st_uid == 0 and s.st_nlink == 1
                and not s.st_mode & 0o022 and 0 < s.st_size <= limit,
                'unprotected artifact')
        data = stream.read(limit + 1)
    require(len(data) == s.st_size, 'artifact changed while reading')
    if digest is not None:
        require(re.fullmatch('[0-9a-f]{64}', digest) and
                hashlib.sha256(data).hexdigest() == digest, 'artifact checksum mismatch')
    return data


def phone():
    boot = Path('/proc/bootconfig').read_text()
    require('androidboot.slot_suffix = "_b"' in boot, 'wrong slot')
    model = Path('/proc/device-tree/compatible').read_bytes().split(b'\0')[0]
    if model == b'oneplus,aston':
        require('androidboot.serialno = "f683d9c9"' in boot, 'wrong device')
        return 'aston'
    require(model == b'nubia,nx809j', 'unsupported phone')
    return 'nx809j'


def configuration():
    require(os.geteuid() == 0, 'root required')
    name = phone()
    config = json.loads(protected(PHONES[name]['config'], limit=8192))
    require(config['format'] == 1, 'unsupported configuration')
    for entry in ('worker', 'firmware', PHONES[name]['contact']):
        protected(config[entry]['path'], config[entry]['sha256'])
    return dict(config, phone=PHONES[name])


def compositor_environment():
    p = subprocess.run(['systemctl', 'show', 'denial.service', '--property=MainPID',
                        '--property=User', '--property=ActiveState'],
                       check=True, capture_output=True, text=True, timeout=3)
    fields = dict(line.split('=', 1) for line in p.stdout.splitlines())
    pid = int(fields['MainPID'])
    require(pid > 1 and fields['User'] == 'logix' and fields['ActiveState'] == 'active',
            'Denial service unavailable')
    require(Path(f'/proc/{pid}').stat().st_uid == 1001, 'wrong compositor owner')
    raw = Path(f'/proc/{pid}/environ').read_bytes()
    env = dict(x.decode().split('=', 1) for x in raw.split(b'\0') if b'=' in x)
    runtime = env.get('XDG_RUNTIME_DIR', '/run/user/1001')
    # The listener is created after exec, so /proc/environ need not contain
    # WAYLAND_DISPLAY. Resolve the sole live socket, then the C client pins
    # its SO_PEERCRED to this exact system-managed compositor PID.
    sockets = [p for p in Path(runtime).glob('wayland-*')
               if re.fullmatch(r'wayland-[0-9]+', p.name)
               and stat.S_ISSOCK(p.lstat().st_mode) and p.lstat().st_uid == 1001]
    require(len(sockets) == 1, 'ambiguous compositor socket')
    display = sockets[0].name
    require(runtime == '/run/user/1001' and re.fullmatch(r'wayland-[0-9]+', display),
            'unexpected compositor socket')
    return {'PATH': '/usr/bin:/bin', 'XDG_RUNTIME_DIR': runtime,
            'WAYLAND_DISPLAY': display, 'UFF_DENIAL_PID': str(pid),
            'UFF_SUSPEND_PROTOCOL': '1'}


class Contact:
    """Strict presentation state: only a clean withdrawal authorizes finish."""
    def __init__(self):
        self.state = 'starting'
        self.stopped = False
        self.suspended = False
        self.pending = None

    def record(self, line):
        if line == 'DFP1 waiting' and self.state == 'starting':
            self.state = 'waiting'
        elif line == 'DFP1 down 1' and self.state == 'waiting' and not self.suspended:
            self.state = 'down'
        elif line == 'DFP1 ready 1' and self.state == 'presenting':
            self.state = 'ready'
        elif line == 'DFP1 stopped 1 0' and self.state == 'finishing' and not self.stopped:
            self.stopped = True
        elif line == 'DFP1 suspended' and self.pending == 'suspend' and self.state == 'waiting':
            self.pending = None
            self.suspended = True
            emit('suspended')
        elif line == 'DFP1 resumed' and self.pending == 'resume' and self.state == 'waiting':
            self.pending = None
            self.suspended = False
            emit('resumed')
        elif line == 'DFP1 suspend-busy' and self.pending == 'suspend':
            self.pending = None
            emit('suspend-busy')
        else:
            raise ProtocolError('presentation cancelled or invalid transition')

    def present(self):
        require(self.state == 'down' and not self.stopped, 'no live contact for presentation')
        self.state = 'presenting'

    def finish(self):
        require(self.state == 'ready' and not self.stopped, 'contact was not ready')
        self.state = 'finishing'

    def completed(self, returncode):
        require(self.state == 'finishing' and self.stopped and returncode == 0,
                'presentation cleanup unconfirmed')


class Native:
    def __init__(self, operation, resident=False):
        self.operation = operation
        self.resident = resident
        self.parked = False
        self.state = 'starting'
        self.cleaned = False
        self.result = None
        self.sample = None
        self.ids = []

    def record(self, line):
        if self.result is not None:
            raise ProtocolError('record after native result')
        if line == 'UFW1 standby' and self.state == 'starting' and self.operation == 'verify':
            self.state = 'standby'
        elif line.startswith('UFW1 challenge ') and self.operation == 'enroll' and self.state == 'starting':
            # The NX809J's worker enrolls only with an authorization token for
            # this challenge from the PIN's credential; nothing supplies one yet.
            raise ProtocolError('enrollment needs an authorization token; none is wired')
        elif line == 'UFW1 prepared' and ((self.operation == 'enroll' and self.state == 'starting') or (self.operation == 'verify' and self.state == 'activating')):
            require(self.operation in ('enroll', 'verify'), 'unexpected optical state')
            self.state = 'prepared'
        elif line.startswith('UFW1 sample ') and self.state == 'capturing':
            require(re.fullmatch(r'UFW1 sample (0|1|3) [0-9]{1,3} [0-9]{1,10} [0-9]{1,3}', line),
                    'invalid sample record')
            code, remaining, identifier, total = map(int, line.split()[2:])
            require(remaining <= 100 and total <= 101 and identifier <= 0xffffffff,
                    'sample values out of range')
            if self.operation == 'verify':
                require((code == 0 and identifier != 0) or (code != 0 and identifier == 0),
                        'invalid verification identifier')
            else:
                require(code != 1 and (code != 0 or total > 0), 'invalid enrollment sample')
            self.sample = (code, remaining, identifier, total)
            self.state = 'sample'
        elif line == 'UFW1 idle' and self.state == 'finishing' and self.operation == 'enroll':
            require(self.sample[0] != 0 or self.sample[1] != 0, 'idle after final sample')
            self.state = 'prepared'
        elif line == 'UFW1 cleaned' and not self.cleaned and not self.resident and self.operation in ('enroll', 'verify'):
            self.cleaned = True
        elif line == 'UFW1 parked' and self.resident and not self.cleaned and self.state == 'finishing':
            self.cleaned = self.parked = True
        elif re.fullmatch(r'UFF1 item [0-9]{1,10}', line) and self.operation == 'list':
            identifier = int(line.split()[2])
            require(0 < identifier <= 0xffffffff and identifier not in self.ids and len(self.ids) < 5,
                    'invalid or duplicate template identifier')
            self.ids.append(identifier)
        elif re.fullmatch(r'UFF1 result (0|1|3) [0-9]{1,10}', line):
            code, identifier = map(int, line.split()[2:])
            require(identifier <= 0xffffffff, 'invalid result identifier')
            if self.operation in ('verify', 'enroll'):
                require(self.cleaned and self.state == 'finishing' and self.sample is not None,
                        'result before contact and secure cleanup')
                require((code, identifier) == (self.sample[0], self.sample[2]),
                        'result differs from captured sample')
                if self.operation == 'enroll':
                    require(code == 0 and identifier and self.sample[1] == 0,
                            'incomplete enrollment')
            else:
                require(code == 0 and identifier == 0, 'unexpected passive result')
            self.result = (code, identifier)
        else:
            raise ProtocolError('invalid native record or transition')

    def activate(self):
        require(self.operation == 'verify' and self.state == 'standby' and not self.cleaned, 'native child not in standby')
        self.state = 'activating'

    def capture(self):
        require(self.state == 'prepared' and not self.cleaned, 'native child not prepared')
        self.state = 'capturing'

    def finish(self):
        require(self.state == 'sample' and not self.cleaned, 'native child has no sample')
        self.state = 'finishing'

    def completed(self, returncode):
        require(returncode == 0 and self.result is not None, 'native cleanup or operation failed')
        return self.result


class Process:
    def __init__(self, argv, state, env=None):
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=None, env=env, bufsize=0, close_fds=True)
        self.state = state
        self.buffer = b''
        self.eof = False
        self.bytes = 0

    def send(self, line):
        self.p.stdin.write((line + '\n').encode('ascii'))

    def stop(self):
        if self.p.poll() is None:
            self.p.terminate()
        try:
            self.p.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.p.kill()
            self.p.wait(timeout=3)
            raise ProtocolError('forced child termination; cleanup unconfirmed')
        finally:
            self.p.stdin.close()
            self.p.stdout.close()


class Bridge:
    def __init__(self, config, operation, argument=None):
        self.config = config
        self.operation = operation
        self.argument = argument
        self.native = None
        self.presentation = None
        self.children = []
        self.sel = selectors.DefaultSelector()
        self.sel.register(sys.stdin.fileno(), selectors.EVENT_READ, 'control')
        self.control = b''

    def start_native(self):
        args = [self.config['worker']['path'], self.operation, self.config['firmware']['path']]
        if self.argument is not None:
            args.append(self.argument)
        resident = self.config['phone']['resident']
        env = dict(os.environ, UFF_RESIDENT='1') if resident else dict(os.environ)
        self.native = Process(args, Native(self.operation, resident=resident), env)
        self.children.append(self.native)
        self.sel.register(self.native.p.stdout, selectors.EVENT_READ, self.native)

    def start_contact(self):
        name = self.config['phone']['contact']
        if name == 'presentation':
            env = compositor_environment()
            env['UFF_DEFER_PRESENT'] = '1'
        else:
            env = {'PATH': '/usr/bin:/bin', 'UFF_SUSPEND_PROTOCOL': '1'}
        self.presentation = Process([self.config[name]['path']], Contact(), env)
        self.children.append(self.presentation)
        self.sel.register(self.presentation.p.stdout, selectors.EVENT_READ, self.presentation)

    def control_record(self, line):
        require(line in ('suspend', 'resume'), 'invalid service control request')
        p = self.presentation
        if self.operation != 'verify' or (self.native is not None and self.native.state.state != 'standby') or p is None or p.eof or p.state.state != 'waiting':
            require(line == 'suspend', 'resume while capture active')
            emit('suspend-busy')
            return
        require(p.state.pending is None and (line == 'resume') == p.state.suspended,
                'invalid sleep transition')
        p.state.pending = line
        p.send(line)

    def pump(self, timeout):
        require(not STOP, 'cancelled')
        events = self.sel.select(min(timeout, 0.2))
        # A simultaneous UP or presentation failure always precedes a sample.
        events.sort(key=lambda e: 0 if e[0].data is self.presentation else 1)
        for key, _ in events:
            owner = key.data
            data = os.read(key.fd, 1024)
            if owner == 'control':
                require(data, 'service control disconnected')
                self.control += data
                require(len(self.control) <= 128, 'oversized service control')
                while b'\n' in self.control:
                    line, self.control = self.control.split(b'\n', 1)
                    self.control_record(line.decode('ascii'))
                continue
            owner.bytes += len(data)
            require(owner.bytes <= 16384, 'excessive child output')
            if not data:
                self.sel.unregister(key.fileobj)
                owner.eof = True
                require(not owner.buffer, 'truncated child record')
                continue
            owner.buffer += data
            require(len(owner.buffer) <= 2048, 'oversized child record')
            while b'\n' in owner.buffer:
                line, owner.buffer = owner.buffer.split(b'\n', 1)
                require(len(line) <= 80, 'oversized protocol line')
                owner.state.record(line.decode('ascii'))
        require(not STOP, 'cancelled')
        # Unexpected exits must not leave the other child running indefinitely.
        for child in (self.presentation, self.native):
            if child is not None and child.eof and child.p.poll() is not None:
                child.state.completed(child.p.returncode)

    def wait(self, predicate, timeout):
        deadline = time.monotonic() + timeout
        while not predicate():
            require(time.monotonic() < deadline, 'operation timeout')
            self.pump(deadline - time.monotonic())

    def joined(self, child):
        return child.eof and child.p.poll() is not None

    def run(self):
        if self.operation not in ('enroll', 'verify'):
            self.start_native()
            self.wait(lambda: self.joined(self.native), 30)
            code, identifier = self.native.state.completed(self.native.p.returncode)
            for item in self.native.state.ids:
                emit(f'item {item}')
            emit(f'result {code} {identifier}')
            return
        if self.operation == 'verify':
            self.start_native()
        if self.operation == 'enroll':
            self.start_native()
            self.wait(lambda: self.native.state.state == 'prepared', 20)
        stages = 0
        for _ in range(100):
            self.start_contact()
            if self.operation == 'verify':
                self.wait(lambda: self.native.state.state == 'standby', 20)
                # A resident worker waits in signed-firmware sleep; a
                # one-shot one waits with the sensor's power off.
                emit('power 2' if self.native.state.resident else 'power 0')
            self.wait(lambda: self.presentation.state.state == 'down', 120 if self.operation == 'enroll' else 86400)
            if self.operation == 'verify':
                self.wait(lambda: self.native.state.state == 'standby', 15)
                emit('power 1')
                self.native.state.activate()
                self.native.send('activate')
            # Panel wake and secure sensor initialization are independent. Start
            # both after the same physical DOWN, then require BOTH readiness
            # conditions before capture. Early lift still aborts through pump.
            ready_start = time.monotonic()
            self.presentation.state.present()
            self.presentation.send('present')
            self.wait(lambda: self.presentation.state.state == 'ready' and
                      self.native.state.state == 'prepared', 15)
            print(f'UFF timing contact_to_ready_ms={(time.monotonic()-ready_start)*1000:.3f}', file=sys.stderr)
            self.native.state.capture()
            self.native.send('capture')
            self.wait(lambda: self.native.state.state == 'sample', 25)
            sample = self.native.state.sample
            self.presentation.state.finish()
            self.presentation.send('finish')
            self.wait(lambda: self.joined(self.presentation), 5)
            self.presentation.state.completed(self.presentation.p.returncode)
            self.native.state.finish()
            self.native.send('finish')
            final = self.operation == 'verify' or (sample[0] == 0 and sample[1] == 0)
            if final:
                self.wait(lambda: self.joined(self.native), 20)
                code, identifier = self.native.state.completed(self.native.p.returncode)
                if self.operation == 'verify':
                    emit('power 2' if self.native.state.parked else 'power 0')
                if self.operation == 'enroll':
                    emit('progress 20')
                emit(f'result {code} {identifier}')
                return
            self.wait(lambda: self.native.state.state == 'prepared', 10)
            if sample[0] == 3:
                emit('retry')
            if sample[0] == 0:
                progress = min(19, 20 * max(0, sample[3] - sample[1]) // sample[3])
                if progress > stages:
                    stages = progress
                    emit(f'progress {progress}')
        raise ProtocolError('contact limit exceeded')

    def close(self):
        # Withdraw presentation before stopping the secure child. Never emit a
        # successful result from this emergency path, even after graceful exit.
        failed = False
        for child in sorted(self.children, key=lambda c: 0 if isinstance(c.state, Contact) else 1):
            try:
                child.stop()
            except (OSError, ProtocolError) as e:
                print(str(e), file=sys.stderr)
                failed = True
        self.sel.close()
        return not failed


def stop(_sig, _frame):
    global STOP
    STOP = True


def main():
    require(len(sys.argv) in (2, 3), 'invalid arguments')
    operation = sys.argv[1]
    require(operation in ('probe', 'open', 'list', 'delete', 'enroll', 'verify'), 'unsupported operation')
    argument = sys.argv[2] if len(sys.argv) == 3 else None
    require((operation == 'delete') == (argument is not None), 'invalid operation arguments')
    if argument is not None:
        require(re.fullmatch(r'[1-9][0-9]{0,9}', argument) and int(argument) <= 0xffffffff,
                'invalid template identifier')
    config = configuration()
    lock = os.open(config['phone']['lock'], os.O_CREAT | os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        bridge = Bridge(config, operation, argument)
        try:
            bridge.run()
        finally:
            require(bridge.close(), 'child cleanup failed')
    finally:
        os.close(lock)


if __name__ == '__main__':
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, stop)
    signal.signal(signal.SIGPIPE, signal.SIG_IGN)
    try:
        main()
    except (ProtocolError, OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f'UFF service: {error}', file=sys.stderr)
        sys.exit(2)
