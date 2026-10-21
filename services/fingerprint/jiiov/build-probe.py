#!/usr/bin/env python3
"""Build the native fingerprint probe and run its host-side boundary tests."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

PW = Path(__file__).resolve().parent
CACHE = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache') / 'phonewave'
OUT = CACHE / 'roadstr/fingerprint-native1'
# Qualcomm minkipc, unmodified, cloned on demand at REVISION.
MINK = CACHE / 'src/minkipc'
REVISION = 'bb99356e15c34434d5bc521f1f68bb2f3273793b'


def run(args, **kwargs):
    return subprocess.run(args, check=True, cwd=OUT, **kwargs)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    if not MINK.exists():
        MINK.parent.mkdir(parents=True, exist_ok=True)
        run(['git', 'clone', '--quiet', 'https://github.com/qualcomm/minkipc', str(MINK)])
        run(['git', '-C', str(MINK), 'checkout', '--quiet', '--detach', REVISION])
    revision = subprocess.check_output(['git', '-C', str(MINK), 'rev-parse', 'HEAD'], text=True).strip()
    if revision != REVISION:
        raise SystemExit(f'Expected Qualcomm minkipc {REVISION}; found {revision}')
    run(['git', '-C', str(MINK), 'diff', '--exit-code', 'HEAD', '--', 'listeners/libfsservice'])
    includes = [Path('/mnt/development/denial-os/baseline-kernels/roadstr/moto-6.6/vendor-qcom-opensource-securemsm-kernel/include/uapi/linux')]
    includes += [MINK / f'listeners/libfsservice/{d}' for d in ('fs', 'gpfs', 'cmn')]
    flags = ['-pthread', '-std=c11', '-D_GNU_SOURCE', '-Wall', '-Wextra', '-Werror']
    for directory in includes:
        flags.extend(['-I', str(directory)])
    build = ['aarch64-linux-gnu-gcc', '-static', '-O2', *flags,
             str(PW / 'tee-probe.c'), str(PW / 'anc-protocol.c'),
             '-o', str(OUT / 'tee-probe')]
    run(build)
    run(['gcc', '-O1', '-g', '-fsanitize=address,undefined', *flags,
         str(PW / 'test-files.c'), str(PW / 'anc-protocol.c'),
         '-o', str(OUT / 'test-files')])
    # LeakSanitizer cannot operate under this workspace's ptrace sandbox.
    # AddressSanitizer and UndefinedBehaviorSanitizer remain enabled.
    run([str(OUT / 'test-files')], env={**os.environ, 'ASAN_OPTIONS': 'detect_leaks=0'})
    run(['gcc', '-O1', '-g', '-fsanitize=address,undefined', *flags,
         str(PW / 'test-protocol.c'), str(PW / 'anc-protocol.c'),
         '-o', str(OUT / 'test-protocol')])
    run([str(OUT / 'test-protocol')], env={**os.environ, 'ASAN_OPTIONS': 'detect_leaks=0'})
    run(['gcc', '-O1', '-g', '-fsanitize=address,undefined', *flags,
         str(PW / 'test-capture-gate.c'), '-o', str(OUT / 'test-capture-gate')])
    run([str(OUT / 'test-capture-gate')], env={**os.environ, 'ASAN_OPTIONS': 'detect_leaks=0'})
    sources = [PW / n for n in
               ('tee-probe.c', 'tee-files.h', 'test-files.c',
                'anc-protocol.c', 'anc-protocol.h', 'test-protocol.c', 'capture-gate.h', 'test-capture-gate.c')]
    sources.append(Path(__file__).resolve())
    sources += [includes[0] / 'smcinvoke.h', includes[1] / 'fs_msg.h',
                includes[2] / 'gpfs_msg.h', includes[3] / 'cmn.h']
    manifest = {
        'minkipc_repository': 'https://github.com/qualcomm/minkipc',
        'minkipc_revision': REVISION,
        'compiler': subprocess.check_output(['aarch64-linux-gnu-gcc', '--version'], text=True).splitlines()[0],
        'command': build,
        'sha256': hashlib.sha256((OUT / 'tee-probe').read_bytes()).hexdigest(),
        'sources': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in sources},
        'tests': 'ASan and UBSan calibration listener, packet vector and frame boundary tests passed; leak detection disabled under ptrace',
    }
    (OUT / 'probe-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('Native probe SHA256:', manifest['sha256'])


if __name__ == '__main__':
    main()
