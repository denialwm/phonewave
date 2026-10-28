#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Exercise split-firmware validation without a phone or vendor blobs."""
import struct
import tempfile
from pathlib import Path
import unittest
from build import assemble


class FirmwareTests(unittest.TestCase):
    def fixture(self, path, duplicate=b"ABCD"):
        header = bytearray(64 + 3 * 56)
        header[:6] = bytes([0x7f, 69, 76, 70, 2, 1])
        struct.pack_into("<Q", header, 32, 64)
        struct.pack_into("<HH", header, 54, 56, 3)
        for index, offset, size in [(0, 0, len(header)), (1, 4096, 4), (2, 4096, 4)]:
            struct.pack_into("<IIQQQQQQ", header, 64 + index * 56,
                             1, 0, offset, 0, 0, size, size, 1)
        (path / "goodixfp64.b00").write_bytes(header)
        (path / "goodixfp64.b01").write_bytes(b"ABCD")
        (path / "goodixfp64.b02").write_bytes(duplicate)
        return header

    def test_preserve_signed_segments_and_identical_overlap(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            header = self.fixture(path)
            data, segments = assemble(path)
            self.assertEqual(data[:len(header)], header)
            self.assertEqual(data[len(header):4096], bytes(4096-len(header)))
            self.assertEqual(data[4096:], b"ABCD")
            self.assertEqual(len(segments), 3)

    def test_reject_conflicting_overlap(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            self.fixture(path, duplicate=b"ABCE")
            with self.assertRaisesRegex(ValueError, "Conflicting overlapping"):
                assemble(path)

    def test_reject_truncated_segment(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            self.fixture(path)
            (path / "goodixfp64.b01").write_bytes(b"ABC")
            with self.assertRaisesRegex(ValueError, "Invalid segment extent"):
                assemble(path)

    def test_reject_program_headers_outside_header(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            header = self.fixture(path)
            struct.pack_into("<Q", header, 32, len(header))
            (path / "goodixfp64.b00").write_bytes(header)
            with self.assertRaisesRegex(ValueError, "Invalid program headers"):
                assemble(path)


if __name__ == "__main__":
    unittest.main()
