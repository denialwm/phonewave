"""Usage: build-composed-shell.py ID

Build the tablet's plugin composition (clock, desktop, launcher, pets, local taskbar; no top bar) from the
latest-code candidate's own source snapshot and engine.

The composition files are the plugin manager's output for this selection (surface-occlusion candidate01). The
composed pubspec must equal the snapshot's pubspec with only the composition edits, so nothing else is borrowed
from the older snapshot. The taskbar comes from the local denial-plugins working tree. Never connects or activates."""
from pathlib import Path
import hashlib, json, re, shutil, subprocess, os, sys

HERE = Path(__file__).resolve().parent
VAULT = Path(os.environ.get('PHONEWAVE_VAULT', '/mnt/development/denial-os/device-vault')) / 'iceland'
ID = sys.argv[1]
OUT = Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache'))) / 'phonewave/iceland/denial' / ID
CANDIDATE = OUT / 'candidate'
TEMPLATE = VAULT / 'denial/composition-template'
TASKBAR = Path('/mnt/development/denial-os/denial-plugins/plugins/denial_taskbar')
FLUTTER = Path('/mnt/exty/denial-flutter-fork-3.44.7')
ARM = 'denial_linux_arm64_release'
HOST = 'denial_host_release'
out = OUT / 'composed'
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
env = dict(os.environ, CI='true', FLUTTER_SUPPRESS_ANALYTICS='true', PUB_ENVIRONMENT='denial:pad4-local')


def run(command, cwd):
    print('+', ' '.join(map(str, command)), flush=True)
    subprocess.run([str(c) for c in command], cwd=cwd, env=env, check=True)


def tree_digest(root):
    # Same walk as deniald's plugin_bundle::hash_tree.
    digest = hashlib.sha256()
    for path in sorted(root.rglob('*')):
        assert not path.is_symlink()
        digest.update(str(path.relative_to(root)).encode() + b'\0')
        digest.update((f'file:{sha(path)}\n' if path.is_file() else 'directory\n').encode())
    return digest.hexdigest()


assert not out.exists()
subprocess.run(['python3', str(HERE / 'build-recipe.py'), 'verify', '--candidate', str(CANDIDATE)], check=True)
manifest = json.loads((CANDIDATE / 'manifest.json').read_text())
engine = CANDIDATE / 'shell/lib/libflutter_engine.so'
icu = CANDIDATE / 'shell/data/icudtl.dat'
assert sha(engine) == manifest['engine_inputs']['libflutter_engine.so'] and sha(icu) == manifest['engine_inputs']['icudtl.dat']

out.mkdir()
source = out / 'source'
# Pub and assemble state belongs to the base shell build; resolve and compile the composition afresh.
shutil.copytree(CANDIDATE / 'source', source, symlinks=True, ignore=shutil.ignore_patterns('.dart_tool'))
shell = source / 'dart_shell'

pubspec = (shell / 'pubspec.yaml').read_text()
top_bar = '  denial_top_bar:\n    path: ../plugins/denial_top_bar\n'
taskbar_git = ('  denial_taskbar:\n    git:\n      url: https://github.com/denialwm/denial-plugins.git\n'
               '      path: plugins/denial_taskbar\n      ref: 4b7adf998852b09b9a91c1cfab48be58debf2e80\n')
taskbar_path = '  denial_taskbar:\n    path: ../plugins/denial_taskbar\n'
assert pubspec.count(top_bar) == 1 and pubspec.count(taskbar_git) == 1, 'checkout pubspec changed; review the composition'
composed = pubspec.replace(top_bar, '').replace(taskbar_git, '').replace('  flutter:\n    sdk: flutter\ndev_dependencies:\n',
                                                                       '  flutter:\n    sdk: flutter\n' + taskbar_path + 'dev_dependencies:\n')
