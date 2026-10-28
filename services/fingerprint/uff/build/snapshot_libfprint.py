#!/usr/bin/python3
"""Make the uff libfprint source archive: upstream pin, Phonewave patches, uff driver.
The driver serves every UFF1 phone; the argument names the phone whose cache gets it."""
from pathlib import Path
import gzip,hashlib,io,json,os,shlex,subprocess,sys,tarfile,tempfile
SRC=Path(__file__).resolve().parents[1]
CACHE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave'
DEVICE=sys.argv[1] if len(sys.argv)>1 else 'aston'
assert DEVICE in ('aston','nx809j'),'unknown phone'
OUT=CACHE/DEVICE/'fingerprint/libfprint'
URL,PIN='https://gitlab.freedesktop.org/libfprint/libfprint.git','80a4b5ec612892c5c056c48dddaa561452cf37ec' # v1.94.100
PATCHES=[*sorted((SRC.parent/'jiiov/libfprint').glob('*.patch')),*sorted((SRC/'libfprint').glob('*.patch'))]
OVERLAY={'libfprint/drivers/uff.c':SRC/'libfprint/uff.c','tests/test-uff-suspend.c':SRC/'libfprint/test-uff-suspend.c'}
repo=CACHE/'src/libfprint'
if not repo.exists():subprocess.run(['git','clone','-q',URL,str(repo)],check=True)
OUT.mkdir(parents=True,exist_ok=True);files={};data=io.BytesIO()
with tempfile.TemporaryDirectory() as t:
 tree=Path(t)
 subprocess.run(f'git -C {shlex.quote(str(repo))} archive {PIN} | tar -x -C {shlex.quote(t)}',shell=True,check=True)
 for p in PATCHES:subprocess.run(['patch','-p1','-s','--no-backup-if-mismatch','-d',str(tree),'-i',str(p)],check=True)
 for name,p in OVERLAY.items():(tree/name).write_bytes(p.read_bytes())
 with gzip.GzipFile(filename='',mode='wb',fileobj=data,mtime=0) as z,tarfile.open(fileobj=z,mode='w',format=tarfile.GNU_FORMAT) as tar:
  for p in sorted(x for x in tree.rglob('*') if x.is_file() and not x.is_symlink()):
   name=str(p.relative_to(tree));b=p.read_bytes()
   member=tarfile.TarInfo(name);member.size=len(b);member.mode=0o755 if os.access(p,os.X_OK) else 0o644
   tar.addfile(member,io.BytesIO(b));files[name]=hashlib.sha256(b).hexdigest()
digest=hashlib.sha256(data.getvalue()).hexdigest();archive=OUT/('libfprint-uff-source-'+digest[:12]+'.tar.gz')
if not archive.exists():archive.write_bytes(data.getvalue())
m=dict(archive=str(archive),archive_sha256=digest,upstream=URL,base_head=PIN,patches=[str(p) for p in PATCHES],overlay_files=sorted(OVERLAY),selected_driver='uff',activated=False,files=files)
(OUT/'source-manifest.json').write_text(json.dumps(m,indent=2)+'\n')
print(digest)
