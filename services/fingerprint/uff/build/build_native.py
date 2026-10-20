#!/usr/bin/env python3
"""Build only the native fingerprint probe, preserving input/output hashes."""
import hashlib,json,os,shutil,subprocess
from pathlib import Path
SRC=Path(__file__).resolve().parents[1]
KERNEL=Path('/mnt/development/linux-perpetua')
CACHE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave'
BUILD=CACHE/'aston/fingerprint/build'
# Upstream pins; patches in deps/<name>/ apply on top.
PINS={'quic-teec':('https://github.com/qualcomm/quic-teec.git','736419e25a2036aac3292a10a93e394a90750ca3'),
 'qcbor':('https://github.com/laurencelundblade/QCBOR.git','a3fce5f34d7b30f12d7b9161185171c6fe137737'),
 'minkipc':('https://github.com/qualcomm/minkipc.git','6ab4aedfbbacd366f0e064c1c72b09ad54b31761')}
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def checkout(name):
 url,pin=PINS[name];d=CACHE/'src'/name
 if not d.exists():subprocess.run(['git','clone','-q',url,str(d)],check=True)
 subprocess.run(['git','-C',str(d),'checkout','-q','--detach',pin],check=True)
 for patch in sorted((SRC/'deps'/name).glob('*.patch')):
  if subprocess.run(['git','-C',str(d),'apply','--reverse','--check',str(patch)],capture_output=True).returncode:
   subprocess.run(['git','-C',str(d),'apply',str(patch)],check=True)
 return d
def static_lib(name,sources,includes):
 out=BUILD/'deps';out.mkdir(parents=True,exist_ok=True);objects=[]
 for s in sources:
  o=out/(s.name+'.o');objects.append(o)
  subprocess.run(['aarch64-linux-gnu-gcc','-DUSE_QCBOR','-D_GNU_SOURCE',*['-I'+str(i) for i in includes],'-std=gnu11','-fPIC','-c',str(s),'-o',str(o)],check=True)
 lib=out/f'lib{name}.a';lib.unlink(missing_ok=True)
 subprocess.run(['aarch64-linux-gnu-ar','qc',str(lib),*map(str,objects)],check=True)
 subprocess.run(['aarch64-linux-gnu-ranlib',str(lib)],check=True)
 return lib
def main():
 (BUILD/'include/linux').mkdir(parents=True,exist_ok=True)
 header=KERNEL/'include/uapi/linux/uff_fp.h'
 shutil.copyfile(header,BUILD/'include/linux/uff_fp.h')
 teec,qcbor,mink=checkout('quic-teec'),checkout('qcbor'),checkout('minkipc')
 (BUILD/'deps/include/linux').mkdir(parents=True,exist_ok=True)
 shutil.copyfile(KERNEL/'include/uapi/linux/tee.h',BUILD/'deps/include/linux/tee.h')
 includes=[BUILD/'deps/include',teec/'libqcomtee/include',qcbor/'inc']
 libs=[static_lib('qcomtee',[teec/'libqcomtee/src'/f for f in ['qcomtee_object.c','objects/credentials_obj.c','objects/mem_obj.c']],[*includes,teec/'libqcomtee/src']),
  static_lib('qcbor',[qcbor/'src'/f for f in ['UsefulBuf.c','ieee754.c','qcbor_encode.c','qcbor_decode.c','qcbor_err_to_str.c']],includes)]
 cmd=['aarch64-linux-gnu-gcc','-std=gnu11','-Wall','-Wextra','-Werror','-O2','-g','-static','-pthread','-I'+str(BUILD/'include'),'-I'+str(teec/'libqcomtee/include'),*['-I'+str(mink/'listeners/libfsservice'/d) for d in ['fs','gpfs','cmn']],str(SRC/'uff-probe.c'),*map(str,libs),'-o',str(BUILD/'uff-probe')]
 subprocess.run(cmd,check=True)
 inputs=[Path(__file__).resolve(),SRC/'uff-probe.c',SRC/'uff-storage.h',SRC/'uff-gp-storage.h',SRC/'uff-template-names.h',SRC/'uff-wire.h',header,*libs,*sorted((mink/'listeners/libfsservice').glob('*/*.h')),*sorted((teec/'libqcomtee/include').glob('*.h'))]
 m={'command':cmd,'compiler':subprocess.check_output([cmd[0],'--version'],text=True),'inputs':{str(p):sha(p) for p in inputs},'output_sha256':sha(BUILD/'uff-probe')}
 (BUILD/'manifest.json').write_text(json.dumps(m,indent=2)+'\n')
 archive=BUILD/'releases'/(m['output_sha256']+'-'+sha(BUILD/'manifest.json')[:12]);archive.mkdir(parents=True,exist_ok=True)
 for source in [BUILD/'uff-probe',BUILD/'manifest.json',SRC/'uff-probe.c',SRC/'uff-storage.h',SRC/'uff-gp-storage.h',SRC/'uff-template-names.h',SRC/'uff-wire.h',header,*libs]:
  target=archive/source.name
  if target.exists():assert target.read_bytes()==source.read_bytes()
  else:shutil.copyfile(source,target)
 print('Built native UFF probe:',m['output_sha256'])
 worker_cmd=cmd[:];worker_cmd[worker_cmd.index(str(SRC/'uff-probe.c'))]=str(SRC/'uff-service-worker.c');worker_cmd[-1]=str(BUILD/'uff-service-worker')
 subprocess.run(worker_cmd,check=True)
 worker_inputs=[SRC/'uff-service-worker.c',SRC/'uff-resident.h',*inputs]
 wm={'command':worker_cmd,'compiler':m['compiler'],'inputs':{str(p):sha(p) for p in worker_inputs},'output_sha256':sha(BUILD/'uff-service-worker')}
 (BUILD/'worker-manifest.json').write_text(json.dumps(wm,indent=2)+'\n')
 wa=BUILD/'releases'/(wm['output_sha256']+'-'+sha(BUILD/'worker-manifest.json')[:12]);wa.mkdir(parents=True,exist_ok=True)
 for p in [BUILD/'uff-service-worker',BUILD/'worker-manifest.json',*worker_inputs]:
  dest=wa/p.name
  if not dest.exists():shutil.copyfile(p,dest)
 print('Built native UFF service child:',wm['output_sha256'])
if __name__=='__main__':main()
