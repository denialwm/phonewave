#!/usr/bin/env python3
"""Build and test the uff libfprint driver on the RedMagic, beside the system one.

Run services/fingerprint/uff/build/snapshot_libfprint.py nx809j first. The
library lands in /opt/libfprint-uff/<archive hash>; nothing switches to it
until install_fprintd.py.
"""
from pathlib import Path
import hashlib,json,os,shlex,subprocess
STATE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/nx809j/fingerprint'
SSH=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','nx809j']
m=json.loads((STATE/'libfprint/source-manifest.json').read_text());data=Path(m['archive']).read_bytes()
assert hashlib.sha256(data).hexdigest()==m['archive_sha256']
CODE=r'''
from pathlib import Path
import hashlib,io,json,os,subprocess,sys,tarfile
assert 'nubia,nx809j' in Path('/proc/device-tree/compatible').read_text()
assert 'androidboot.slot_suffix = "_b"' in Path('/proc/bootconfig').read_text()
digest=sys.argv[1];data=sys.stdin.buffer.read();assert hashlib.sha256(data).hexdigest()==digest
root=Path('/var/tmp/libfprint-uff-'+digest[:12]);prefix=Path('/opt/libfprint-uff')/digest[:12]
source=root/'source';build=root/'build';root.mkdir(mode=0o700,exist_ok=True)
with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as tar:
 for e in tar:
  assert e.isfile() and not Path(e.name).is_absolute() and '..' not in Path(e.name).parts
 if not source.exists():source.mkdir(mode=0o700);tar.extractall(source,filter='data')
cmd=['meson','setup',str(build),str(source),'--prefix='+str(prefix),'--libdir=lib','--buildtype=debugoptimized','-Ddrivers=uff','-Dintrospection=false','-Ddoc=false','-Dgtk-examples=false','-Dinstalled-tests=false','-Dudev_rules=disabled','-Dudev_hwdb=disabled']
if not (build/'build.ninja').exists():subprocess.run(cmd,check=True)
subprocess.run(['ninja','-C',str(build),'-j','6'],check=True)
subprocess.run(['meson','test','-C',str(build),'--suite','unit-tests','--print-errorlogs'],check=True)
subprocess.run(['meson','install','-C',str(build)],check=True)
print('UFF_BUILD_MANIFEST '+json.dumps({'prefix':str(prefix),'root':str(root),'archive_sha256':digest,'library_sha256':hashlib.sha256((prefix/'lib/libfprint-2.so.2.0.0').read_bytes()).hexdigest()}))
'''
command=shlex.join(['systemd-inhibit','--what=sleep:idle','--mode=block','--who=nx809j-fingerprint-build','--why=Build the native fingerprint library','python3','-c',CODE,m['archive_sha256']])
log=STATE/'libfprint/build.log'
with log.open('wb') as out:
 p=subprocess.run([*SSH,command],input=data,stdout=out,stderr=subprocess.STDOUT)
lines=log.read_text(errors='replace').splitlines()
for line in lines:
 if line.startswith('UFF_BUILD_MANIFEST '):(STATE/'libfprint/build-manifest.json').write_text(json.dumps(json.loads(line[19:]),indent=2)+'\n')
print('\n'.join(lines[-18:]));print('Build exit:',p.returncode);raise SystemExit(p.returncode)
