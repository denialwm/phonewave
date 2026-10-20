#!/usr/bin/env python3
"""Build an isolated experimental libfprint on the validated ARM phone."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile
import time

PW = Path(__file__).resolve().parents[4] / 'services/fingerprint/jiiov'
CACHE = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache') / 'phonewave'
UPSTREAM = '80a4b5ec612892c5c056c48dddaa561452cf37ec'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--evidence-dir', type=Path, default=CACHE / 'roadstr/fingerprint-evidence')
parser.add_argument('--output-dir', type=Path, default=CACHE / 'roadstr/fingerprint-native2')
parser.add_argument('--ssh-wrapper', type=Path, help='Trusted argument-quoting device SSH helper')
parser.add_argument('--run-unit-tests', action='store_true')
args = parser.parse_args()
args.evidence_dir.mkdir(parents=True, exist_ok=True)
args.output_dir.mkdir(parents=True, exist_ok=True)
SPEC = importlib.util.spec_from_file_location('optical', Path(__file__).with_name('run-optical-test.py'))
optical = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(optical)
# Upstream libfprint at UPSTREAM plus the phonewave patches, prepared on demand.
source = CACHE / 'src/libfprint'
if not source.exists():
    source.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix='libfprint.', dir=source.parent))
    subprocess.run(['git', 'clone', '--quiet', 'https://gitlab.freedesktop.org/libfprint/libfprint.git',
                    str(staging)], check=True)
    subprocess.run(['git', '-C', str(staging), 'checkout', '--quiet', '--detach', UPSTREAM], check=True)
    subprocess.run(['git', '-C', str(staging), 'apply',
                    *map(str, sorted((PW / 'libfprint').glob('*.patch')))], check=True)
    staging.rename(source)
archive = args.output_dir / 'libfprint-jiiov-source.tar.gz'
with tarfile.open(archive, 'w:gz') as tar:
    for path in sorted(source.rglob('*')):
        relative = path.relative_to(source)
        if '.git' in relative.parts or not path.is_file():
            continue
        tar.add(path, arcname=str(relative), recursive=False)
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
build_root = '/var/tmp/libfprint-jiiov-' + digest[:12]
prefix = '/opt/libfprint-jiiov/' + digest[:12]
remote_archive = '/var/tmp/libfprint-jiiov-' + digest[:12] + '.tar.gz'
if args.ssh_wrapper:
    transfer = ('import hashlib,sys; from pathlib import Path; '
                'assert "ZY22MMG59D" in Path("/proc/bootconfig").read_text(); '
                'data=sys.stdin.buffer.read(); assert hashlib.sha256(data).hexdigest()==sys.argv[2]; '
                'Path(sys.argv[1]).write_bytes(data)')
    for attempt in range(10):
        result = subprocess.run([str(args.ssh_wrapper), '/usr/bin/python3', '-c', transfer,
                                 remote_archive, digest], input=archive.read_bytes())
        if result.returncode != 255:
            result.check_returncode()
            break
        time.sleep(2)
    else:
        raise SystemExit('Phone was not reachable in a scheduled wake window')
else:
    optical.stage(archive, remote_archive)
setup = ['meson', 'setup', build_root + '/build', build_root + '/source',
         '--prefix=' + prefix, '--libdir=lib', '--buildtype=debugoptimized',
         '-Ddrivers=jiiov', '-Dintrospection=false', '-Ddoc=false',
         '-Dgtk-examples=false', '-Dinstalled-tests=false', '-Dudev_rules=disabled']
command = f'''set -eu
grep -q ZY22MMG59D /proc/bootconfig
test "$(uname -m)" = aarch64
install -d -m 700 {build_root}/source
tar -xzf {remote_archive} -C {build_root}/source
{shlex.join(setup)}
ninja -C {build_root}/build -j 4
{'meson test -C ' + build_root + '/build --suite unit-tests --print-errorlogs' if args.run_unit_tests else ':'}
meson install -C {build_root}/build
sha256sum {prefix}/lib/libfprint-2.so.2.0.0
gcc --version
pkg-config --modversion glib-2.0 gusb gudev-1.0
'''
log_path = args.evidence_dir / 'libfprint-phone-build.log'
with log_path.open('w') as log:
    invocation = ( [str(args.ssh_wrapper), '/usr/bin/systemd-inhibit', '--what=sleep:idle',
                    '--mode=block', '--who=jiiov-library-build', '--why=Build and test the matched fingerprint library',
                    '/usr/bin/sh', '-c', command] if args.ssh_wrapper else optical.SSH + [command])
    process = subprocess.Popen(invocation, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True)
    for line in process.stdout:
        log.write(line)
        log.flush()
    status = process.wait()
manifest = {'source_archive_sha256': digest, 'prefix': prefix,
            'build_root': build_root, 'configure': setup, 'exit_status': status,
            'upstream_commit': UPSTREAM}
(args.evidence_dir / 'libfprint-phone-manifest.json').write_text(
    json.dumps(manifest, indent=2) + '\n')
print(json.dumps(manifest, indent=2))
raise SystemExit(status)