strip_comment = lambda text: text.replace('# Source metadata only; signed Git tags supply release/runtime versions.\n', '')
assert strip_comment(composed) == (TEMPLATE / 'pubspec.yaml').read_text(), 'composition differs from the plugin manager output'
(shell / 'pubspec.yaml').write_text(composed)
for name in ['lib/main.dart', 'lib/composition.g.dart', 'composition-discovery.json']:
    shutil.copy2(TEMPLATE / name, shell / name)
shutil.copytree(TASKBAR, source / 'plugins/denial_taskbar', ignore=shutil.ignore_patterns('.dart_tool', 'build', '.packages'))
(out / 'taskbar.patch').write_bytes(subprocess.check_output(['git', '-C', str(TASKBAR.parent.parent), 'diff', '--binary', 'HEAD', '--', 'plugins/denial_taskbar']))

flutter = FLUTTER / 'bin/flutter'
run([flutter, '--suppress-analytics', 'pub', 'get', '--offline'], shell)
view = CANDIDATE / 'engine-view'
assembly = out / 'assembly'
run([flutter, 'assemble', f'--local-engine-src-path={view}', f'--local-engine={ARM}', f'--local-engine-host={HOST}',
     '--suppress-analytics', '--resource-pool-size=10', f'--output={assembly}',
     '-dTargetFile=lib/main.dart', '-dBuildMode=release', '-dTargetPlatform=linux-arm64',
     '-dDartObfuscation=false', '-dTrackWidgetCreation=true', '-dTreeShakeIcons=false',
     'release_bundle_linux-arm64_assets'], shell)
# The engine view links to the shared ARM output; it must still be the engine the candidate packaged.
assert sha(view / 'out' / ARM / 'libflutter_engine.so') == sha(engine)

bundle = out / 'bundle'
(bundle / 'lib').mkdir(parents=True)
(bundle / 'data').mkdir()
shutil.copy2(assembly / 'lib/libapp.so', bundle / 'lib/libapp.so')
shutil.copy2(engine, bundle / 'lib/libflutter_engine.so')
shutil.copy2(icu, bundle / 'data/icudtl.dat')
shutil.copytree(assembly / 'flutter_assets', bundle / 'data/flutter_assets')
assert 'ARM aarch64' in subprocess.check_output(['file', '-b', str(bundle / 'lib/libapp.so')], text=True)
abi = re.search(r'FLUTTER_ENGINE_ABI: &str = "([^"]+)"', (source / 'compositor/src/lib.rs').read_text()).group(1)
metadata = {
    'schema': 1, 'id': ID + '-composed', 'mode': 'release', 'platform': 'linux-arm64',
    'flutter_generation': abi, 'engine_sha256': sha(engine), 'app_sha256': sha(bundle / 'lib/libapp.so'),
    'icu_sha256': sha(icu), 'assets_sha256': tree_digest(bundle / 'data/flutter_assets'),
    'framework_revision': manifest['flutter']['head'],
    'source_identity': {'manifest_sha256': sha(CANDIDATE / 'source-manifest.json')},
    'pub_lock_sha256': sha(shell / 'pubspec.lock'),
}
(bundle / 'denial-plugin-bundle.json').write_text(json.dumps(metadata, indent=2) + '\n')
record = {'scope': 'composed shell AOT/assets for the candidate engine', 'candidate': str(CANDIDATE),
          'candidate_manifest_sha256': sha(CANDIDATE / 'manifest.json'),
          'composition_template': str(TEMPLATE), 'taskbar_source': str(TASKBAR),
          'taskbar_head': subprocess.check_output(['git', '-C', str(TASKBAR.parent.parent), 'rev-parse', 'HEAD'], text=True).strip(),
          'builder_sha256': sha(__file__), 'bundle': metadata,
          'payload_sha256': {str(p.relative_to(out)): sha(p) for p in sorted(bundle.rglob('*')) if p.is_file()}}
(out / 'manifest.json').write_text(json.dumps(record, indent=2) + '\n')
shutil.copy2(__file__, out / 'build-recipe.py')
print(json.dumps({'output': str(out), 'aot_sha256': metadata['app_sha256'], 'engine_sha256': metadata['engine_sha256']}), flush=True)
