#!/usr/bin/python3
"""Activate a staged, explicitly user-authorized compositor update; no rollback."""
from pathlib import Path
import hashlib,json,shlex,subprocess
from run_probe import SSH,STATE as R
s=json.loads((R/'compositor-staged.json').read_text())
code='''import hashlib,json,subprocess,sys,time
from pathlib import Path
s=json.load(sys.stdin);sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
boot=Path('/proc/bootconfig').read_text();assert 'androidboot.serialno = "f683d9c9"' in boot and 'androidboot.slot_suffix = "_b"' in boot
pid=int(subprocess.check_output(['systemctl','show','denial.service','-p','MainPID','--value'],text=True))
assert sha('/proc/'+str(pid)+'/exe')==s['base_sha256']
candidate=Path(s['candidate']);assert sha(candidate/'native/deniald')==s['sha256']
old=subprocess.check_output(['systemctl','cat','denial.service'],text=True)
backup=Path('/var/lib/aston-fingerprint/denial-service-before-'+s['sha256'][:12]+'.txt');assert not backup.exists();backup.write_text(old);backup.chmod(0o600)
base=s['active']
data='[Service]\\nExecStart=\\nExecStart='+str(candidate/'native/deniald')+' --device /dev/dri/card0 --render-device /dev/dri/renderD128 --output-config /home/logix/.config/denial/outputs.conf --wayland --flutter-bundle '+base+'/shell\\n'
p=Path('/etc/systemd/system/denial.service.d/zz-fingerprint.conf');assert p.is_file()
tmp=p.with_suffix('.tmp');assert not tmp.exists();tmp.write_text(data);tmp.chmod(0o644);tmp.replace(p)
subprocess.run(['systemctl','daemon-reload'],check=True)
subprocess.run(['systemctl','restart','denial.service'],check=True,timeout=40)
new=int(subprocess.check_output(['systemctl','show','denial.service','-p','MainPID','--value'],text=True));assert new and new!=pid
for attempt in range(50):
 if sha('/proc/'+str(new)+'/exe')==s['sha256']:break
 time.sleep(.1)
else:raise RuntimeError('Compositor did not exec the selected binary')
assert subprocess.check_output(['systemctl','is-active','denial.service'],text=True).strip()=='active'
print(json.dumps(dict(previous_pid=pid,pid=new,candidate=str(candidate),sha256=s['sha256'],backup=str(backup),selected=True,active=True)))
'''
p=subprocess.run([*SSH,shlex.join(['python3','-c',code])],input=json.dumps(s).encode(),capture_output=True)
record=R/'compositor-activation.json'
if record.exists():
 old=json.loads(record.read_text());backup=R/('compositor-activation-'+old['sha256'][:12]+'.json')
 if not backup.exists():backup.write_bytes(record.read_bytes())
if p.returncode==0:record.write_bytes(p.stdout)
else:(R/'compositor-activation-failed.log').write_bytes(p.stdout+p.stderr)
print(p.stdout.decode());print(p.stderr.decode());raise SystemExit(p.returncode)
