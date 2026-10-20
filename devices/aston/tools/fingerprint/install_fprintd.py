#!/usr/bin/python3
"""Select the isolated native driver and its required service capabilities."""
from pathlib import Path
import hashlib,json,shlex,subprocess
from run_probe import SSH,STATE as R
library=json.loads((R/'libfprint/build-manifest.json').read_text())
config='''[Service]
Environment=LD_LIBRARY_PATH={LIBRARY_PREFIX}/lib
ReadWritePaths=/var/lib/aston-fingerprint
ProtectHome=tmpfs
BindReadOnlyPaths=/run/user/1001
DeviceAllow=/dev/tee0 rw
DeviceAllow=/dev/fingerprint_dev rw
DeviceAllow=/dev/aston_fp_illumination rw
DeviceAllow=/dev/dma_heap/system rw
DeviceAllow=char-input r
'''
config=config.replace('{LIBRARY_PREFIX}',library['prefix'])
code='''import hashlib,json,os,re,subprocess,sys
from pathlib import Path
c=json.load(sys.stdin);lib=Path(c['prefix'])/'lib/libfprint-2.so.2';assert hashlib.sha256(lib.read_bytes()).hexdigest()==c['library_sha256']
p=Path('/etc/systemd/system/fprintd.service.d/60-aston-native.conf');p.parent.mkdir(parents=True,exist_ok=True)
if p.exists():
 if p.read_text()!=c['config']:
  old=c['config'].replace(c['prefix'],re.search(r'^Environment=LD_LIBRARY_PATH=(/opt/libfprint-uff/[0-9a-f]{12})/lib$',p.read_text(),re.M).group(1))
  assert p.read_text()==old,'Service configuration changed outside this deployment'
  backup=Path('/var/lib/aston-fingerprint')/('fprintd-before-'+hashlib.sha256(p.read_bytes()).hexdigest()[:12]+'.conf')
  if not backup.exists():backup.write_text(p.read_text());backup.chmod(0o600)
  temporary=p.with_suffix('.new');temporary.write_text(c['config']);temporary.chmod(0o644);temporary.replace(p)
else:p.write_text(c['config']);p.chmod(0o644)
subprocess.run(['systemctl','daemon-reload'],check=True)
subprocess.run(['systemctl','restart','fprintd.service'],check=True,timeout=20)
print(subprocess.check_output(['systemctl','show','fprintd.service','-p','MainPID','-p','ActiveState'],text=True))
pid=int(subprocess.check_output(['systemctl','show','fprintd.service','-p','MainPID','--value'],text=True))
maps=Path('/proc/'+str(pid)+'/maps').read_text();assert str(lib.parent) in maps
print(subprocess.check_output(['busctl','call','net.reactivated.Fprint','/net/reactivated/Fprint/Manager','net.reactivated.Fprint.Manager','GetDevices'],text=True))
print('Native fprintd library selected; sandbox/device access configured')
'''
c=dict(library,config=config)
p=subprocess.run([*SSH,shlex.join(['python3','-c',code])],input=json.dumps(c).encode(),capture_output=True)
(R/'fprintd-install.log').write_bytes(p.stdout+p.stderr)
print(p.stdout.decode());print(p.stderr.decode());raise SystemExit(p.returncode)
