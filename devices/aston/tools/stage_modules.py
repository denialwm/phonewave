#!/usr/bin/env python3
"""Install the new versioned module directory without loading any module."""
import argparse,hashlib,io,json,os,re,shlex,subprocess,tarfile
from pathlib import Path
STATE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/aston/kernel'
SSH=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','aston']
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--generation',type=int,default=27)
GENERATION=parser.parse_args().generation
assert 27<=GENERATION<=99
RELEASE=f'7.3.0-devices{GENERATION}-aston'
SOURCE=Path(f'/mnt/development/linux-perpetua-build/releases/devices-{GENERATION}-aston/modules-root/lib/modules')/RELEASE
REMOTE=r'''
from pathlib import Path
import hashlib,io,json,sys,tarfile
boot=Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "f683d9c9"' in boot
assert 'androidboot.slot_suffix = "_b"' in boot
raw=sys.stdin.buffer.read();assert hashlib.sha256(raw).hexdigest()==sys.argv[1]
with tarfile.open(fileobj=io.BytesIO(raw)) as tar:
 m=json.load(tar.extractfile('fingerprint-module-manifest.json'))
 assert m['release']==sys.argv[2]
 names=set(m['files'])|{'fingerprint-module-manifest.json'}
 assert len(tar.getmembers())==len(names)
 for x in tar:
  assert x.isfile() and x.name in names and not Path(x.name).is_absolute() and '..' not in Path(x.name).parts
 target=Path('/usr/lib/modules')/m['release']
 if not target.exists():
  staging=target.with_name('.'+target.name+'-'+sys.argv[1][:12]);staging.mkdir(mode=0o755)
  tar.extractall(staging,filter='data')
  for name,digest in m['files'].items():
   p=staging/name;assert hashlib.sha256(p.read_bytes()).hexdigest()==digest
   assert p.stat().st_uid==0
  staging.rename(target)
 else:
  for name,digest in m['files'].items():assert hashlib.sha256((target/name).read_bytes()).hexdigest()==digest
 print(json.dumps({'directory':str(target),'files':len(m['files']),'archive_sha256':sys.argv[1],'loaded':False}))
'''
def main():
 files={str(p.relative_to(SOURCE)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(SOURCE.rglob('*')) if p.is_file() and not p.is_symlink()}
 assert any('oplus-uff-fp.ko' in p for p in files)
 buf=io.BytesIO()
 with tarfile.open(fileobj=buf,mode='w') as tar:
  for name in files:tar.add(SOURCE/name,arcname=name,recursive=False)
  data=(json.dumps({'release':RELEASE,'files':files},indent=2)+'\n').encode()
  member=tarfile.TarInfo('fingerprint-module-manifest.json');member.size=len(data);member.mode=0o644;tar.addfile(member,io.BytesIO(data))
 data=buf.getvalue();digest=hashlib.sha256(data).hexdigest()
 cmd=shlex.join(['python3','-c',REMOTE,digest,RELEASE])
 p=subprocess.run([*SSH,cmd],input=data,stdout=subprocess.PIPE,check=True)
 STATE.mkdir(parents=True,exist_ok=True);(STATE/f'modules-staged-devices{GENERATION}.json').write_bytes(p.stdout);print(p.stdout.decode())
if __name__=='__main__':main()
