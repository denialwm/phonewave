#!/usr/bin/env python3
"""Install what fprintd's uff driver runs on the RedMagic.

- /usr/local/libexec/goodix-fprint-helper: the UFF1 helper, as on the Aston.
- /usr/local/libexec/nx809j-fingerprint/: the Goodix worker, its signed app
  and the contact child, built here from services/fingerprint/goodix.
- /etc/nx809j-fingerprint/backend.json: their paths and hashes, which the
  helper checks before every operation.
- /var/lib/nx809j-fingerprint/calibration: this unit's factory calibration,
  copied once from persist's goodix/ with persist mounted read-only, and
  templates/, the worker's private store.
- /usr/libexec/goodix-gatekeeper-worker and rootfs/'s nx809j-gatekeeper
  socket and service, which keep the lock PIN; the socket is enabled.
- /run/nx809j-fingerprint, created at boot: where a warm verification
  worker waits for the next attempt.
- /usr/libexec/goodix-secure-storage and its service/read-only mount: private
  Linux backing-file updates, with stock persist and hardware RPMB read-only.
- sudo, with rootfs/'s rule letting logix start only `deniald
  --fingerprint-settings` as root, without a password: deniald asks the
  lock PIN itself, as Denial's fingerprint settings do.

Run services/fingerprint/goodix/build.py first; it leaves the worker and the
app in the cache folder this reads. fprintd restarts to find the reader.
"""
from pathlib import Path
import hashlib,io,json,os,shlex,subprocess,tarfile
ROOT=Path(__file__).resolve().parents[4]
STATE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/nx809j/fingerprint'
SSH=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','nx809j']
CC=ROOT.parent/'cross/aarch64-linux-gnu-gcc-droidloom'
GOODIX=ROOT/'services/fingerprint/goodix'

subprocess.run([str(CC),'-std=gnu11','-O2','-Wall','-Wextra','-Werror','-static','-o',str(STATE/'goodix-contact'),
                str(GOODIX/'goodix-contact.c')],check=True)
DEVICE=ROOT/'devices/nx809j/rootfs'
UNITS=DEVICE/'usr/lib/systemd/system'
FILES={'goodix-fprint-helper':STATE/'goodix-fprint-helper',
       'goodix-service-worker':STATE/'goodix-service-worker','goodixfp64.elf':STATE/'goodixfp64.elf',
       'goodix-contact':STATE/'goodix-contact','goodix-gatekeeper-worker':STATE/'goodix-gatekeeper-worker',
       'goodix-secure-storage':STATE/'goodix-secure-storage',
       'nx809j-secure-storage.service':UNITS/'nx809j-secure-storage.service',
       'run-nx809j-securestorage.mount':UNITS/'run-nx809j-securestorage.mount',
       'nx809j-secure-storage.conf':DEVICE/'usr/lib/tmpfiles.d/nx809j-secure-storage.conf',
       'nx809j-fingerprint.conf':DEVICE/'usr/lib/tmpfiles.d/nx809j-fingerprint.conf',
       '70-nx809j-secure-storage.conf':DEVICE/'etc/systemd/system/fprintd.service.d/70-nx809j-secure-storage.conf',
       'nx809j-gatekeeper.socket':UNITS/'nx809j-gatekeeper.socket',
       'nx809j-gatekeeper@.service':UNITS/'nx809j-gatekeeper@.service',
       'nx809j-fingerprint.sudoers':DEVICE/'etc/sudoers.d/nx809j-fingerprint'}
data=io.BytesIO()
with tarfile.open(fileobj=data,mode='w') as tar:
    for name,path in FILES.items():tar.add(path,arcname=name)
digests={name:hashlib.sha256(path.read_bytes()).hexdigest() for name,path in FILES.items()}

