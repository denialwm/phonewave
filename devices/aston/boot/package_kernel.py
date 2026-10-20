#!/usr/bin/python3
"""Package boot only from the verified previous Ace3 image, without device writes."""
import argparse,hashlib,json,os,subprocess
from pathlib import Path
B=Path(os.environ.get('PHONEWAVE_VAULT','/mnt/development/device-vault'))/'aston/boot'
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--generation',type=int,required=True);p.add_argument('--previous',type=int,required=True);args=p.parse_args()
n=args.generation;previous=args.previous;assert 30<=previous<n<=99
old=json.loads((B/f'kernel-flash-evidence-devices{previous}.json').read_text())
template=B/f'boot_b-devices{previous}.img';assert hashlib.sha256(template.read_bytes()).hexdigest()==old['readback_sha256']
image=B/f'boot_b-devices{n}.img';evidence=B/f'boot-evidence-devices{n}';evidence.mkdir(exist_ok=True)
assert not image.exists()
source=Path(f'/mnt/development/linux-perpetua-build/releases/devices-{n}-aston/artifacts/Image')
result=subprocess.run(['/mnt/development/linux-perpetua/devices/boot/android-gki','boot',str(template),str(source),str(image),str(evidence)],check=True,capture_output=True)
m=json.loads(result.stdout);(B/f'boot-package-devices{n}.json').write_bytes(result.stdout)
print(json.dumps(m))
