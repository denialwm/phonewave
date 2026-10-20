#!/usr/bin/python3
"""Install the exact Ace3 firmware gesture profile; no UI/test event or restart."""
from pathlib import Path
import hashlib,json,shlex,subprocess
from run_probe import SSH,STATE as R
profile=Path('/home/logix/denial/docs/hardware/aston-wake-gesture.json').read_text()
assert json.loads(profile)==dict(input_name='synaptics_tcm_gesture',key_code=143,output='DSI-1')
rule='ACTION!="remove", SUBSYSTEM=="input", KERNEL=="event*", ATTRS{name}=="synaptics_tcm_gesture", ENV{LIBINPUT_IGNORE_DEVICE}="1"\n'
code='''from pathlib import Path
import hashlib,json,os,subprocess,sys
b=Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "f683d9c9"' in b and 'androidboot.slot_suffix = "_b"' in b
payload=json.load(sys.stdin);records=[]
for path,data in payload.items():
 p=Path(path);p.parent.mkdir(parents=True,exist_ok=True)
 if p.exists():assert not p.is_symlink() and p.read_text()==data,'Unexpected existing gesture setup'
 else:
  fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o644)
  with os.fdopen(fd,'w') as f:f.write(data)
 assert p.stat().st_uid==0 and p.stat().st_mode&0o022==0
 records.append(dict(path=path,sha256=hashlib.sha256(p.read_bytes()).hexdigest()))
subprocess.run(['udevadm','control','--reload-rules'],check=True)
print(json.dumps(records))
'''
payload={'/etc/denial/wake-gesture.json':profile,'/etc/udev/rules.d/72-aston-wake-gesture.rules':rule}
p=subprocess.run([*SSH,shlex.join(['python3','-c',code])],input=json.dumps(payload).encode(),capture_output=True,check=True)
(R/'gesture-profile-install.json').write_bytes(p.stdout)
print(p.stdout.decode())
