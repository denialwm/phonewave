"""Read-only boot-chain and module snapshot; writes no tablet files."""
from pathlib import Path
import hashlib
import json
import os
import subprocess

def sha(path):
    with open(path, 'rb', buffering=0) as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

bc = Path('/proc/bootconfig').read_text()
assert 'androidboot.serialno = "59b8e7b3"' in bc
assert 'androidboot.slot_suffix = "_b"' in bc
assert Path('/etc/pad4-root-id').read_text().strip() == '59b8e7b3:archroot-v1'
baseline = json.loads(Path('/var/lib/pad4-release-backups/charging-chg1-baseline/manifest.json').read_text())
plan = json.loads(Path('/var/lib/pad4-release/7.2.0-sm8850-pad4dp1/boot-success-plan/plan.json').read_text())
registry = json.loads(Path('/usr/share/pad4-flash/images.json').read_text())
banner = Path('/proc/version').read_text().strip().removeprefix('Linux version ')
result = {'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
          'release': os.uname().release, 'banner': banner, 'slot': 'b', 'partitions': {}, 'gpt': {},
          'kernel_notes_sha256': sha('/sys/kernel/notes'), 'registry': registry,
          'updater_sha256': sha('/usr/local/sbin/pad4-flash')}
for name in sorted(plan['partitions']):
    path = Path('/dev/disk/by-partlabel')/name
    result['partitions'][name] = sha(path)
    if name.startswith(('abl_', 'xbl_')):
        assert result['partitions'][name] == baseline['partitions'][name]['sha256'], name
boot = Path('/dev/disk/by-partlabel/boot_b').resolve()
disk = (Path('/sys/class/block')/boot.name).resolve().parent.name
sysdisk = Path('/sys/class/block')/disk
assert '1d84000.ufs' in str(sysdisk.resolve())
assert int((sysdisk/'size').read_text())*512 == plan['disk_bytes']
assert int((sysdisk/'queue/logical_block_size').read_text()) == plan['logical_sector_bytes']
with open('/dev/'+disk,'rb') as f:
    for region in plan['regions']:
        f.seek(region['offset'])
        data = f.read(region['bytes'])
        assert len(data) == region['bytes']
        result['gpt'][region['name']] = hashlib.sha256(data).hexdigest()
        assert result['gpt'][region['name']] == baseline['gpt'][region['name']], region['name']
    f.seek(0)
    result['mbr_sha256'] = hashlib.sha256(f.read(512)).hexdigest()
# The running kernel may differ from boot_b between a write and its reboot.
# Verify the module closure of the kernel actually running, found by banner.
records = [r for r in registry['images'].values()
           if r.get('partition') == 'boot' and r.get('kernel_banner') == banner]
assert records and len({r['modules_manifest_sha256'] for r in records}) == 1, banner
record = records[0]
assert record['kernel_release'] == os.uname().release
manifest = Path(record['modules_manifest'])
assert sha(manifest) == record['modules_manifest_sha256']
expected = set()
for line in manifest.read_text().splitlines():
    digest, name = line.split(maxsplit=1)
    assert name.startswith('usr/lib/modules/'+os.uname().release+'/')
    assert sha('/'+name) == digest, name
    expected.add(name)
actual = {str(p.relative_to('/')) for p in (Path('/usr/lib/modules')/os.uname().release).rglob('*') if p.is_file()}
assert actual == expected
result['modules_manifest_sha256'] = sha(manifest)
result['module_files_verified'] = len(expected)
result['denial'] = subprocess.run(['systemctl','is-active','pad4-denial.service'],capture_output=True,text=True).stdout.strip()
print(json.dumps(result,indent=2),flush=True)
