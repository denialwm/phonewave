#!/usr/bin/env python3
"""Acquire only fingerprint factory files from Aston slot-B logical partitions.

Read-only loop mounts live in a private namespace. No partition is written.
Factory calibration and enrolled templates are deliberately excluded.
"""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile

ROOT = Path(os.environ.get('PHONEWAVE_VAULT', '/mnt/development/device-vault')) / 'aston/firmware'
SSH = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=5', 'aston']
REMOTE = r'''
from pathlib import Path
import hashlib,io,json,subprocess,sys,tarfile,tempfile
boot=Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "f683d9c9"' in boot
assert 'androidboot.slot_suffix = "_b"' in boot
parts=[('odm_b',9224192,2639128,'de964152e44049c4c4d15656c3ddb6ac5bf7ce64c873910bceb36d2f3907e7e1')]
records=[]
with tarfile.open(fileobj=sys.stdout.buffer,mode='w|') as archive:
 for name,sector,count,expected in parts:
  with tempfile.TemporaryDirectory(prefix='aston-fingerprint-',dir='/run') as d:
   loop=subprocess.check_output(['losetup','--find','--show','--read-only','--offset',str(sector*512),'--sizelimit',str(count*512),'/dev/disk/by-partlabel/super'],text=True).strip()
   mounted=False
   try:
    digest=hashlib.sha256()
    with open(loop,'rb') as f:
     for b in iter(lambda:f.read(4*1024*1024),b''):digest.update(b)
    assert digest.hexdigest()==expected, (name,'factory image changed')
    subprocess.run(['mount','-o','ro',loop,d],check=True);mounted=True
    info=[l for l in Path('/proc/self/mountinfo').read_text().splitlines() if l.split()[4]==d]
    assert len(info)==1 and 'ro' in info[0].split()[5].split(',')
    selected=[Path(d)/'bin/hw/vendor.oplus.hardware.biometrics.fingerprint@2.1-service_uff',Path(d)/'etc/init/vendor.oplus.hardware.biometrics.fingerprint@2.1-service.rc',Path(d)/'etc/vintf/manifest/manifest_oplus_fingerprint_aidl_v2.xml']
    selected+=sorted((Path(d)/'vendor/firmware').glob('uff_*'))
    for p in selected:
     assert p.is_file() and not p.is_symlink() and p.stat().st_size<32*1024*1024
     rel='factory/'+name+'/'+str(p.relative_to(d));data=p.read_bytes()
     entry=tarfile.TarInfo(rel);entry.size=len(data);entry.mode=0o600
     archive.addfile(entry,io.BytesIO(data))
     records.append({'path':rel,'size':len(data),'sha256':hashlib.sha256(data).hexdigest()})
   finally:
    if mounted:subprocess.run(['umount',d],check=True)
    subprocess.run(['losetup','--detach',loop],check=True)
 manifest={'serial':'f683d9c9','slot':'_b','kernel':Path('/proc/sys/kernel/osrelease').read_text().strip(),'source':'/dev/disk/by-partlabel/super','partitions':[{'name':n,'offset':s*512,'bytes':c*512,'sha256':h} for n,s,c,h in parts],'files':records}
 data=(json.dumps(manifest,indent=2)+'\n').encode();entry=tarfile.TarInfo('factory-manifest.json');entry.size=len(data);entry.mode=0o600;archive.addfile(entry,io.BytesIO(data))
'''

def main():
    result = subprocess.run([*SSH, 'unshare --mount --propagation private python3 -'],
                            input=REMOTE.encode(), stdout=subprocess.PIPE, check=True)
    with tarfile.open(fileobj=io.BytesIO(result.stdout)) as archive:
        manifest = json.load(archive.extractfile('factory-manifest.json'))
        expected = {x['path']: x for x in manifest['files']}
        assert len(archive.getmembers()) == len(expected) + 1
        for member in archive:
            assert member.isfile() and member.size < 32 * 1024 * 1024
            assert member.name == 'factory-manifest.json' or member.name in expected
            assert not Path(member.name).is_absolute() and '..' not in Path(member.name).parts
            data = archive.extractfile(member).read()
            if member.name in expected:
                assert hashlib.sha256(data).hexdigest() == expected[member.name]['sha256']
            target = ROOT / member.name
            if target.exists():
                assert target.read_bytes() == data, f'Refusing to overwrite changed evidence: {target}'
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
                target.chmod(0o600)
    print(json.dumps({'files': len(expected), 'partition_sha256': manifest['partitions'][0]['sha256']}))

if __name__ == '__main__':
    main()
