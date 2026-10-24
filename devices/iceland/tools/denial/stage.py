"""Usage: stage.py ID. Stage a Denial payload on denialpad4 (root) from ~logix/.cache/pad4-deploy (reused). Installs a new versioned directory only:
the running session, launcher and plugin selection are untouched."""
from pathlib import Path
import ctypes, hashlib, json, os, subprocess, sys

assert os.geteuid() == 0
assert Path('/etc/pad4-root-id').read_text().strip() == '59b8e7b3:archroot-v1'
assert b'oneplus,iceland\0' in Path('/proc/device-tree/compatible').read_bytes()
ID = sys.argv[1]
S = Path('/home/logix/.cache/pad4-deploy')
D = Path('/opt/denial') / ID
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()

assert sha(S / 'payload.tar.zst') == (S / 'archive.sha256').read_text().split()[0]
names = subprocess.check_output(['tar', '--zstd', '-tf', str(S / 'payload.tar.zst')], text=True).splitlines()
assert all(not Path(n).is_absolute() and '..' not in Path(n).parts for n in names)
assert not D.exists()
D.mkdir(mode=0o755)
subprocess.run(['tar', '--zstd', '--no-same-owner', '--no-same-permissions', '-xf', str(S / 'payload.tar.zst'), '-C', str(D)], check=True)
assert not any(p.is_symlink() for p in D.rglob('*'))
subprocess.run(['sha256sum', '--quiet', '-c', 'payload.sha256'], cwd=D, check=True)
# deniald accepts only a sealed plugin bundle; seal the whole installation.
subprocess.run(['chmod', '-R', 'a-w', str(D)], check=True)

m = json.loads((D / 'deployment.json').read_text())
assert m['id'] == D.name and m['settings_schema'] == json.loads((D / 'build-manifest.json').read_text())['settings_schema']
for name in ['shell', 'settings', 'plugin-bundle']:
    assert sha(D / name / 'lib/libflutter_engine.so') == m['engine_sha256'], name
env = dict(os.environ, LD_LIBRARY_PATH='/opt/pad4-denial-runtime-20261013/usr/lib')
subprocess.run([str(D / 'abi-probe'), str(D)], env=env, check=True)
# The composed shell uses the same engine/ICU; its snapshot must load too.
ctypes.CDLL(str(D / 'plugin-bundle/lib/libapp.so'), mode=os.RTLD_NOW | os.RTLD_LOCAL)
for name in ['native/deniald', 'native/denialctl', 'native/denial-ui', 'settings/denial-settings']:
    r = subprocess.run(['ldd', str(D / name)], env=env, text=True, capture_output=True)
    assert r.returncode == 0 and 'not found' not in r.stdout + r.stderr, (name, r.stdout, r.stderr)
    print('DEPENDENCIES PASS', name)
print('STAGED and ABI-verified', D, '; current session unchanged', flush=True)
