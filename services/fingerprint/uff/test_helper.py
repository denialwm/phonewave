#!/usr/bin/python3
"""No hardware or UI: exercise the two independent authorization state machines."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location('helper', Path(__file__).with_name('uff-fprint-helper.py'))
h = importlib.util.module_from_spec(spec)
spec.loader.exec_module(h)

class ProtocolTests(unittest.TestCase):
    def contact(self):
        c = h.Contact()
        for record in ('DFP1 waiting', 'DFP1 down 1'):
            c.record(record)
        c.present()
        c.record('DFP1 ready 1')
        return c

    def native(self, operation='verify', sample='0 100 123 0'):
        n = h.Native(operation)
        if operation == 'verify':
            n.record('UFW1 standby')
            n.activate()
        n.record('UFW1 prepared')
        n.capture()
        n.record('UFW1 sample ' + sample)
        return n

    def test_contact_requires_scanout_ready(self):
        c = h.Contact()
        c.record('DFP1 waiting')
        c.record('DFP1 down 1')
        with self.assertRaises(h.ProtocolError): c.finish()

    def test_early_lift_cannot_authorize_finish(self):
        c = self.contact()
        with self.assertRaises(h.ProtocolError): c.record('DFP1 lost 1')
        with self.assertRaises(h.ProtocolError): c.completed(2)

    def test_withdrawal_and_successful_exit_both_required(self):
        c = self.contact()
        c.finish()
        with self.assertRaises(h.ProtocolError): c.completed(0)
        c.record('DFP1 stopped 1 0')
        with self.assertRaises(h.ProtocolError): c.completed(2)
        c.completed(0)

    def test_timeout_never_becomes_success(self):
        c = self.contact()
        c.finish()
        with self.assertRaises(h.ProtocolError): c.record('DFP1 stopped 1 1')

    def test_ready_requires_physical_down(self):
        c = h.Contact()
        c.record('DFP1 waiting')
        with self.assertRaises(h.ProtocolError): c.record('DFP1 ready 1')

    def test_match_requires_finish_and_cleanup(self):
        n = self.native()
        with self.assertRaises(h.ProtocolError): n.record('UFF1 result 0 123')
        n.finish()
        with self.assertRaises(h.ProtocolError): n.record('UFF1 result 0 123')
        n.record('UFW1 cleaned')
        n.record('UFF1 result 0 123')
        with self.assertRaises(h.ProtocolError): n.completed(1)
        self.assertEqual(n.completed(0), (0, 123))

    def test_unexpected_identifier_rejected(self):
        n = self.native()
        n.finish()
        n.record('UFW1 cleaned')
        with self.assertRaises(h.ProtocolError): n.record('UFF1 result 0 124')

    def test_nonmatch_identifier_must_be_zero(self):
        with self.assertRaises(h.ProtocolError): self.native(sample='1 100 123 0')
        n = self.native(sample='1 100 0 0')
        n.finish()
        n.record('UFW1 cleaned')
        n.record('UFF1 result 1 0')
        self.assertEqual(n.completed(0), (1, 0))

    def test_duplicate_templates_rejected(self):
        n = h.Native('list')
        n.record('UFF1 item 123')
        with self.assertRaises(h.ProtocolError): n.record('UFF1 item 123')

    def test_enrollment_requires_final_persisted_result(self):
        n = self.native('enroll', '0 3 123 5')
        n.finish()
        n.record('UFW1 cleaned')
        with self.assertRaises(h.ProtocolError): n.record('UFF1 result 0 123')
        n = self.native('enroll', '0 0 123 5')
        n.finish()
        n.record('UFW1 cleaned')
        n.record('UFF1 result 0 123')
        self.assertEqual(n.completed(0), (0, 123))

    def test_duplicate_result_rejected(self):
        n = h.Native('open')
        n.record('UFF1 result 0 0')
        with self.assertRaises(h.ProtocolError): n.record('UFF1 result 0 0')

    def test_preloaded_verification_cannot_capture_before_activation(self):
        n = h.Native('verify')
        n.record('UFW1 standby')
        with self.assertRaises(h.ProtocolError): n.capture()
        with self.assertRaises(h.ProtocolError): n.record('UFW1 prepared')
        n.activate()
        n.record('UFW1 prepared')
        n.capture()

    def test_no_readiness_before_authorized_presentation(self):
        c = h.Contact()
        c.record('DFP1 waiting')
        c.record('DFP1 down 1')
        with self.assertRaises(h.ProtocolError): c.record('DFP1 ready 1')
        c.present()
        c.record('DFP1 ready 1')
        c.finish()

    def test_resident_result_requires_confirmed_park(self):
        n = h.Native('verify', resident=True)
        n.record('UFW1 standby')
        n.activate()
        n.record('UFW1 prepared')
        n.capture()
        n.record('UFW1 sample 0 100 123 0')
        n.finish()
        with self.assertRaises(h.ProtocolError): n.record('UFW1 cleaned')
        with self.assertRaises(h.ProtocolError): n.record('UFF1 result 0 123')
        n.record('UFW1 parked')
        n.record('UFF1 result 0 123')
        self.assertTrue(n.parked)
        self.assertEqual(n.completed(0), (0, 123))

    def test_resident_cannot_park_before_capture_cleanup(self):
        n = h.Native('verify', resident=True)
        n.record('UFW1 standby')
        with self.assertRaises(h.ProtocolError): n.record('UFW1 parked')

    def test_no_capture_after_cleanup(self):
        n = h.Native('verify')
        n.record('UFW1 standby')
        n.activate()
        n.record('UFW1 prepared')
        n.record('UFW1 cleaned')
        with self.assertRaises(h.ProtocolError): n.capture()

class FakeProcess:
    def __init__(self, operation, presentation=False):
        self.eof = False
        self.presentation = presentation
        self.p = SimpleNamespace(poll=lambda: 0 if self.eof else None, returncode=0)
        if presentation:
            self.state = h.Contact()
            for line in ('DFP1 waiting', 'DFP1 down 1'):
                self.state.record(line)
        else:
            self.state = h.Native(operation)
            self.state.record('UFW1 standby' if operation == 'verify' else 'UFW1 prepared')

    def send(self, line):
        if self.presentation and line == 'present':
            self.state.record('DFP1 ready 1')
        elif self.presentation:
            assert line == 'finish'
            self.state.record('DFP1 stopped 1 0')
            self.eof = True
        elif line == 'activate':
            self.state.record('UFW1 prepared')
        elif line == 'capture':
            self.state.record('UFW1 sample 0 0 123 1')
        else:
            assert line == 'finish'
            self.state.record('UFW1 cleaned')
            self.state.record('UFF1 result 0 123')
            self.eof = True

class FakeBridge(h.Bridge):
    def __init__(self, operation):
        self.operation = operation
        self.native = self.presentation = None
    def start_native(self): self.native = FakeProcess(self.operation)
    def start_contact(self): self.presentation = FakeProcess(self.operation, True)
    def wait(self, predicate, timeout): assert predicate(), 'unexpected pending transition'

class BridgeTests(unittest.TestCase):
    def test_enrollment_stops_at_an_unanswered_challenge(self):
        n = h.Native('enroll')
        with self.assertRaises(h.ProtocolError): n.record('UFW1 challenge 1234')
        n = h.Native('verify')
        with self.assertRaises(h.ProtocolError): n.record('UFW1 challenge 1234')

    def test_enrollment_emits_only_enrollment_contract(self):
        records = []
        with patch.object(h, 'emit', records.append): FakeBridge('enroll').run()
        self.assertEqual(records, ['progress 20', 'result 0 123'])
    def test_panel_wake_starts_before_sensor_initialization_finishes(self):
        bridge = FakeBridge('verify')
        original_native = bridge.start_native
        original_contact = bridge.start_contact
        def native():
            original_native()
            send = bridge.native.send
            def delayed(line):
                if line != 'activate': send(line)
            bridge.native.send = delayed
        def contact():
            original_contact()
            send = bridge.presentation.send
            def ready(line):
                if line == 'present':
                    self.assertEqual(bridge.native.state.state, 'activating')
                    send(line)
                    bridge.native.state.record('UFW1 prepared')
                else: send(line)
            bridge.presentation.send = ready
        bridge.start_native = native
        bridge.start_contact = contact
        with patch.object(h, 'emit'): bridge.run()

    def test_capture_waits_for_sensor_even_when_panel_is_ready(self):
        bridge = FakeBridge('verify')
        original_wait = bridge.wait
        def wait(predicate, timeout):
            if bridge.native is not None and bridge.native.state.state == 'activating':
                self.assertEqual(bridge.presentation.state.state, 'ready')
                self.assertFalse(predicate())
                bridge.native.state.record('UFW1 prepared')
            original_wait(predicate, timeout)
        bridge.wait = wait
        original_native = bridge.start_native
        def native():
            original_native()
            send = bridge.native.send
            bridge.native.send = lambda line: None if line == 'activate' else send(line)
        bridge.start_native = native
        with patch.object(h, 'emit'): bridge.run()

    def test_verification_power_contract(self):
        records = []
        with patch.object(h, 'emit', records.append): FakeBridge('verify').run()
        self.assertEqual(records, ['power 0', 'power 1', 'power 0', 'result 0 123'])

if __name__ == '__main__':
    unittest.main()
