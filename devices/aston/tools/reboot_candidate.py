#!/usr/bin/env python3
"""Verify the candidate and backup, then normally reboot the authorized phone."""
import argparse,hashlib,json,os,shlex,subprocess
from pathlib import Path
ROOT=Path(os.environ.get('PHONEWAVE_VAULT','/mnt/development/device-vault'))/'aston/boot'
STATE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/aston/kernel'
parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--generation',type=int,required=True)
GEN=parser.parse_args().generation;assert GEN>=27
SSH=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','aston']
manifest=ROOT/('kernel-flash-evidence.json' if GEN==27 else f'kernel-flash-evidence-devices{GEN}.json')
m=json.loads(manifest.read_text());backup=ROOT/f'boot_b-before-devices{GEN}.img'
if not backup.exists():
 with backup.open('xb') as f:subprocess.run([*SSH,shlex.join(['cat',m['backup']])],stdout=f,check=True)
assert backup.stat().st_size==201326592
assert hashlib.sha256(backup.read_bytes()).hexdigest()==m['backup_sha256']
pre=json.loads((ROOT/'boot-preflight.json').read_text())
code='''from pathlib import Path
import hashlib,json,subprocess,sys
m=json.loads(sys.argv[1]);b=Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "f683d9c9"' in b and 'androidboot.slot_suffix = "_b"' in b
for name,digest in m['hashes'].items():
 h=hashlib.sha256()
 with open('/dev/disk/by-partlabel/'+name,'rb') as f:
  for data in iter(lambda:f.read(4*1024*1024),b''):h.update(data)
 assert h.hexdigest()==digest
print('Previous boot ID:',Path('/proc/sys/kernel/random/boot_id').read_text().strip(),flush=True)
subprocess.run(['systemctl','stop','serial-getty@ttyGS0.service'],check=True)
subprocess.run(['systemctl','reboot'],check=True)
'''
payload={'hashes':{'boot_b':m['readback_sha256'],'init_boot_b':pre['init_boot_b']}}
p=subprocess.run([*SSH,shlex.join(['python3','-c',code,json.dumps(payload)])],stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
STATE.mkdir(parents=True,exist_ok=True);(STATE/f'kernel-reboot-devices{GEN}.log').write_bytes(p.stdout)
print(p.stdout.decode());print('SSH exit:',p.returncode)
