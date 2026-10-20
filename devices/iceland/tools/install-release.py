"""Install/audit the staged rootfs and register images; no partition writes."""
from pathlib import Path
import hashlib
import json
import os
import shutil
import subprocess
import tarfile

# Staged by tools/deploy.py next to the release manifest it installs.
ROOT = Path(__file__).resolve().parent
NEW, OLD = (json.loads((ROOT / 'manifest.json').read_text())[k] for k in ['release', 'previous_release'])
assert ROOT == Path('/var/lib/pad4-release') / NEW
REG = Path('/usr/share/pad4-flash/images.json')

def sha(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

assert os.geteuid() == 0
assert Path('/etc/pad4-root-id').read_text().strip() == '59b8e7b3:archroot-v1'
bc = Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "59b8e7b3"' in bc and 'androidboot.slot_suffix = "_b"' in bc
assert os.uname().release == OLD
manifest = json.loads((ROOT / 'manifest.json').read_text())
before = json.loads((ROOT / 'preflash-audit.json').read_text())
assert manifest['release'] == NEW and manifest['previous_release'] == OLD
assert sha('/sys/kernel/notes') == before['kernel_notes_sha256']
assert sha('/usr/local/sbin/pad4-flash') == before['updater_sha256']
for name in ['boot', 'init_boot']:
    assert sha('/dev/disk/by-partlabel/' + name + '_b') == before['partitions'][name + '_b']
    assert before['partitions'][name + '_b'] == manifest['images'][name]['baseline_sha256']
    assert sha(ROOT / 'artifacts' / (name + '.img')) == manifest['images'][name]['sha256']
registry = json.loads(REG.read_text())
boot_old = before['partitions']['boot_b']
init_old = before['partitions']['init_boot_b']
current = registry['images'][boot_old]
assert current['kernel_release'] == OLD
assert sha(current['modules_manifest']) == current['modules_manifest_sha256']
for line in Path(current['modules_manifest']).read_text().splitlines():
    digest, name = line.split(None, 1)
    assert sha('/' + name.strip()) == digest, name
assert not (Path('/usr/lib/modules') / NEW).exists()
assert not (Path('/usr/share/pad4-kernel') / NEW).exists()
assert sha(ROOT / 'artifacts/rootfs.tar.gz') == manifest['rootfs_sha256']
helpers = ['usr/local/libexec/pad4-kernel-release', 'usr/local/libexec/pad4-usb-setup']
rollback = ROOT / 'rootfs-backup'
rollback.mkdir(mode=0o700)
for name in helpers + ['usr/share/pad4-flash/images.json']:
    target = rollback / name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2('/' + name, target)
with tarfile.open(ROOT / 'artifacts/rootfs.tar.gz') as archive:
    for member in archive.getmembers():
        assert not member.issym() and not member.islnk()
        assert member.uid == 0 and member.gid == 0, member.name
        assert member.name.startswith('usr/lib/modules/' + NEW) or member.name in manifest['rootfs_helpers'], member.name
    archive.extractall('/', filter='data')
for name, digest in manifest['rootfs_helpers'].items():
    assert sha('/' + name) == digest, name
module_manifest = Path('/usr/share/pad4-kernel') / NEW / 'modules-files.sha256'
assert sha(module_manifest) == manifest['modules_manifest_sha256']
expected = set()
for line in module_manifest.read_text().splitlines():
    digest, name = line.split(None, 1)
    name = name.strip()
    assert sha('/' + name) == digest, name
    expected.add(name)
assert expected == {str(p).lstrip('/') for p in (Path('/usr/lib/modules') / NEW).rglob('*') if p.is_file()}
boot_new = manifest['images']['boot']['sha256']
init_new = manifest['images']['init_boot']['sha256']
record = dict(current, release=NEW, kernel_release=NEW,
              kernel_sha256=manifest['kernel_sha256'], kernel_banner=manifest['kernel_banner'],
              modules_manifest=str(module_manifest), modules_manifest_sha256=sha(module_manifest),
              companion_sha256=[init_new])
registry['images'][boot_new] = record
# The transitional init supports both kernels. Keep the old module closure
# registered and installed so either boot image can recover.
init_record = dict(registry['images'][init_old], release=NEW + '-dual-init',
                   kernel_release=OLD, modules_manifest=current['modules_manifest'],
                   modules_manifest_sha256=current['modules_manifest_sha256'],
                   companion_sha256=[boot_old, boot_new])
registry['images'][init_new] = init_record
registry['images'][boot_old]['companion_sha256'].append(init_new)
tmp = REG.with_name('images.devices.tmp')
with tmp.open('x') as f:
    json.dump(registry, f, indent=2)
    f.write('\n')
    f.flush()
    os.fsync(f.fileno())
tmp.chmod(0o644)
os.replace(tmp, REG)
subprocess.run(['/usr/local/sbin/pad4-flash', 'init_boot', str(ROOT / 'artifacts/init_boot.img'), '--check'], check=True)
print('PASS: complete rootfs installed and audited; images registered. No partition written.', flush=True)
