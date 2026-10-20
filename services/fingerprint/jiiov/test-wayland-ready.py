#!/usr/bin/env python3
"""Synthetic local tests; no real device, compositor or Wayland connection."""
import importlib.util
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'ready', Path(__file__).with_name('jiiov-wayland-ready.py'))
ready = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ready)


class Fixture:
    def __init__(self):
        self.temp = tempfile.TemporaryDirectory(prefix='t37-gate-')
        self.base = Path(self.temp.name)
        self.proc = self.base / 'proc'
        self.process = self.proc / '321'
        (self.process / 'fd').mkdir(parents=True)
        (self.process / 'net').mkdir()
        (self.process / 'exe').symlink_to('/synthetic/deniald')
        (self.process / 'root').symlink_to('/')
        fields = ['S'] + ['0'] * 18 + ['1234']
        (self.process / 'stat').write_text('321 (odd ) comm) ' + ' '.join(fields))
        self.path = self.base / 'wayland-1'
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(str(self.path))
        self.listener.listen(8)
        (self.process / 'fd/8').symlink_to('socket:[9001]')
        self.row()
        self.uid = ready.SESSION_UID
        actual_lstat = Path.lstat

        def lstat(path):
            result = actual_lstat(path)
            if path == self.path:
                values = list(result)
                values[4] = self.uid  # Simulate session ownership without chown.
                return os.stat_result(values)
            return result

        self.owner = patch.object(Path, 'lstat', lstat)
        self.owner.start()

    def row(self, inode='9001', flags='00010000', kind='0001', path=None):
        (self.process / 'net/unix').write_text(
            'Num RefCount Protocol Flags Type St Inode Path\n'
            f'000: 2 0 {flags} {kind} 01 {inode} {path or self.path}\n')

    def inspect(self):
        return ready.inspect_listener(321, self.proc, self.path)

    def close(self):
        self.owner.stop()
        self.listener.close()
        self.temp.cleanup()


class ListenerTests(unittest.TestCase):
    def setUp(self):
        self.f = Fixture()
        self.addCleanup(self.f.close)

    def test_current_listener_succeeds(self):
        identity = self.f.inspect()
        self.assertEqual(identity[:2], (321, '1234'))
        self.assertEqual(identity[3], self.f.path.stat().st_ino)
        self.assertEqual(identity[4], '9001')

    def test_stale_socket_before_current_pid_listener_is_not_ready(self):
        # T39: pathname exists but current PID does not own its old listener.
        (self.f.process / 'fd/8').unlink()
        with self.assertRaisesRegex(ValueError, 'does not own'):
            self.f.inspect()

    def test_listener_owned_by_different_process_rejected(self):
        self.f.row(inode='8999')
        with self.assertRaisesRegex(ValueError, 'does not own'):
            self.f.inspect()

    def test_connected_socket_not_listener_rejected(self):
        self.f.row(flags='00000000')
        with self.assertRaises(ValueError):
            self.f.inspect()

    def test_datagram_socket_rejected(self):
        self.f.row(kind='0002')
        with self.assertRaises(ValueError):
            self.f.inspect()

    def test_wrong_path_or_deleted_listener_rejected(self):
        self.f.row(path=str(self.f.path) + '//deleted')
        with self.assertRaises(ValueError):
            self.f.inspect()

    def test_wrong_namespace_inode_rejected(self):
        (self.f.process / 'root').unlink()
        (self.f.process / 'root').symlink_to(self.f.base / 'namespace')
        alternate = self.f.base / 'namespace' / str(self.f.path).lstrip('/')
        alternate.parent.mkdir(parents=True)
        with socket.socket(socket.AF_UNIX) as other:
            other.bind(str(alternate))
            with self.assertRaisesRegex(ValueError, 'inode differs'):
                self.f.inspect()

    def test_wrong_executable_rejected(self):
        (self.f.process / 'exe').unlink()
        (self.f.process / 'exe').symlink_to('/synthetic/unrelated')
        with self.assertRaisesRegex(ValueError, 'not deniald'):
            self.f.inspect()

    def test_foreign_owner_socket_rejected(self):
        self.f.uid = 123
        with self.assertRaisesRegex(ValueError, 'session-owned'):
            self.f.inspect()

    def test_plain_file_not_socket(self):
        self.f.path.unlink()
        self.f.path.write_text('placeholder')
        with self.assertRaisesRegex(ValueError, 'session-owned socket'):
            self.f.inspect()

    def test_symlink_not_socket(self):
        self.f.path.unlink()
        self.f.path.symlink_to(self.f.process / 'stat')
        with self.assertRaisesRegex(ValueError, 'session-owned socket'):
            self.f.inspect()

    def test_pid_reuse_during_check_rejected(self):
        with patch.object(ready, 'process_start', side_effect=['1234', '5678']):
            with self.assertRaisesRegex(ValueError, 'identity changed'):
                self.f.inspect()

    def test_path_replacement_during_check_rejected(self):
        initial = self.f.path.stat()
        replacement = list(initial)
        replacement[1] += 1  # A different filesystem inode, same other metadata.
        replacement[4] = ready.SESSION_UID
        first = list(initial)
        first[4] = ready.SESSION_UID
        actual_lstat = Path.lstat
        calls = [0]

        def lstat(path):
            if path != self.f.path:
                return actual_lstat(path)
            calls[0] += 1
            return os.stat_result(first if calls[0] == 1 else replacement)

        with patch.object(Path, 'lstat', lstat):
            with self.assertRaisesRegex(ValueError, 'identity changed'):
                self.f.inspect()


