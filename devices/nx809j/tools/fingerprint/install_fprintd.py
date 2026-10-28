#!/usr/bin/env python3
"""Point the RedMagic's fprintd at the library build_libfprint.py made.

A drop-in sets its library path and opens what the Goodix helper and worker
need inside fprintd's sandbox: the TEE, the sensor's control node, DMA
buffers, touch input, their state folders and the session's Wayland socket.
It also lets the driver give them a utilization floor: RestrictRealtime
filters sched_setattr whole, uclamp included.
"""
from pathlib import Path
import json,os,shlex,subprocess
STATE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/nx809j/fingerprint'
SSH=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','nx809j']
library=json.loads((STATE/'libfprint/build-manifest.json').read_text())
config=f'''[Service]
Environment=LD_LIBRARY_PATH={library['prefix']}/lib
ReadWritePaths=-/var/lib/nx809j-fingerprint -/run/nx809j-fingerprint
ProtectHome=tmpfs
BindReadOnlyPaths=-/run/user/1000
DeviceAllow=/dev/tee0 rw
DeviceAllow=/dev/goodix_fp rw
DeviceAllow=/dev/dma_heap/system rw
DeviceAllow=char-input r
RestrictRealtime=no
'''
CODE='''import hashlib,json,subprocess,sys
from pathlib import Path
assert 'nubia,nx809j' in Path('/proc/device-tree/compatible').read_text()
c=json.load(sys.stdin);lib=Path(c['prefix'])/'lib/libfprint-2.so.2'
assert hashlib.sha256(lib.read_bytes()).hexdigest()==c['library_sha256']
p=Path('/etc/systemd/system/fprintd.service.d/60-nx809j-native.conf');p.parent.mkdir(parents=True,exist_ok=True)
temporary=p.with_suffix('.new');temporary.write_text(c['config']);temporary.chmod(0o644);temporary.replace(p)
subprocess.run(['systemctl','daemon-reload'],check=True)
subprocess.run(['systemctl','restart','fprintd.service'],check=True,timeout=20)
pid=int(subprocess.check_output(['systemctl','show','fprintd.service','-p','MainPID','--value'],text=True))
assert str(lib.parent) in Path(f'/proc/{pid}/maps').read_text(),'fprintd did not load the native library'
print('fprintd',pid,'uses',lib.parent)
print(subprocess.run(['busctl','call','net.reactivated.Fprint','/net/reactivated/Fprint/Manager','net.reactivated.Fprint.Manager','GetDevices'],capture_output=True,text=True).stdout.strip())
'''
p=subprocess.run([*SSH,shlex.join(['python3','-c',CODE])],input=json.dumps(dict(library,config=config)).encode(),capture_output=True)
print(p.stdout.decode()+p.stderr.decode());raise SystemExit(p.returncode)
