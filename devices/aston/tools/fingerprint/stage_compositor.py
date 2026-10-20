#!/usr/bin/python3
"""Prepare a native-only candidate and exact panel profile. No service restart."""
from pathlib import Path
import hashlib,json,shlex,subprocess
from run_probe import SSH,STAGE,STATE as R
m=json.loads((R/'compositor-build.json').read_text());source=Path(m['path'])/'deniald'
assert hashlib.sha256(source.read_bytes()).hexdigest()==m['sha256']
remote='/var/lib/aston-fingerprint/deniald-'+m['sha256'][:12]
subprocess.run([*SSH,shlex.join(['python3','-c',STAGE,remote,m['sha256'],'755'])],input=source.read_bytes(),check=True,capture_output=True)
previous=json.loads((R/'compositor-activation.json').read_text())
oldhash=previous['sha256']
profile={'sensor':'goodix-g7s-uff','output':'DSI-1','panel_name':'oneplus,aston-boe-aa551','panel_width':1264,'panel_height':2780,'x':535,'y':1928,'width':195,'height':195,'max_duration_ms':30000,'settle_ms':150,'illumination_backend':'aston_aa551_fd'}
code='''import hashlib,json,os,shutil,subprocess,sys
from pathlib import Path
cfg=json.load(sys.stdin)
sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
service=dict(x.split('=',1) for x in subprocess.check_output(['systemctl','show','denial.service','-p','MainPID','-p','User','-p','ActiveState'],text=True).splitlines())
assert service['User']=='logix' and service['ActiveState']=='active'
pid=int(service['MainPID']);exe=Path('/proc')/str(pid)/'exe';base=exe.resolve().parent.parent
assert base==Path(cfg['expected_base'])
assert sha(exe)==cfg['base_sha256']
target=base.parent/('fingerprint-v2-'+cfg['sha256'][:12])
assert not target.exists(),'Candidate already staged; verify its manifest separately'
shutil.copytree(base,target,copy_function=os.link,symlinks=True)
# Replace directory entries atomically: never truncate hard-linked live files.
q=target/'native/deniald.new';shutil.copy2(cfg['binary'],q);q.chmod(0o755)
os.replace(q,target/'native/deniald')
assert sha(target/'native/deniald')==cfg['sha256'] and sha(exe)==cfg['base_sha256']
for root in ['shell','settings']:
 for old in (base/root).rglob('*'):
  if old.is_file():assert sha(old)==sha(target/old.relative_to(base))
record=dict(candidate=str(target),active=str(base),active_pid=pid,selected=False,activated=False,sha256=cfg['sha256'],base_sha256=cfg['base_sha256'],engine_shell_settings='byte-identical to current candidate',profile=cfg['profile'])
q=target/'fingerprint-deployment.json';q.write_text(json.dumps(record,indent=2)+'\\n')
p=Path('/etc/denial/fingerprint.json');data=(json.dumps(cfg['profile'],indent=2)+'\\n').encode()
p.parent.mkdir(exist_ok=True)
if p.exists():assert p.read_bytes()==data,'Existing profile differs; preserve it before changes'
else:
 fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o644)
 with os.fdopen(fd,'wb') as out:out.write(data)
check=subprocess.run(['ldd',str(target/'native/deniald')],text=True,capture_output=True,check=True)
assert 'not found' not in check.stdout+check.stderr
record['ldd']=check.stdout;print(json.dumps(record))
'''
cfg={'binary':remote,'sha256':m['sha256'],'base_sha256':oldhash,'profile':profile,'expected_base':previous['candidate']}
p=subprocess.run([*SSH,shlex.join(['python3','-c',code])],input=json.dumps(cfg).encode(),capture_output=True)
if p.returncode:print(p.stderr.decode());raise SystemExit(p.returncode)
(R/'compositor-staged.json').write_bytes(p.stdout)
print('Compositor and profile staged, current service unchanged; dependencies resolved')
