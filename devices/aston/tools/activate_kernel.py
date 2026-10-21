#!/usr/bin/env python3
"""Write this exact candidate using the existing guarded Aston boot updater.

No reboot. The source image, companion, module closure and updater hashes must
match; the updater backs up boot_b and verifies a complete readback.
"""
import argparse,hashlib,json,os,shlex,subprocess
from pathlib import Path
ROOT=Path(os.environ.get('PHONEWAVE_VAULT','/mnt/development/denial-os/device-vault'))/'aston/boot'
SSH=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','aston']
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--generation',type=int,default=27)
parser.add_argument('--previous-generation',type=int,default=24)
ARGS=parser.parse_args();GENERATION=ARGS.generation
assert 27<=GENERATION<=99
RELEASE=f'7.3.0-devices{GENERATION}-aston'
IMAGE=ROOT/f'boot_b-devices{GENERATION}.img'
UPDATER=Path(__file__).resolve().parent/'remote_boot_flash.py'
REMOTE_IMAGE=f'/var/tmp/aston-fingerprint/boot_b-devices{GENERATION}.img'
STAGE=r'''
from pathlib import Path
import hashlib,sys
b=Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "f683d9c9"' in b
assert 'androidboot.slot_suffix = "_b"' in b
p=Path(sys.argv[1]);digest=sys.argv[2]
p.parent.mkdir(mode=0o700,exist_ok=True)
if p.exists():
 assert hashlib.sha256(p.read_bytes()).hexdigest()==digest
else:
 import os
 fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'wb') as out:
  for data in iter(lambda:sys.stdin.buffer.read(4*1024*1024),b''):out.write(data)
assert p.stat().st_size==201326592
assert hashlib.sha256(p.read_bytes()).hexdigest()==digest
print(digest)
'''
ACTIVATE=r'''
from pathlib import Path
import hashlib,json,subprocess,sys
expected=json.loads(sys.argv[1])
def sha(p):
 h=hashlib.sha256()
 with open(p,'rb') as f:
  for b in iter(lambda:f.read(4*1024*1024),b''):h.update(b)
 return h.hexdigest()
b=Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "f683d9c9"' in b
assert 'androidboot.slot_suffix = "_b"' in b
assert sha('/dev/disk/by-partlabel/init_boot_b')==expected['companion']
assert sha('/dev/disk/by-partlabel/boot_b')==expected['previous']
updater=Path('/var/lib/aston-release/remote_boot_flash.py')
assert sha(updater)==expected['updater']
modules=Path('/usr/lib/modules')/expected['release']
m=json.loads((modules/'fingerprint-module-manifest.json').read_text())
assert m['release']==expected['release']
for name,digest in m['files'].items():
 p=modules/name
 assert not p.is_symlink() and p.stat().st_uid==0 and sha(p)==digest
assert sha(expected['image'])==expected['image_sha256']
# Only this existing updater opens the allowlisted boot_b partition for writing.
r=subprocess.run(['python3',str(updater),'--apply','--partition','boot_b','--image',expected['image'],'--expected-sha',expected['image_sha256'],'--expect-current-sha',expected['previous'],'--backup-dir','/var/lib/aston-release-backups/fingerprint-'+expected['release']],stdout=subprocess.PIPE,check=True,text=True)
result=json.loads(r.stdout)
assert result['readback_sha256']==expected['image_sha256']
assert result['backup_sha256']==expected['previous']
print(json.dumps(result))
'''
def main():
 data=IMAGE.read_bytes();digest=hashlib.sha256(data).hexdigest()
 assert digest==json.loads((ROOT/('boot-package.json' if GENERATION==27 else f'boot-package-devices{GENERATION}.json')).read_text())['sha256']
 p=subprocess.run([*SSH,shlex.join(['python3','-c',STAGE,REMOTE_IMAGE,digest])],input=data,stdout=subprocess.PIPE,check=True)
 assert p.stdout.decode().strip()==digest
 pre=json.loads((ROOT/'boot-preflight.json').read_text())
 if ARGS.previous_generation!=24:
  previous=json.loads((ROOT/('kernel-flash-evidence.json' if ARGS.previous_generation==27 else f'kernel-flash-evidence-devices{ARGS.previous_generation}.json')).read_text())
  pre['boot_b']=previous['readback_sha256']
 expected={'release':RELEASE,'image':REMOTE_IMAGE,'image_sha256':digest,'previous':pre['boot_b'],'companion':pre['init_boot_b'],'updater':hashlib.sha256(UPDATER.read_bytes()).hexdigest()}
 p=subprocess.run([*SSH,shlex.join(['python3','-c',ACTIVATE,json.dumps(expected)])],stdout=subprocess.PIPE,check=True)
 (ROOT/('kernel-flash-evidence.json' if GENERATION==27 else f'kernel-flash-evidence-devices{GENERATION}.json')).write_bytes(p.stdout)
 print(p.stdout.decode())
if __name__=='__main__':main()
