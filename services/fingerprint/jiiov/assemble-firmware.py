#!/usr/bin/env python3
"""Reassemble JIIOV signed split firmware without altering its segments."""
import argparse
import hashlib
import json
import struct
from pathlib import Path


def assemble(directory):
    header = (directory / 'ancapp64.b00').read_bytes()
    if header[:6] != b'\x7fELF\x02\x01' or len(header) < 64:
        raise ValueError('Expected a little-endian ELF64 header')
    phoff = struct.unpack_from('<Q', header, 32)[0]
    phsize, phnum = struct.unpack_from('<HH', header, 54)
    if phsize != 56 or not 1 <= phnum <= 32 or phoff + phsize * phnum > len(header):
        raise ValueError('Invalid program header table')
    segments = []
    extent = 0
    for index in range(phnum):
        _, _, offset, _, _, filesz, _, _ = struct.unpack_from('<IIQQQQQQ', header, phoff + index * phsize)
        p = directory / f'ancapp64.b{index:02d}'
        data = p.read_bytes()
        if len(data) != filesz or offset + filesz > 32 * 1024 * 1024:
            raise ValueError(f'Invalid segment size: {p}')
        segments.append((offset, data, p.name))
        extent = max(extent, offset + len(data))
    output = bytearray(extent)
    covered = bytearray(extent)
    for offset, data, name in segments:
        for i, b in enumerate(data, offset):
            if covered[i] and output[i] != b:
                raise ValueError(f'Conflicting overlapping segment: {name}')
        output[offset:offset+len(data)] = data
        covered[offset:offset+len(data)] = b'\x01' * len(data)
    return output, {name: hashlib.sha256(data).hexdigest() for _, data, name in segments}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    image, segments = assemble(args.directory)
    args.output.write_bytes(image)
    manifest = {'size': len(image), 'sha256': hashlib.sha256(image).hexdigest(), 'segments': segments}
    args.output.with_suffix('.json').write_text(json.dumps(manifest, indent=2)+'\n')
    print(json.dumps(manifest, indent=2))

if __name__ == '__main__':
    main()
