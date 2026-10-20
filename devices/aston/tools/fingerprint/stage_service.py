#!/usr/bin/python3
"""Stage the verified bridge and run passive checks; no capture or UI event."""
from pathlib import Path
import hashlib,json,shlex,subprocess
from run_probe import SRC,SSH,STAGE,STATE as R
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
worker=json.loads((R/'build/worker-manifest.json').read_text())
firmware=json.loads((R/'build/firmware-manifest.json').read_text())['uff_gx']['sha256']
presentation=json.loads((R/'build/presentation/manifest.json').read_text())
config={'format':1}
for key,path,digest in [('worker',R/'build/uff-service-worker',worker['output_sha256']),('firmware',R/'build/uff_gx.elf',firmware),('presentation',R/'build/presentation/uff-denial-presentation',presentation['sha256']),('helper',SRC/'uff-fprint-helper.py',sha(SRC/'uff-fprint-helper.py'))]:
 assert sha(path)==digest
 remote='/var/lib/aston-fingerprint/'+path.name+'-'+digest[:12]
 subprocess.run([*SSH,shlex.join(['python3','-c',STAGE,remote,digest,'600' if key=='firmware' else '700'])],input=path.read_bytes(),check=True,capture_output=True)
 config[key]={'path':remote,'sha256':digest}
if (R/'service-staged.json').exists():
 previous_code="from pathlib import Path;import json,sys;p=Path('/var/lib/aston-fingerprint/backend-previous-approved.json');data=sys.stdin.buffer.read();assert json.loads(data)==json.loads(Path('/etc/aston-fingerprint/backend.json').read_bytes());p.write_bytes(data);p.chmod(0o600)"
 subprocess.run([*SSH,shlex.join(['python3','-c',previous_code])],input=(R/'service-staged.json').read_bytes(),check=True)
install='''import json,os,sys
from pathlib import Path
c=json.load(sys.stdin);p=Path('/etc/aston-fingerprint');p.mkdir(mode=0o700,exist_ok=True)
f=p/'backend.json';data=(json.dumps(c,indent=2)+'\\n').encode()
if f.exists():
 previous=json.loads(Path('/var/lib/aston-fingerprint/backend-previous-approved.json').read_text())
 assert json.loads(f.read_text())==previous,'Backend changed since verified staging'
 if f.read_bytes()!=data:
  import hashlib
  backup=f.with_name('backend-'+hashlib.sha256(f.read_bytes()).hexdigest()[:12]+'.json')
  if not backup.exists():backup.write_bytes(f.read_bytes());backup.chmod(0o600)
  temporary=f.with_suffix('.new');assert not temporary.exists();temporary.write_bytes(data);temporary.chmod(0o600);temporary.replace(f)
else:
 fd=os.open(f,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
 with os.fdopen(fd,'wb') as s:s.write(data)
p=Path('/usr/local/libexec/uff-fprint-helper');p.parent.mkdir(parents=True,exist_ok=True)
if p.exists() or p.is_symlink():
 assert p.is_symlink() and os.readlink(p) in (c['helper']['path'],previous['helper']['path'])
 if os.readlink(p)!=c['helper']['path']:
  temporary=p.with_suffix('.new');assert not temporary.exists();temporary.symlink_to(c['helper']['path']);temporary.replace(p)
else:p.symlink_to(c['helper']['path'])
print(json.dumps(c))
'''
p=subprocess.run([*SSH,shlex.join(['python3','-c',install])],input=json.dumps(config).encode(),capture_output=True,check=True)
(R/'service-staged.json').write_bytes(p.stdout)
check='''import json,subprocess,sys
records=[]
for op in ['probe','open','list']:
 p=subprocess.Popen([sys.argv[1],op],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 # Keep the control pipe open like libfprint; communicate() would signal cancellation.
 import selectors,time
 out=[];sel=selectors.DefaultSelector();sel.register(p.stdout,selectors.EVENT_READ)
 deadline=time.monotonic()+30
 while p.poll() is None:
  if time.monotonic()>deadline:p.terminate();raise RuntimeError('helper timeout')
  for key,_ in sel.select(.1):
   line=key.fileobj.readline()
   if line:out.append(line)
   else:sel.unregister(key.fileobj)
 out.append(p.stdout.read());stderr=p.stderr.read();p.stdin.close()
 records.append(dict(operation=op,returncode=p.returncode,stdout=''.join(out),stderr=stderr))
 lines=''.join(out).splitlines()
 assert p.returncode==0 and lines and lines[-1]=='UFF1 result 0 0',records[-1]
 if op=='list':
  import re
  ids=[]
  for line in lines[:-1]:
   assert re.fullmatch('UFF1 item [1-9][0-9]{0,9}',line),line
   value=int(line.split()[2]);assert value<=0xffffffff and value not in ids;ids.append(value)
  assert len(ids)<=5
 else:assert len(lines)==1
print(json.dumps(records))
'''
p=subprocess.run([*SSH,shlex.join(['python3','-c',check,config['helper']['path']])],capture_output=True,check=True)
(R/'helper-passive-test.json').write_bytes(p.stdout)
print('Verified helper probe/open/list: PASS; no presentation or capture requested')
