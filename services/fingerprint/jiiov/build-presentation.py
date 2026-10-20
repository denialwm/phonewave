#!/usr/bin/env python3
"""Cross-build the privileged DMA-BUF/FOD presentation worker; never launch it."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

PW = Path(__file__).resolve().parent
CACHE = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache') / 'phonewave'
OUT = CACHE / 'roadstr/denial-fingerprint1/presentation'
SYSROOT = Path('/mnt/development/cross/sysroot')
PROTOCOLS = {
    'denial-fingerprint': Path('/home/logix/denial/compositor/protocol/denial-fingerprint-v1.xml'),
    'linux-dmabuf': Path('/usr/share/wayland-protocols/stable/linux-dmabuf/linux-dmabuf-v1.xml'),
}


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    inputs = [Path(__file__).resolve(), PW / 'denial-presentation.c']
    for name, xml in PROTOCOLS.items():
        inputs.append(xml)
        for mode, suffix in [('client-header', '-client-protocol.h'), ('private-code', '-protocol.c')]:
            output = OUT / (name + suffix)
            subprocess.run(['wayland-scanner', mode, str(xml), str(output)], check=True)
            inputs.append(output)
    binary = OUT / 'jiiov-denial-presentation'
    command = ['aarch64-linux-gnu-gcc', f'--sysroot={SYSROOT}', '-O2', '-Wall', '-Wextra', '-Werror',
               f'-I{OUT}', f'-I{SYSROOT}/usr/include', str(inputs[1]),
               *(str(OUT / (name + '-protocol.c')) for name in PROTOCOLS),
               f'-L{SYSROOT}/usr/lib', f'-L{SYSROOT}/lib',
               f'-Wl,-rpath-link,{SYSROOT}/usr/lib', f'-Wl,-rpath-link,{SYSROOT}/lib',
               '-lwayland-client', '-o', str(binary)]
    subprocess.run(command, check=True)
    data = binary.read_bytes()
    assert data[:6] == b'\x7fELF\x02\x01' and data[18:20] == b'\xb7\x00'
    inputs.extend(SYSROOT / name for name in ['usr/lib/libwayland-client.so', 'usr/lib/libffi.so.8', 'lib/libc.so.6'])
    manifest = dict(command=command, sha256=hashlib.sha256(data).hexdigest(),
                    inputs={str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs},
                    compiler=subprocess.check_output(['aarch64-linux-gnu-gcc', '--version'], text=True).splitlines()[0])
    (OUT / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(manifest['sha256'])


if __name__ == '__main__':
    main()
