#!/usr/bin/env python3
"""Stage verified native probe/firmware and perform sensor identification only."""
import argparse,hashlib,json,os,shlex,subprocess,time
from pathlib import Path
STATE=Path(os.environ.get('XDG_CACHE_HOME') or Path.home()/'.cache')/'phonewave/aston/fingerprint'
VAULT=Path(os.environ.get('PHONEWAVE_VAULT','/mnt/development/denial-os/device-vault'))/'aston'
SRC=Path(__file__).resolve().parents[4]/'services/fingerprint/uff'
SSH=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=5','aston']
STATE.mkdir(parents=True,exist_ok=True)
STAGE=r'''
from pathlib import Path
import hashlib,os,sys
b=Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "f683d9c9"' in b and 'androidboot.slot_suffix = "_b"' in b
assert Path('/dev/fingerprint_dev').exists()
p=Path(sys.argv[1]);data=sys.stdin.buffer.read();assert hashlib.sha256(data).hexdigest()==sys.argv[2]
p.parent.mkdir(mode=0o700,exist_ok=True)
if p.exists():assert p.read_bytes()==data
else:
 with p.open('xb') as f:f.write(data)
p.chmod(int(sys.argv[3],8));print(hashlib.sha256(p.read_bytes()).hexdigest())
'''
def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--inspect',action='store_true');parser.add_argument('--prepare',action='store_true');parser.add_argument('--database',action='store_true');args=parser.parse_args()
 firmware='uff_gx' if args.inspect or args.prepare or args.database else 'uff_spi'
 mode='database' if args.database else 'prepare' if args.prepare else 'inspect' if args.inspect else 'identify'
 m=json.loads((STATE/'build/manifest.json').read_text())
 f=json.loads((STATE/'build/firmware-manifest.json').read_text())
 destinations=[]
 for name,expected,permissions in [('uff-probe',m['output_sha256'],'700'),(firmware+'.elf',f[firmware]['sha256'],'600')]:
  data=(STATE/'build'/name).read_bytes();assert hashlib.sha256(data).hexdigest()==expected
  remote='/var/lib/aston-fingerprint/'+name+'-'+expected[:12]
  p=subprocess.run([*SSH,shlex.join(['python3','-c',STAGE,remote,expected,permissions])],input=data,stdout=subprocess.PIPE,check=True)
  assert p.stdout.decode().strip()==expected;destinations.append(remote)
 command=[destinations[0],'--'+mode,destinations[1]]
 if args.prepare or args.database:
  metadata=json.loads((VAULT/'firmware/calibration-metadata.json').read_text())
  record=next(x for x in metadata['files'] if x['name']=='fingerprint_cali.bin' and x['present'])
  guard='from pathlib import Path;import hashlib,subprocess,sys;p=Path(sys.argv[1]);assert not p.is_symlink();assert hashlib.sha256(p.read_bytes()).hexdigest()==sys.argv[2];raise SystemExit(subprocess.run(sys.argv[3:]).returncode)'
  command=['python3','-c',guard,record['copy'],record['sha256'],*command]
 p=subprocess.run([*SSH,shlex.join(command)],stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
 (STATE/f'{mode}-live-{time.time_ns()}.log').write_bytes(p.stdout)
 (STATE/f'{mode}-live.log').write_bytes(p.stdout)
 print(p.stdout.decode());print('Exit:',p.returncode)
 raise SystemExit(p.returncode)
if __name__=='__main__':main()
