#!/usr/bin/env python3
"""Host serial transport for explicitly supplied rescue commands and RAM uploads."""
import argparse
import fcntl
import gzip
import hashlib
import os
from pathlib import Path
import select
import shlex
import termios
import time
import uuid


class RescueSerial:
    def __init__(self, port="/dev/ttyACM0"):
        self.fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        config = termios.tcgetattr(self.fd)
        config[0], config[1], config[3] = 0, 0, 0
        config[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        config[4] = config[5] = termios.B115200
        config[6][termios.VMIN] = config[6][termios.VTIME] = 0
        termios.tcsetattr(self.fd, termios.TCSANOW, config)
        # Keep the host in raw mode on close: restoring ECHO can bounce the
        # target shell's prompt back into its input when no terminal is open.
        self.write(b"\x15\n")
        self.drain(.2)

    def close(self):
        os.close(self.fd)

    def write(self, data):
        offset = 0
        end = time.monotonic() + 120
        while offset < len(data):
            if time.monotonic() > end:
                raise TimeoutError("serial write timeout")
            if select.select([], [self.fd], [], 1)[1]:
                try:
                    offset += os.write(self.fd, data[offset:offset+4096])
                except BlockingIOError:
                    pass

    def drain(self, seconds):
        result = bytearray()
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([self.fd], [], [], .05)[0]:
                try: result.extend(os.read(self.fd, 65536))
                except BlockingIOError: pass
        return bytes(result)

    def until(self, marker, timeout=20):
        result = bytearray()
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            result.extend(self.drain(.1))
            if marker in bytes(result).replace(b"\r", b""):
                return bytes(result)
        raise TimeoutError("serial response timeout: " + result[-1500:].decode(errors="replace"))

    def run(self, command, timeout=30):
        if "\n" in command:
            raise ValueError("Use one shell command line")
        marker = "PAD4_DONE_" + uuid.uuid4().hex
        self.write((command + "; printf '\\n" + marker + "\\n'\n").encode())
        return self.until(("\n" + marker + "\n").encode(), timeout)

    def upload(self, source, destination):
        if not destination.startswith("/tmp/") or ".." in Path(destination).parts:
            raise ValueError("Uploads are restricted to tablet RAM /tmp")
        data = Path(source).read_bytes()
        compressed = gzip.compress(data, mtime=0)
        target = shlex.quote(destination)
        ready = "PAD4_UPLOAD_READY_" + uuid.uuid4().hex
        done = "PAD4_UPLOAD_DONE_" + uuid.uuid4().hex
        command = (f"stty raw -echo; printf '\\n{ready}\\n'; "
                   f"dd of={target}.gz bs=4096 count={len(compressed)} iflag=count_bytes,fullblock status=none; "
                   f"gzip -df {target}.gz; stty sane; sha256sum {target}; printf '\\n{done}\\n'\n")
        self.write(command.encode())
        self.until(("\n" + ready + "\n").encode())
        self.write(compressed)
        response = self.until(("\n" + done + "\n").encode(), 90)
        expected = hashlib.sha256(data).hexdigest()
        if expected.encode() not in response:
            raise RuntimeError("Uploaded RAM file checksum mismatch: " + response.decode(errors="replace"))
        print(f"Verified RAM upload {destination}: {len(data)} bytes, SHA256 {expected}", flush=True)
        return response


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--port", default="/dev/ttyACM0")
    p.add_argument("--command")
    p.add_argument("--command-file", type=Path)
    p.add_argument("--upload", type=Path)
    p.add_argument("--destination")
    p.add_argument("--out", type=Path)
    p.add_argument("--timeout", type=int, default=30)
    args = p.parse_args()
    serial = RescueSerial(args.port)
    output = bytearray()
    try:
        if args.upload:
            output.extend(serial.upload(args.upload, args.destination))
        command = args.command_file.read_text().strip() if args.command_file else args.command
        if command:
            output.extend(serial.run(command, args.timeout))
    finally:
        serial.close()
    if args.out: args.out.write_bytes(output)
    print(output.decode(errors="replace"))


if __name__ == "__main__":
    main()
