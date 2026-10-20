#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Authentication outcome checks using captured status traces and bad records."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

PW = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('helper', PW / 'jiiov-fprint-helper.py')
helper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(helper)
UNLOAD = 'unload ancapp64: ioctl=0 errno=0 result=0'


class Results(unittest.TestCase):
    @staticmethod
    def close_process(process):
        helper.stop(process)
        for stream in (process.stdin, process.stdout, process.stderr):
            if stream:
                stream.close()

    def test_twenty_samples_do_not_mean_saved(self):
        records = helper.Records('enroll')
        trace = PW / 'traces/target-enrollment-live.log'
        with contextlib.redirect_stdout(io.StringIO()):
            for line in trace.read_text().splitlines():
                records.feed(line)
        self.assertEqual(records.remaining, 0)
        self.assertEqual(records.final, (407, 0))
        with self.assertRaises(RuntimeError):
            records.result(0)  # Even an incorrect process exit cannot mask this.

    def test_match_requires_loaded_identifier_and_clean_exit(self):
        for identifier, returncode, unload, good in [
                (42, 0, True, True), (43, 0, True, False),
                (42, 1, True, False), (42, 0, False, False)]:
            with self.subTest(identifier=identifier, returncode=returncode, unload=unload):
                records = helper.Records('verify')
                records.feed('Isolated template ID=42')
                records.feed(f'Verification compare status=500 matched_id={identifier}')
                if unload:
                    records.feed(UNLOAD)
                if good:
                    self.assertEqual(records.result(returncode), (0, 42))
                else:
                    with self.assertRaises(RuntimeError):
                        records.result(returncode)

    def presentation(self, script, control=False):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        worker = Path(temporary.name) / 'presentation'
        worker.write_text('#!/usr/bin/python3\nimport sys,time\n' + script)
        worker.chmod(0o700)
        environment = os.environ.copy()
        environment.pop('JIIOV_SUSPEND_PROTOCOL', None)
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(helper, 'PRESENTATION', str(worker)))
            if control:
                reader, writer = os.pipe()
                stream = os.fdopen(reader, 'rb', buffering=0)
                self.addCleanup(stream.close)
                self.addCleanup(os.close, writer)
                stack.enter_context(patch.object(helper.sys, 'stdin', stream))
                runtime = patch.object(helper, 'RUNTIME', Path(temporary.name))
                runtime.start()
                self.addCleanup(runtime.stop)
                environment['JIIOV_SUSPEND_PROTOCOL'] = '1'
            presentation = helper.Presentation(environment)
            if control:
                presentation.control_writer = writer
        self.addCleanup(self.close_process, presentation.process)
        return presentation

    def await_presentation(self, presentation, predicate):
        deadline = time.monotonic() + 2
        while not predicate():
            presentation.poll()
            self.assertLess(time.monotonic(), deadline)
            presentation.wait_event()

    def test_suspend_retains_listener_and_releases_contact_after_resume(self):
        presentation = self.presentation(
            "print('DFP1 waiting',flush=True)\n"
            "assert sys.stdin.readline()=='suspend\\n'\n"
            "print('DFP1 suspended',flush=True)\n"
            "assert sys.stdin.readline()=='resume\\n'\n"
            "print('DFP1 resumed\\nDFP1 down 1',flush=True)\ntime.sleep(5)\n", control=True)
        pid = presentation.process.pid
        with contextlib.redirect_stdout(io.StringIO()) as output:
            os.write(presentation.control_writer, b'su')
            presentation.poll()
            self.assertFalse(presentation.suspended)
            os.write(presentation.control_writer, b'spend\n')
            self.await_presentation(presentation, lambda: presentation.suspended)
            self.assertFalse(presentation.down)
            self.assertTrue(helper.retained_path(os.getpid()).exists())
            record = helper.json.loads(helper.retained_path(os.getpid()).read_text())
            self.assertEqual(record['presentation'], helper.process_identity(pid))
            self.assertEqual(record['worker'], helper.process_identity(os.getpid()))
            os.write(presentation.control_writer, b'resume\n')
            self.await_presentation(presentation, lambda: presentation.down)
            self.assertFalse(presentation.suspended)
            self.assertIsNone(presentation.control_request)
            self.assertFalse(helper.retained_path(os.getpid()).exists())
            self.assertEqual(presentation.process.pid, pid)
            self.assertEqual(output.getvalue().splitlines(), ['JFP1 suspended', 'JFP1 resumed'])

    def test_suspend_refuses_active_contact_without_marking_retained(self):
        presentation = self.presentation(
            "print('DFP1 down 1',flush=True)\n"
            "assert sys.stdin.readline()=='suspend\\n'\n"
            "print('DFP1 suspend-busy',flush=True)\ntime.sleep(5)\n", control=True)
        self.await_presentation(presentation, lambda: presentation.down)
        with contextlib.redirect_stdout(io.StringIO()) as output:
            os.write(presentation.control_writer, b'suspend\n')
            self.await_presentation(presentation, lambda: presentation.suspend_rejected)
            self.assertFalse(presentation.suspended)
            self.assertFalse(helper.retained_path(os.getpid()).exists())
            self.assertEqual(output.getvalue().strip(), 'JFP1 suspend-busy')

    def test_early_lift_buffered_during_sleep_still_cancels_after_resume(self):
        presentation = self.presentation(
            "assert sys.stdin.readline()=='suspend\\n'\nprint('DFP1 suspended',flush=True)\n"
            "assert sys.stdin.readline()=='resume\\n'\n"
            "print('DFP1 resumed\\nDFP1 down 1\\nDFP1 lost 1',flush=True)\ntime.sleep(5)\n", control=True)
        with contextlib.redirect_stdout(io.StringIO()):
            os.write(presentation.control_writer, b'suspend\n')
            self.await_presentation(presentation, lambda: presentation.suspended)
            os.write(presentation.control_writer, b'resume\n')
            with self.assertRaises(helper.ContactLost):
                presentation.wait_down()
        self.assertFalse(presentation.ready)

    def test_unsolicited_suspend_acknowledgement_and_invalid_controls_fail(self):
        presentation = self.presentation("print('DFP1 suspended',flush=True)\ntime.sleep(5)\n", control=True)
        with self.assertRaises(RuntimeError):
            presentation.wait_down()
        presentation = self.presentation("time.sleep(5)\n", control=True)
        os.write(presentation.control_writer, b'resume\n')
        with self.assertRaises(ValueError):
            presentation.poll()

    def test_release_cancels_even_when_native_capture_is_silent(self):
        presentation = self.presentation("print('DFP1 ready 1',flush=True)\ntime.sleep(.1)\nprint('DFP1 lost 1',flush=True)\ntime.sleep(5)\n")
        presentation.wait_ready()
        native = subprocess.Popen(['/usr/bin/python3', '-c', 'import time; time.sleep(5)'], stdout=subprocess.PIPE)
        self.addCleanup(self.close_process, native)
        start = time.monotonic()
        with self.assertRaisesRegex(RuntimeError, 'contact or presentation was lost'):
            list(helper.lines(native, 3, presentation.poll))
        self.assertLess(time.monotonic() - start, 1)

    def test_capture_success_requires_confirmed_illumination_cleanup(self):
        for reason, good in [(0, True), (1, False)]:
            presentation = self.presentation(f"print('DFP1 ready 1',flush=True)\nassert sys.stdin.readline() == 'finish\\n'\nprint('DFP1 stopped 1 {reason}',flush=True)\n")
            presentation.wait_ready()
            if good:
                presentation.finish()
            else:
                with self.assertRaises(RuntimeError):
                    presentation.finish()

    def test_early_lift_retry_requires_confirmed_cleanup(self):
        for reason, valid in [(0, True), (1, False)]:
            presentation = self.presentation(f"print('DFP1 ready 1',flush=True)\ntime.sleep(.1)\nprint('DFP1 lost 1',flush=True)\nprint('DFP1 stopped 1 {reason}',flush=True)\nsys.exit(2)\n")
            presentation.wait_ready()
            time.sleep(.15)
            with self.assertRaises(helper.ContactLost):
                presentation.poll()
            if valid:
                presentation.cancelled_finish()
            else:
                with self.assertRaises(RuntimeError):
                    presentation.cancelled_finish()

    def test_capture_permission_waits_and_ready_followed_by_up_revokes_it(self):
        for loss in [False, True]:
            records = "print('DFP1 down 1',flush=True)\ntime.sleep(.05)\nprint('DFP1 ready 1'" + ("+'\\nDFP1 lost 1'" if loss else "") + ",flush=True)\ntime.sleep(2)\n"
            presentation = self.presentation(records)
            presentation.wait_down()
            gate = helper.CaptureGate(presentation)
            self.addCleanup(gate.close)
            gate.poll()
            self.assertFalse(gate.sent)
            time.sleep(.1)
            if loss:
                with self.assertRaises(helper.ContactLost):
                    gate.poll()
                self.assertFalse(gate.sent)
            else:
                gate.poll()
                self.assertTrue(gate.sent)
                self.assertEqual(os.read(gate.read_fd, 1), b'\x01')
                gate.poll()
                self.assertEqual(os.read(gate.read_fd, 1), b'')

    def test_preparation_runs_before_ready_but_capture_waits(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            marker = root / 'prepared'
            ui = root / 'ui'
            native = root / 'native'
            ui.write_text("#!/usr/bin/python3\nimport sys,time\nfrom pathlib import Path\n"
                "print('DFP1 waiting',flush=True)\nprint('DFP1 down 1',flush=True)\n"
                f"marker=Path({str(marker)!r})\ndeadline=time.monotonic()+2\n"
                "while not marker.exists():\n assert time.monotonic()<deadline\n time.sleep(.005)\n"
                "time.sleep(.03)\nprint('DFP1 ready 1',flush=True)\n"
                "assert sys.stdin.readline()=='finish\\n'\nprint('DFP1 stopped 1 0',flush=True)\n")
            native.write_text("#!/usr/bin/python3\nimport os,sys\nfrom pathlib import Path\n"
                "assert sys.argv[1]=='--verify-gated-with-files'\n"
                f"Path({str(marker)!r}).touch()\n"
                "assert os.read(int(os.environ['ANC_CAPTURE_READY_FD']),1)==b'\\x01'\n"
                "print('Isolated template ID=42')\nprint('Verification compare status=500 matched_id=42')\n"
                "print('unload ancapp64: ioctl=0 errno=0 result=0')\n")
            ui.chmod(0o700); native.chmod(0o700)
            with patch.object(helper, 'PRESENTATION', str(ui)), patch.object(helper, 'PROBE', str(native)), \
                 patch.object(helper, 'RUNTIME', root), patch.dict(os.environ, {'JIIOV_DENIAL_UI':'1'}), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(helper.run_locked('verify', None), (0,42))
                self.assertEqual(output.getvalue().splitlines(), ['JFP1 power 0','JFP1 power 1','JFP1 power 0'])

    def test_lift_during_preparation_retries_without_capture(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            marker = root / 'prepared'
            capture = root / 'captured'
            ui = root / 'ui'
            native = root / 'native'
            ui.write_text("#!/usr/bin/python3\nimport sys,time\nfrom pathlib import Path\n"
                "print('DFP1 down 1',flush=True)\n"
                f"marker=Path({str(marker)!r})\ndeadline=time.monotonic()+2\n"
                "while not marker.exists():\n assert time.monotonic()<deadline\n time.sleep(.005)\n"
                "print('DFP1 lost 1\\nDFP1 stopped 1 0',flush=True)\nsys.exit(2)\n")
            native.write_text("#!/usr/bin/python3\nimport os,sys,signal\nfrom pathlib import Path\n"
                "def finish(*args):\n print('unload ancapp64: ioctl=0 errno=0 result=0',flush=True)\n sys.exit(1)\n"
                "signal.signal(signal.SIGTERM,finish)\n"
                f"Path({str(marker)!r}).touch()\n"
                "permission=os.read(int(os.environ['ANC_CAPTURE_READY_FD']),1)\n"
                f"if permission==b'\\x01': Path({str(capture)!r}).touch()\n"
                "finish()\n")
            ui.chmod(0o700); native.chmod(0o700)
            with patch.object(helper, 'PRESENTATION', str(ui)), patch.object(helper, 'PROBE', str(native)), \
                 patch.object(helper, 'RUNTIME', root), patch.dict(os.environ, {'JIIOV_DENIAL_UI':'1'}), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(helper.run_locked('verify', None), (3,0))
                self.assertFalse(capture.exists())
                self.assertEqual(output.getvalue().splitlines(), ['JFP1 power 0','JFP1 power 1','JFP1 power 0'])

    def test_ready_does_not_survive_worker_death(self):
        presentation = self.presentation("print('DFP1 ready 1',flush=True)\ntime.sleep(.1)\n")
        presentation.wait_ready()
        presentation.process.wait(timeout=2)
        with self.assertRaisesRegex(RuntimeError, 'worker exited'):
            presentation.poll()

    def test_nonmatch_distinct_from_capture_failure(self):
        records = helper.Records('verify')
        records.feed(UNLOAD)
        with self.assertRaises(RuntimeError):
            records.result(1)
        records.feed('Verification compare status=501 matched_id=0')
        self.assertEqual(records.result(1), (1, 0))

    def test_live_positive_and_negative_traces(self):
        root = PW / 'traces'
        for name, exitcode, expected in [
                ('same-finger-verify-live.log', 0, (0, 767149)),
                ('different-finger-verify-live.log', 1, (1, 0))]:
            records = helper.Records('verify')
            for line in (root / name).read_text().splitlines():
                records.feed(line)
            self.assertEqual(records.result(exitcode), expected)

    def test_duplicate_ids_and_regressive_progress_rejected(self):
        records = helper.Records('list')
        records.feed('Isolated template ID=42')
        with self.assertRaises(ValueError):
            records.feed('Isolated template ID=42')
        records = helper.Records('enroll')
        with contextlib.redirect_stdout(io.StringIO()):
            records.feed('Enrollment progress status=401 remaining=19')
            with self.assertRaises(ValueError):
                records.feed('Enrollment progress status=401 remaining=20')


if __name__ == '__main__':
    unittest.main()
