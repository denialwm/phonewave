"""Pack the latest-code candidate and its composed shell into one matched tablet payload. Never connects."""
from pathlib import Path
import hashlib, json, os, shutil, subprocess

HERE = Path(__file__).resolve().parent
VAULT = Path(os.environ.get('PHONEWAVE_VAULT', '/mnt/development/device-vault')) / 'iceland'
OUT = Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache'))) / 'phonewave/iceland/denial/pad4-buffer-return-20261016'
CANDIDATE = OUT / 'candidate'
COMPOSED = OUT / 'composed'
ID = 'pad4-buffer-return-20261016'
payload = OUT / 'payload'
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()

subprocess.run(['python3', str(HERE / 'build-recipe.py'), 'verify', '--candidate', str(CANDIDATE)], check=True)
manifest = json.loads((CANDIDATE / 'manifest.json').read_text())
composed = json.loads((COMPOSED / 'manifest.json').read_text())
assert composed['candidate_manifest_sha256'] == sha(CANDIDATE / 'manifest.json')
for name, digest in composed['payload_sha256'].items():
    assert sha(COMPOSED / name) == digest, name
bundle = json.loads((COMPOSED / 'bundle/denial-plugin-bundle.json').read_text())
assert bundle['engine_sha256'] == manifest['engine_inputs']['libflutter_engine.so']
assert bundle['icu_sha256'] == manifest['engine_inputs']['icudtl.dat']
# Layered and native window composition are built-in defaults: neither binary may still read its switch.
assert b'DENIAL_LAYERED_COMPOSITION' not in (CANDIDATE / 'shell/lib/libflutter_engine.so').read_bytes()
assert b'DENIAL_NATIVE_WINDOW_COMPOSITION' not in (CANDIDATE / 'native/deniald').read_bytes()

assert not payload.exists()
payload.mkdir()
for name in ['native', 'shell', 'settings']:
    shutil.copytree(CANDIDATE / name, payload / name)
shutil.copytree(COMPOSED / 'bundle', payload / 'plugin-bundle')
shutil.copy2(CANDIDATE / 'manifest.json', payload / 'build-manifest.json')
shutil.copy2(CANDIDATE / 'source-manifest.json', payload / 'build-source-manifest.json')
shutil.copy2(COMPOSED / 'manifest.json', payload / 'composition-manifest.json')
shutil.copy2(VAULT / 'denial/abi-probe/pad4-denial-abi-probe', payload / 'abi-probe')
deployment = {
    'id': ID, 'source_root': '/home/logix/denial', 'source_mode': 'local development including uncommitted changes',
    'classification': 'release-mode engine and AOT; DEVELOPMENT build, not release-audited',
    'source_head': json.loads((CANDIDATE / 'source-manifest.json').read_text())['head'],
    'flutter': manifest['flutter'], 'skia': manifest['skia'], 'lock_divergence': manifest['lock_divergence'],
    'settings_schema': manifest['settings_schema'], 'engine_sha256': manifest['engine_inputs']['libflutter_engine.so'],
    'plugin_bundle': 'plugin-bundle', 'plugin_bundle_app_sha256': bundle['app_sha256'],
    'prior_installation': '/opt/denial/pad4-apply-late-20261016',
    'composition': 'layered and native window composition built in; no DENIAL_* composition switches',
}
(payload / 'deployment.json').write_text(json.dumps(deployment, indent=2) + '\n')
files = {str(p.relative_to(payload)): sha(p) for p in sorted(payload.rglob('*')) if p.is_file()}
(payload / 'payload.sha256').write_text(''.join(f'{digest}  {name}\n' for name, digest in sorted(files.items())))
archive = OUT / 'payload.tar.zst'
subprocess.run(['tar', '--zstd', '--owner=0', '--group=0', '-cf', str(archive), '-C', str(payload), '.'], check=True)
(OUT / 'archive.sha256').write_text(f'{sha(archive)}  payload.tar.zst\n')
print(f'Packed {len(files)} files; archive {archive.stat().st_size} bytes; {sha(archive)}')
