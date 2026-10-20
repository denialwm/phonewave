#!/usr/bin/env python3
"""Build the ace3 trusted presentation client. This never launches it."""
from pathlib import Path
import hashlib,json,os,shutil,subprocess
R=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/aston/fingerprint';OUT=R/'build/presentation';OUT.mkdir(parents=True,exist_ok=True)
SRC=Path(__file__).resolve().parents[1]
SYS=Path('/mnt/development/cross/sysroot')
K=Path('/mnt/development/linux-perpetua')
header=K/'include/uapi/linux/aston_fp_illumination.h'
(R/'build/include/linux').mkdir(parents=True,exist_ok=True)
shutil.copyfile(header,R/'build/include/linux/aston_fp_illumination.h')
protocols={'denial-fingerprint':Path('/home/logix/denial/compositor/protocol/denial-fingerprint-v1.xml'),'linux-dmabuf':Path('/usr/share/wayland-protocols/stable/linux-dmabuf/linux-dmabuf-v1.xml')}
inputs=[Path(__file__).resolve(),SRC/'uff-denial-presentation.c',SRC/'uff-contact.h',header]
for name,xml in protocols.items():
 inputs.append(xml)
 for mode,suffix in [('client-header','-client-protocol.h'),('private-code','-protocol.c')]:
  path=OUT/(name+suffix);subprocess.run(['wayland-scanner',mode,str(xml),str(path)],check=True);inputs.append(path)
command=['aarch64-linux-gnu-gcc','--sysroot='+str(SYS),'-O2','-Wall','-Wextra','-Werror','-I'+str(OUT),'-I'+str(R/'build/include'),'-I'+str(SYS/'usr/include'),str(SRC/'uff-denial-presentation.c'),*[str(OUT/(name+'-protocol.c')) for name in protocols],'-L'+str(SYS/'usr/lib'),'-L'+str(SYS/'lib'),'-Wl,-rpath-link,'+str(SYS/'lib'),'-Wl,-rpath-link,'+str(SYS/'usr/lib'),'-lwayland-client','-o',str(OUT/'uff-denial-presentation')]
subprocess.run(command,check=True)
inputs.extend([SYS/'usr/lib/libwayland-client.so',SYS/'usr/lib/libffi.so.8',SYS/'lib/libc.so.6',SYS/'lib/libc_nonshared.a',SYS/'lib/ld-linux-aarch64.so.1'])
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
m={'command':command,'inputs':{str(p):sha(p) for p in inputs},'sha256':sha(OUT/'uff-denial-presentation'),'compiler':subprocess.check_output([command[0],'--version'],text=True)}
(OUT/'manifest.json').write_text(json.dumps(m,indent=2)+'\n')
archive=OUT/'releases'/m['sha256'];archive.mkdir(parents=True,exist_ok=True)
for source in [OUT/'uff-denial-presentation',OUT/'manifest.json',*inputs]:
 target=archive/source.name
 if not target.exists():shutil.copy2(source,target)
print(m['sha256'])
