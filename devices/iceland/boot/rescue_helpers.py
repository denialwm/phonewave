"""Ramdisk and ELF helpers for package_pad4_devices.py, from the retired build_usb_rescue.py."""
from pathlib import Path
import hashlib
import struct
import subprocess


def run(*args, cwd=None, data=None):
    p = subprocess.run([str(a) for a in args], cwd=cwd, input=data, capture_output=True)
    if p.returncode:
        raise RuntimeError(f"{args}:\n{p.stdout.decode(errors='replace')}\n{p.stderr.decode(errors='replace')}")
    return p.stdout


def sha(path):
    return hashlib.file_digest(Path(path).open("rb"), "sha256").hexdigest()


def cpio_read(data):
    entries = {}
    offset = 0
    while True:
        assert data[offset:offset+6] == b"070701", f"bad newc at {offset}"
        values = [int(data[offset+6+i*8:offset+14+i*8], 16) for i in range(13)]
        name_end = offset+110+values[11]
        name = data[offset+110:name_end-1].decode()
        assert data[name_end-1] == 0
        start = (name_end+3) & ~3
        end = start+values[6]
        assert end <= len(data)
        offset = (end+3) & ~3
        if name == "TRAILER!!!":
            break
        while name.startswith("./"):
            name = name[2:]
        assert not name.startswith("/") and ".." not in name.split("/")
        entries[name] = (values, data[start:end])
    return entries


def cpio_write(entries):
    result = bytearray()
    for inode, (name, (values, data)) in enumerate(
            list(entries.items()) + [("TRAILER!!!", ([0]*13, b""))], 1):
        values = values.copy()
        name_bytes = name.encode()+b"\0"
        values[0], values[6], values[11], values[12] = inode, len(data), len(name_bytes), 0
        result += b"070701" + b"".join(f"{v:08x}".encode() for v in values) + name_bytes
        result += b"\0" * (-len(result) % 4)
        result += data
        result += b"\0" * (-len(result) % 4)
    result += b"\0" * (-len(result) % 512)
    return bytes(result)


def entry(mode, data=b""):
    fields = [0]*13
    fields[1], fields[4] = mode, 1
    return fields, data


def static_arm64(path):
    data = path.read_bytes()
    assert data[:5] == b"\x7fELF\x02" and struct.unpack_from("<H", data, 18)[0] == 183
    assert "INTERP" not in run("readelf", "-l", path).decode()
    assert "NEEDED" not in run("readelf", "-d", path).decode()
