#!/usr/bin/env python3
"""Activate canonical protected-file and authenticated RPMB commits on NX809J.

Run only after explicit approval for writes to persist/data and authenticated
RPMB security state. This is not a partition flasher or a credential client.
It sends no PIN, creates no credential and never programs an RPMB key. The
normal Gatekeeper UI operation subsequently asks the trusted app to commit.
The diagnostic Linux overlay is preserved and becomes inactive.
"""
from pathlib import Path
import argparse
import hashlib
import io
import json
import os
import shlex
import subprocess
import tarfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--enable-canonical-authenticated', action='store_true', required=True)
args = parser.parse_args()
cache = Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/nx809j/fingerprint'
binary = cache/'goodix-secure-storage'
manifest = json.loads((cache/'secure-storage-manifest.json').read_text())
payload = binary.read_bytes()
digest = hashlib.sha256(payload).hexdigest()
if digest != manifest['output_sha256']:
    raise SystemExit('Storage executable does not match its build manifest')
# Check source changes after the build, rather than silently activating stale code.
for name, expected in manifest['inputs'].items():
    if hashlib.sha256(Path(name).read_bytes()).hexdigest() != expected:
        raise SystemExit('Rebuild userspace before activation: '+name)

code = r'''
from pathlib import Path
import hashlib,os,subprocess,sys
expected=sys.argv[1]
payload=sys.stdin.buffer.read()
assert hashlib.sha256(payload).hexdigest()==expected
assert 'nubia,nx809j' in Path('/proc/device-tree/compatible').read_text()
assert 'androidboot.slot_suffix = "_b"' in Path('/proc/bootconfig').read_text()
def run(*args):return subprocess.run(args,check=True,timeout=25)
def put(path,data,mode):
    path=Path(path);path.parent.mkdir(parents=True,exist_ok=True)
    tmp=path.with_name('.'+path.name+'.new');tmp.unlink(missing_ok=True)
    fd=os.open(tmp,os.O_WRONLY|os.O_CREAT|os.O_EXCL,mode)
    with os.fdopen(fd,'wb') as f:f.write(data);f.flush();os.fsync(f.fileno())
    os.chown(tmp,0,0);tmp.chmod(mode);tmp.replace(path)
    d=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
    try:os.fsync(d)
    finally:os.close(d)
active=subprocess.check_output(['systemctl','list-units','--state=running','--no-legend','nx809j-gatekeeper@*'],text=True)
assert not active.strip(),'A credential operation is active; let it finish before activation'
# Quiesce the clients before changing backing stores. A failed activation
# leaves them stopped; it never falls back to an overlay plus RPMB writes.
run('systemctl','stop','nx809j-gatekeeper.socket','fprintd.service')
run('systemctl','stop','nx809j-secure-storage.service')
run('systemctl','stop','run-nx809j-securestorage.mount')
put('/usr/libexec/goodix-secure-storage',payload,0o700)
put('/etc/systemd/system/run-nx809j-securestorage.mount.d/80-canonical-commit.conf',
    b'[Mount]\nOptions=rw,nosuid,nodev,noexec\n',0o644)
put('/etc/systemd/system/nx809j-secure-storage.service.d/80-canonical-commit.conf',
    b'[Service]\nEnvironment=NX809J_CANONICAL_AUTHENTICATED_STORAGE=1\nReadWritePaths=/run/nx809j/securestorage/data\n',0o644)
run('systemctl','daemon-reload')
run('systemctl','start','nx809j-secure-storage.service')
run('systemctl','start','nx809j-gatekeeper.socket','fprintd.service')
run('systemctl','is-active','nx809j-secure-storage.service','nx809j-gatekeeper.socket','fprintd.service')
print('Canonical protected-storage commit enabled. No credential request was submitted.')
'''
subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=6','nx809j',
                shlex.join(['python3','-c',code,digest])],input=payload,check=True)
