#!/usr/bin/env python3
"""Build, but never launch, the bounded screen-on illumination diagnostic."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

PW = Path(__file__).resolve().parent
CACHE = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache') / 'phonewave'
OUT = CACHE / 'roadstr/fingerprint-native2'
SYSROOT = Path('/mnt/development/cross/sysroot')
XML = Path('/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml')
INSETS = Path('/home/logix/denial/compositor/protocol/denial-insets-v1.xml')


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for mode, name in [('client-header', 'xdg-shell-client-protocol.h'),
                       ('private-code', 'xdg-shell-protocol.c')]:
        subprocess.run(['wayland-scanner', mode, str(XML), str(OUT / name)], check=True)
    for mode, name in [('client-header', 'denial-insets-client-protocol.h'),
                       ('private-code', 'denial-insets-protocol.c')]:
        subprocess.run(['wayland-scanner', mode, str(INSETS), str(OUT / name)], check=True)
    command = ['aarch64-linux-gnu-gcc', f'--sysroot={SYSROOT}', '-O2', '-Wall',
               '-Wextra', '-Werror', f'-I{OUT}', f'-I{SYSROOT}/usr/include',
               str(PW / 'illumination-test.c'),
               str(OUT / 'xdg-shell-protocol.c'), str(OUT / 'denial-insets-protocol.c'), f'-L{SYSROOT}/usr/lib',
               f'-L{SYSROOT}/lib', f'-Wl,-rpath-link,{SYSROOT}/usr/lib',
               f'-Wl,-rpath-link,{SYSROOT}/lib', '-lwayland-client',
               '-o', str(OUT / 'illumination-test')]
    subprocess.run(command, check=True)
    binary = (OUT / 'illumination-test').read_bytes()
    if binary[:6] != b'\x7fELF\x02\x01' or binary[18:20] != b'\xb7\x00':
        raise SystemExit('Expected a little-endian AArch64 ELF')
    inputs = [PW / 'illumination-test.c', PW / 'diagnostic-ui.h',
              Path(__file__).resolve(), XML, INSETS]
    inputs += [OUT / name for name in ['xdg-shell-protocol.c', 'xdg-shell-client-protocol.h']]
    inputs += [OUT / name for name in ['denial-insets-protocol.c', 'denial-insets-client-protocol.h']]
    inputs += [SYSROOT / name for name in ['usr/lib/libwayland-client.so', 'usr/lib/libffi.so.8', 'lib/libc.so.6']]
    manifest = {
        'command': command,
        'compiler': subprocess.check_output(['aarch64-linux-gnu-gcc', '--version'], text=True).splitlines()[0],
        'sha256': hashlib.sha256(binary).hexdigest(),
        'inputs': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs},
        'runtime_validation': 'Build only; record device results separately',
    }
    (OUT / 'illumination-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('Illumination diagnostic SHA256:', manifest['sha256'])


if __name__ == '__main__':
    main()