CODE=r'''
from pathlib import Path
import io,json,os,shutil,subprocess,sys,tarfile,tempfile
assert 'nubia,nx809j' in Path('/proc/device-tree/compatible').read_text()
assert 'androidboot.slot_suffix = "_b"' in Path('/proc/bootconfig').read_text()
digests=json.loads(sys.argv[1])
def folder(path,mode):
    path.mkdir(mode=mode,parents=True,exist_ok=True);os.chown(path,0,0);path.chmod(mode)
def put(path,data,mode):
    path.parent.mkdir(parents=True,exist_ok=True)
    temporary=path.with_name('.'+path.name+'.new')
    temporary.unlink(missing_ok=True)
    with open(temporary,'xb') as f:f.write(data)
    os.chown(temporary,0,0);temporary.chmod(mode);temporary.replace(path)
subprocess.run(['pacman','-S','--needed','--noconfirm','sudo'],check=True,stdout=subprocess.DEVNULL)
libexec=Path('/usr/local/libexec');folder(libexec,0o755)
own=libexec/'nx809j-fingerprint';folder(own,0o755)
units=Path('/etc/systemd/system')
# Arch has no /usr/libexec; nx809j-gatekeeper@.service runs its worker from there.
folder(Path('/usr/libexec'),0o755)
places={'goodix-fprint-helper':(libexec/'goodix-fprint-helper',0o755),
        'goodix-service-worker':(own/'goodix-service-worker',0o700),
        'goodix-contact':(own/'goodix-contact',0o700),
        'goodixfp64.elf':(own/'goodixfp64.elf',0o600),
        'goodix-gatekeeper-worker':(Path('/usr/libexec/goodix-gatekeeper-worker'),0o755),
        'goodix-secure-storage':(Path('/usr/libexec/goodix-secure-storage'),0o700),
        'nx809j-secure-storage.service':(units/'nx809j-secure-storage.service',0o644),
        'run-nx809j-securestorage.mount':(units/'run-nx809j-securestorage.mount',0o644),
        'nx809j-secure-storage.conf':(Path('/usr/lib/tmpfiles.d/nx809j-secure-storage.conf'),0o644),
        'nx809j-fingerprint.conf':(Path('/usr/lib/tmpfiles.d/nx809j-fingerprint.conf'),0o644),
        '70-nx809j-secure-storage.conf':(units/'fprintd.service.d/70-nx809j-secure-storage.conf',0o644),
        'nx809j-gatekeeper.socket':(units/'nx809j-gatekeeper.socket',0o644),
        'nx809j-gatekeeper@.service':(units/'nx809j-gatekeeper@.service',0o644),
        'nx809j-fingerprint.sudoers':(Path('/etc/sudoers.d/nx809j-fingerprint'),0o440)}
with tarfile.open(fileobj=io.BytesIO(sys.stdin.buffer.read())) as tar:
    for member in tar:
        path,mode=places[member.name]
        data=tar.extractfile(member).read()
        if path.suffix=='' and path.parent.name=='sudoers.d':
            # A broken rule would lock sudo out: check it before it lands.
            check=Path('/run/nx809j-fingerprint.sudoers.check');check.write_bytes(data)
            subprocess.run(['visudo','-cqf',str(check)],check=True);check.unlink()
        put(path,data,mode)
subprocess.run(['systemctl','daemon-reload'],check=True)
etc=Path('/etc/nx809j-fingerprint');folder(etc,0o755)
entry=lambda name:{'path':str(places[name][0]),'sha256':digests[name]}
put(etc/'backend.json',(json.dumps({'format':1,'worker':entry('goodix-service-worker'),
    'firmware':entry('goodixfp64.elf'),'contact':entry('goodix-contact')},indent=2)+'\n').encode(),0o644)
state=Path('/var/lib/nx809j-fingerprint');folder(state,0o700);folder(state/'templates',0o700)
calibration=state/'calibration'
if not calibration.exists():
    # Read-only and without journal replay: persist is never written.
    mount=Path(tempfile.mkdtemp(prefix='nx809j-persist-',dir='/run'))
    subprocess.run(['mount','-t','ext4','-o','ro,noload,nosuid,nodev,noexec',
                    '/dev/disk/by-partlabel/persist',str(mount)],check=True)
    try:
        copy=state/'.calibration.new';shutil.rmtree(copy,ignore_errors=True)
        shutil.copytree(mount/'goodix',copy,symlinks=False)
    finally:
        subprocess.run(['umount',str(mount)],check=True);mount.rmdir()
    for path in sorted(copy.rglob('*'),reverse=True):
        os.chown(path,0,0,follow_symlinks=False);path.chmod(0o500 if path.is_dir() else 0o400)
    os.chown(copy,0,0);copy.chmod(0o500);copy.replace(calibration)
files=[p for p in calibration.rglob('*') if p.is_file()]
print('calibration:',len(files),'files,',sum(p.stat().st_size for p in files),'bytes')
folder(Path('/run/nx809j'),0o700)
subprocess.run(['systemd-tmpfiles','--create','/usr/lib/tmpfiles.d/nx809j-fingerprint.conf'],check=True)
subprocess.run(['systemctl','enable','--now','nx809j-secure-storage.service'],check=True)
subprocess.run(['systemctl','enable','--now','nx809j-gatekeeper.socket'],check=True)
subprocess.run(['systemctl','restart','fprintd.service'],check=True,timeout=20)
print(subprocess.run(['busctl','call','net.reactivated.Fprint','/net/reactivated/Fprint/Manager',
    'net.reactivated.Fprint.Manager','GetDevices'],capture_output=True,text=True).stdout.strip())
'''
p=subprocess.run([*SSH,shlex.join(['python3','-c',CODE,json.dumps(digests)])],input=data.getvalue(),capture_output=True)
print(p.stdout.decode()+p.stderr.decode());raise SystemExit(p.returncode)