class WaitTests(unittest.TestCase):
    def wait(self, state, inspect, timeout=.2):
        now = [0.0]

        def sleep(delay):
            now[0] += delay

        return ready.wait_ready(timeout, state, inspect, lambda: now[0], sleep)

    def test_delayed_listener_succeeds_without_service_action(self):
        attempts = [0]

        def inspect(pid):
            attempts[0] += 1
            if attempts[0] < 3:
                raise ValueError('old socket has no current owner')
            return pid, '1234', 51, 504, '9001'

        result = self.wait(lambda: (321, 'active', 'running'), inspect)
        self.assertEqual(result[3], 504)
        self.assertEqual(attempts[0], 3)

    def test_timeout_does_not_restart_or_stop_service(self):
        with patch.object(ready.subprocess, 'run') as command:
            with self.assertRaisesRegex(TimeoutError, 'not active/running'):
                self.wait(lambda: (0, 'inactive', 'dead'), lambda pid: None)
            command.assert_not_called()

    def test_mainpid_changes_between_snapshots_rejected(self):
        count = [0]

        def state():
            count[0] += 1
            return 321 + (count[0] % 2), 'active', 'running'

        with self.assertRaisesRegex(TimeoutError, 'MainPID/service state changed'):
            self.wait(state, lambda pid: (pid, '1234', 51, 504, '9001'))

    def test_service_inspection_only_uses_show(self):
        output = subprocess.CompletedProcess([], 0,
            'MainPID=321\nActiveState=active\nSubState=running\n')
        with patch.object(ready.subprocess, 'run', return_value=output) as run:
            self.assertEqual(ready.service_state(), (321, 'active', 'running'))
        self.assertEqual(run.call_args.args[0][1], 'show')
        self.assertEqual(run.call_args.kwargs['timeout'], 1)

    def test_late_snapshot_cannot_claim_readiness(self):
        now = [0.0]

        def inspect(pid):
            now[0] = 1
            return pid, '1234', 51, 504, '9001'

        with self.assertRaisesRegex(TimeoutError, 'deadline'):
            ready.wait_ready(.2, lambda: (321, 'active', 'running'), inspect,
                             lambda: now[0], lambda delay: None)


if __name__ == '__main__':
    unittest.main(verbosity=2)
