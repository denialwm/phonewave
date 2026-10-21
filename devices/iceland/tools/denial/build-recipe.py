#!/usr/bin/env python3
"""Pad4 AArch64 DEVELOPMENT build with retained Rust and engine caches. Never connects or activates."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import shlex
import subprocess
import sys
import tomllib

os.environ['GIT_OPTIONAL_LOCKS'] = '0'
os.environ['LC_ALL'] = 'C'  # Machine-readable ELF/tool diagnostics, independent of host locale.
TARGET = 'aarch64-unknown-linux-gnu'
ARM_NAME = 'denial_linux_arm64_release'
HOST_NAME = 'denial_host_release'

def run(args, **kwargs):
    print('+', ' '.join(map(str, args)), flush=True)
    return subprocess.run(list(map(str, args)), check=True, **kwargs)

def text(args):
    return subprocess.check_output(list(map(str, args)), text=True).strip()

def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def git_files(root):
    return sorted(set(subprocess.check_output(['git', '-C', str(root), 'ls-files', '-c', '-o', '--exclude-standard', '-z']).decode().rstrip('\0').split('\0')))

def inventory(root):
    return {name: sha(root / name) for name in git_files(root) if (root / name).is_file()}

def state(root):
    return dict(head=text(['git', '-C', root, 'rev-parse', 'HEAD']),
                changes_sha256=hashlib.sha256(subprocess.check_output(['git', '-C', str(root), 'diff', '--binary', 'HEAD']) +
                    json.dumps({n: sha(root / n) for n in text(['git', '-C', root, 'ls-files', '--others', '--exclude-standard']).splitlines() if (root / n).is_file()}, sort_keys=True).encode()).hexdigest(),
                status=text(['git', '-C', root, 'status', '--porcelain=v1']))

def gn_args(path):
    return dict(re.findall(r'^(\w+)\s*=\s*("[^"\n]*"|\w+)$', path.read_text(), re.M))

def verify_args(path, flutter, skia, cpu):
    args = gn_args(path)
    expected = dict(engine_version=json.dumps(flutter['head']), skia_version=json.dumps(skia['head']),
                    host_cpu='"x64"', target_cpu=json.dumps(cpu), target_os='"linux"',
                    dart_target_arch=json.dumps(cpu), flutter_runtime_mode='"release"',
                    slimpeller='true', skia_enable_ganesh='false', skia_enable_graphite='false',
                    shell_enable_vulkan='false', test_enable_vulkan='false', skia_use_vulkan='false')
    for key, value in expected.items():
        if args.get(key) != value:
            raise RuntimeError(f'{path}: {key} expected {value}, got {args.get(key)}')
    return args

def elf(path, machine):
    header = text(['readelf', '-h', path])
    if not re.search(r'Machine:\s*' + machine, header):
        raise RuntimeError(f'Wrong ELF architecture: {path}')
    return dict(sha256=sha(path), file=text(['file', path]),
                notes=text(['readelf', '-n', path]), dynamic=text(['readelf', '-d', path]))

def engine_symbols(source):
    symbols = re.findall(r'b"((?:Denial)?FlutterEngine\w+)\\0"', (source / 'compositor/flutter-engine/src/lib.rs').read_text())
    # Completion callback is an optional additive extension in the current loader.
    return sorted(set(symbols) - {'DenialFlutterEngineRenderOutputsWithCompletion'}) + ['FlutterEngineRunsAOTCompiledDartCode']

def aot_contract(engine_src):
    header = engine_src / 'flutter/third_party/dart/runtime/include/dart_api.h'
    definitions = re.findall(r'^#define kSnapshot(?:Data|Text)AsmSymbol "([^"]+)"$', header.read_text(), re.M)
    if len(definitions) != 2:
        raise RuntimeError('Dart ELF snapshot symbol contract changed; review adapter')
    return dict(symbols=definitions, header_sha256=sha(header), source=str(header),
                loader_sha256=sha(engine_src / 'flutter/third_party/dart/runtime/bin/elf_loader.cc'))

def flutter_ready(flutter, expected_head):
    cache = flutter / 'bin/cache'
    tools = flutter / 'packages/flutter_tools'
    if (flutter / 'bin/internal/bootstrap.sh').exists():
        raise RuntimeError('Unreviewed Flutter bootstrap.sh; prepare caches separately')
    if os.environ.get('FLUTTER_TOOL_ARGS') or os.environ.get('FLUTTER_PREBUILT_ENGINE_VERSION') or os.environ.get('FLUTTER_REALM'):
        raise RuntimeError('Flutter launcher overrides are unsupported; unset them')
    if (cache / 'flutter_tools.stamp').read_text().strip() != expected_head + ':':
        raise RuntimeError('Flutter tool stamp stale; bootstrap canonically as a separate operation')
    if (tools / 'pubspec.yaml').stat().st_mtime_ns > (tools / 'pubspec.lock').stat().st_mtime_ns:
        raise RuntimeError('Flutter tool pubspec newer than lock; prepare tool separately')
    if (cache / 'engine-dart-sdk.stamp').read_text().strip() != (flutter / 'bin/internal/engine.version').read_text().strip():
        raise RuntimeError('Flutter Dart cache is not ready')
    version = json.loads((cache / 'flutter.version.json').read_text())
    if version['frameworkRevision'] != expected_head:
        raise RuntimeError('Flutter version metadata does not describe canonical HEAD')

def canonical_gn_policy(source):
    script = (source / 'tools/denial-flutter-engine').read_text()
    block = re.search(r'GN_ENGINE_ARGS=\((.*?)\n\)', script, re.S).group(1)
    args = shlex.split(block)
    if any('$' in x or '`' in x for x in args):
        raise RuntimeError('Canonical GN policy changed; adapter needs review')
    return args

def tree_hashes(root):
    return {str(path.relative_to(root)): sha(path) for path in sorted(root.rglob('*')) if path.is_file()}

def schema(root):
    dart = re.search(r'schemaVersion = (\d+)', (root / 'packages/denial_flutter_sdk/lib/src/settings/shell_settings.dart').read_text()).group(1)
    rust = re.search(r'SETTINGS_SCHEMA_VERSION: u64 = (\d+)', (root / 'compositor/src/bin/deniald/settings.rs').read_text()).group(1)
    if dart != rust:
        raise RuntimeError('Settings Dart/Rust schema mismatch')
    return int(dart)

def disjoint(path, protected):
    for other in protected:
        if path == other or path.is_relative_to(other) or other.is_relative_to(path):
            raise RuntimeError(f'Unsafe overlapping output {path} and protected path {other}')

def verify(candidate):
    # SHA256SUMS authenticates internal consistency, not a cryptographic signature.
    sums = {}
    for line in (candidate / 'SHA256SUMS').read_text().splitlines():
        digest, name = line.split('  ', 1)
        path = Path(name)
        if path.is_absolute() or '..' in path.parts or name in sums:
            raise RuntimeError('Unsafe or duplicate checksum path')
        sums[name] = digest
        if sha(candidate / name) != digest:
            raise RuntimeError(f'Checksum mismatch: {name}')
    for name in ['manifest.json', 'source-manifest.json', 'source.patch', 'flutter.patch', 'skia.patch', 'build-recipe.py']:
        if name not in sums:
            raise RuntimeError(f'Missing provenance checksum: {name}')
    m = json.loads((candidate / 'manifest.json').read_text())
    if m['mode'] != 'DEVELOPMENT' or m['release_validated'] or m['runtime_validated'] or m['architecture'] != TARGET:
        raise RuntimeError('Incorrect candidate classification')
    sm = json.loads((candidate / 'source-manifest.json').read_text())
    for name, digest in sm['source_sha256'].items():
        path = Path(name)
        if path.is_absolute() or '..' in path.parts or sha(candidate / 'source' / name) != digest:
            raise RuntimeError(f'Source snapshot changed: {name}')
    if schema(candidate / 'source') != m['settings_schema']:
        raise RuntimeError('Packaged schema does not describe snapshot')
    required = {'native/deniald', 'native/denialctl', 'native/denial-ui', 'settings/denial-settings'}
    for app in ['shell', 'settings']:
        required.update({f'{app}/lib/libapp.so', f'{app}/lib/libflutter_engine.so', f'{app}/data/icudtl.dat', f'{app}/data/flutter_assets/AssetManifest.bin'})
    payload = {str(path.relative_to(candidate)): sha(path) for root in ['native', 'shell', 'settings'] for path in (candidate / root).rglob('*') if path.is_file()}
    if not required.issubset(payload) or payload != m['payload_sha256']:
        raise RuntimeError('Missing, changed or undeclared payload')
    if any(sums.get(n) != h for n, h in payload.items()):
        raise RuntimeError('Payload checksum inventory mismatch')
    for name in payload:
        with (candidate / name).open('rb') as stream:
            if stream.read(4) == b'\x7fELF':
                elf(candidate / name, 'AArch64')
    engine_exports = text(['nm', '-D', '--defined-only', candidate / 'shell/lib/libflutter_engine.so'])
    for symbol in engine_symbols(candidate / 'source'):
        if not re.search(r'\b' + symbol + r'$', engine_exports, re.M):
            raise RuntimeError(f'Missing coupled engine symbol {symbol}')
    if sha(candidate / 'build-recipe.py') != m['build_recipe_sha256']:
        raise RuntimeError('Incorrect builder provenance')
    if sha(candidate / 'source/prebuilt/flutter-engine/SOURCE_LOCK.json') != m['source_lock_sha256']:
        raise RuntimeError('Incorrect source-lock provenance')
    if set(m['aot_contract']['symbols']) != {'_kDartSnapshotData', '_kDartSnapshotText'}:
        raise RuntimeError('Unsupported or incomplete Dart AOT loader contract')
    for app in ['shell', 'settings']:
        exports = text(['nm', '-D', '--defined-only', candidate / app / 'lib/libapp.so'])
        for symbol in m['aot_contract']['symbols']:
            if not re.search(r'\b' + symbol + r'$', exports, re.M):
                raise RuntimeError(f'Missing AOT symbol {symbol}')
        if sha(candidate / app / 'lib/libflutter_engine.so') != m['engine_inputs']['libflutter_engine.so']:
            raise RuntimeError('Mixed engine')
        if sha(candidate / app / 'data/icudtl.dat') != m['engine_inputs']['icudtl.dat']:
            raise RuntimeError('Mixed ICU data')
        asset_names = {str(x.relative_to(candidate / app / 'data/flutter_assets')) for x in (candidate / app / 'data/flutter_assets').rglob('*') if x.is_file()}
        if not set(m['required_sdk_assets']).issubset(asset_names):
            raise RuntimeError('Missing SDK assets')
    print(f'Verified local DEVELOPMENT payload: {candidate}; no runtime/device validation')

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=['check', 'build', 'verify'], nargs='?', default='check')
    p.add_argument('--denial', type=Path, default=Path('/mnt/development/denial-os/denial'))
    p.add_argument('--flutter', type=Path, default=Path(os.environ.get('DENIAL_FLUTTER_SOURCE_ROOT', '/mnt/exty/denial-flutter-fork-3.44.7')))
    p.add_argument('--skia', type=Path, default=Path(os.environ.get('DENIAL_SKIA_SOURCE_ROOT', '/mnt/exty/denial-skia-fork-3.44.7')))
    p.add_argument('--sysroot', type=Path, default=Path('/mnt/development/denial-os/cross/sysroot'))
    p.add_argument('--engine-build', type=Path, default=Path.home() / '.cache/denial/flutter-engine/build-arm64')
    p.add_argument('--candidate', type=Path, help='New, nonexisting directory (required for build); existing candidate for verify')
    p.add_argument('--rust-cache', type=Path, default=Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache'))) / 'phonewave/iceland/denial/rust')
    p.add_argument('--jobs', type=int, default=min(6, max(1, len(os.sched_getaffinity(0)) - 4)))
    a = p.parse_args()
    if a.action == 'verify':
        if not a.candidate:
            p.error('--candidate required')
        verify(a.candidate.resolve())
        return
    if len(os.sched_getaffinity(0)) <= 4 or not 1 <= a.jobs <= len(os.sched_getaffinity(0)) - 4:
        p.error('--jobs must leave four CPUs free')
    for cmd in ['git', 'rustup', 'cargo', 'aarch64-linux-gnu-gcc', 'aarch64-linux-gnu-ar', 'pkg-config', 'readelf', 'nm', 'file', 'ninja']:
        if not shutil.which(cmd):
            raise RuntimeError(f'Missing command: {cmd}')
    for field in ['denial', 'flutter', 'skia', 'sysroot', 'engine_build']:
        setattr(a, field, getattr(a, field).resolve())
    toolchain = tomllib.loads((a.denial / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    if TARGET not in text(['rustup', 'target', 'list', '--installed', '--toolchain', toolchain]).splitlines():
        raise RuntimeError(f'Install Rust target for checkout toolchain {toolchain}: {TARGET}')
    if bool(os.environ.get('DENIAL_FLUTTER_SOURCE_ROOT')) != bool(os.environ.get('DENIAL_SKIA_SOURCE_ROOT')):
        p.error('Set both source-root environment overrides together')
    fstate, sstate = state(a.flutter), state(a.skia)
    lock = json.loads((a.denial / 'prebuilt/flutter-engine/SOURCE_LOCK.json').read_text())
    # Local trees are authoritative, never reset to the committed source lock.
    print(json.dumps(dict(mode='DEVELOPMENT', rust=toolchain, flutter=fstate, skia=sstate,
                         lock_divergence=bool(fstate['head'] != lock['flutter']['revision'] or sstate['head'] != lock['skia']['revision'] or fstate['status'] or sstate['status']), jobs=a.jobs), indent=2))
    engine_src = a.flutter / 'engine/src'
    if (engine_src / 'flutter/third_party/skia').resolve() != a.skia:
        raise RuntimeError('Canonical Flutter/Skia source pair not linked')
    host = (engine_src / 'out' / HOST_NAME).resolve()
    verify_args(host / 'args.gn', fstate, sstate, 'x64')
    env = dict(os.environ, CARGO_NET_OFFLINE='true', PKG_CONFIG_ALLOW_CROSS='1',
        PKG_CONFIG_SYSROOT_DIR=str(a.sysroot), PKG_CONFIG_LIBDIR=f'{a.sysroot}/usr/lib/pkgconfig:{a.sysroot}/usr/share/pkgconfig',
        CC_aarch64_unknown_linux_gnu='aarch64-linux-gnu-gcc', AR_aarch64_unknown_linux_gnu='aarch64-linux-gnu-ar',
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER='aarch64-linux-gnu-gcc',
        CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS=f'-C link-arg=-Wl,--allow-shlib-undefined -C link-arg=-Wl,-rpath-link,{a.sysroot}/usr/lib')
    for package in ['egl', 'gbm', 'libinput', 'libseat', 'libudev', 'wayland-server', 'wayland-client', 'wayland-egl', 'xkbcommon']:
        run(['pkg-config', '--exists', package], env=env)
    dart_schema = schema(a.denial)
    print('Settings schema:', dart_schema)
    protected = [a.denial, a.flutter, a.skia, host, a.sysroot]
    disjoint(a.engine_build, protected)
    if a.candidate:
        a.candidate = a.candidate.resolve()
        disjoint(a.candidate, protected + [a.engine_build])
    # Already bootstrapped tool caches are required; never call bootstrap/download.
    flutter_cache = a.flutter / 'bin/cache'
    for required in [flutter_cache / 'dart-sdk/bin/dart', flutter_cache / 'flutter_tools.snapshot',
                     flutter_cache / 'flutter.version.json', engine_src / 'flutter/third_party/gn/gn']:
        if not required.is_file():
            raise RuntimeError(f'Missing bootstrapped tool input {required}; prepare canonically first')
    flutter_ready(a.flutter, fstate['head'])
    if a.action == 'check':
        print('Prerequisites OK. build regenerates ARM GN and rebuilds engine/compiler; stale ARM cache is NOT selected. No writes/device access.')
        return
    if not a.candidate:
        p.error('--candidate required for build')
    a.candidate.mkdir(parents=True, exist_ok=False)
    c = a.candidate
    source = c / 'source'
    source.mkdir()
    shutil.copy2(Path(__file__), c / 'build-recipe.py')
    inputs = inventory(a.denial)
    for name in inputs:
        dst = source / name
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(a.denial / name, dst)
    dart_schema = schema(source)
    for root, name in [(a.flutter, 'flutter'), (a.skia, 'skia')]:
        (c / (name + '.patch')).write_bytes(subprocess.check_output(['git', '-C', str(root), 'diff', '--binary', 'HEAD']))
    (c / 'source.patch').write_bytes(subprocess.check_output(['git', '-C', str(a.denial), 'diff', '--binary', 'HEAD']))
    (c / 'source-manifest.json').write_text(json.dumps(dict(head=text(['git', '-C', a.denial, 'rev-parse', 'HEAD']), source_sha256=inputs, flutter=fstate, skia=sstate), indent=2) + '\n')
    def unchanged():
        if inventory(a.denial) != inputs or any(sha(source / n) != h for n, h in inputs.items()):
            raise RuntimeError('Denial source changed during build; do not package')
        if state(a.flutter) != fstate or state(a.skia) != sstate:
            raise RuntimeError('Fork source changed during build; do not package')
    # Shared lock matches canonical Denial engine tooling; no live engine writes.
    cache = Path(os.environ.get('DENIAL_FLUTTER_ENGINE_CACHE_ROOT', str(Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache'))) / 'denial/flutter-engine'))).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    with (cache / 'build.lock').open('a') as build_lock:
        fcntl.flock(build_lock, fcntl.LOCK_EX)
        depot = engine_src / 'flutter/third_party/depot_tools'
        engine_env = dict(os.environ, PATH=f'{depot}:{os.environ["PATH"]}', DEPOT_TOOLS_UPDATE='0', VPYTHON_BYPASS='manually managed python not supported by chrome operations')
        unchanged()
        # Reuse canonical GN generator and release policy, explicitly cross-targeting ARM64.
        gn_policy = canonical_gn_policy(source)
        run([engine_src / 'flutter/tools/gn', '--linux', '--linux-cpu=arm64', '--runtime-mode=release', *gn_policy,
             f'--out-dir={a.engine_build}', f'--target-dir={ARM_NAME}'], cwd=engine_src, env=engine_env)
        arm = a.engine_build / 'out' / ARM_NAME
        arm_args = verify_args(arm / 'args.gn', fstate, sstate, 'arm64')
        run(['/usr/bin/ninja', '-C', arm, '-j', a.jobs, 'libflutter_engine.so', 'flutter_patched_sdk', 'clang_x64/gen_snapshot'], env=engine_env)
        run(['/usr/bin/ninja', '-C', host, '-j', a.jobs, 'dart_sdk', 'flutter_patched_sdk', 'gen_snapshot', 'flutter/tools/const_finder:const_finder', 'flutter/tools/font_subset:font_subset', 'flutter/sky/packages/sky_engine:sky_engine', 'libflutter_linux_gtk.so', 'flutter/shell/platform/linux:publish_headers_linux'], env=engine_env)
        unchanged()
        host_args = verify_args(host / 'args.gn', fstate, sstate, 'x64')
        if arm_args['dart_version'] != host_args['dart_version']:
            raise RuntimeError('Host/ARM Dart revisions mismatch')
        elf(arm / 'libflutter_engine.so', 'AArch64')
        elf(arm / 'clang_x64/gen_snapshot', 'Advanced Micro Devices X86-64')
        cross_snapshot_version = subprocess.check_output([str(arm / 'clang_x64/gen_snapshot'), '--version'], stderr=subprocess.STDOUT, text=True).strip()
        host_dart_version = subprocess.check_output([str(host / 'dart-sdk/bin/dart'), '--version'], stderr=subprocess.STDOUT, text=True).strip()
        if re.search(r'Dart SDK version: (\S+)', cross_snapshot_version).group(1) != re.search(r'Dart SDK version: (\S+)', host_dart_version).group(1) or 'linux_simarm64' not in cross_snapshot_version:
            raise RuntimeError('ARM cross snapshot compiler and host frontend SDK do not match')
        exports = text(['nm', '-D', '--defined-only', arm / 'libflutter_engine.so'])
        for symbol in engine_symbols(source):
            if not re.search(r'\b' + symbol + r'$', exports, re.M):
                raise RuntimeError(f'Missing engine ABI symbol: {symbol}')
        # Cross gen_snapshot must execute on the host, but generate ARM AOT.
        view = c / 'engine-view'
        (view / 'out' / ARM_NAME).mkdir(parents=True)
        (view / 'flutter').symlink_to(engine_src / 'flutter')
        (view / 'out' / HOST_NAME).symlink_to(host)
        for entry in arm.iterdir():
            (view / 'out' / ARM_NAME / entry.name).symlink_to(arm / 'clang_x64/gen_snapshot' if entry.name == 'gen_snapshot' else entry)
        flutter_ready(a.flutter, fstate['head'])
        flutter = a.flutter / 'bin/flutter'
        flutter_env = dict(os.environ, CI='true', FLUTTER_SUPPRESS_ANALYTICS='true', PUB_ENVIRONMENT='denial:moto-local', FLUTTER_GIT_URL=lock['flutter']['repository'])
        run([flutter, '--suppress-analytics', 'pub', 'get', '--offline', '--enforce-lockfile'], cwd=source / 'packages/denial_flutter_sdk', env=flutter_env)
        run([flutter, '--suppress-analytics', 'gen-l10n'], cwd=source / 'packages/denial_flutter_sdk', env=flutter_env)
        # Regenerated tracked localization must match the snapshot; stale ARBs are rejected.
        unchanged()
        for directory, bundle in [('dart_shell', 'shell'), ('settings_app', 'settings')]:
            run([flutter, '--suppress-analytics', 'pub', 'get', '--offline', '--enforce-lockfile'], cwd=source / directory, env=flutter_env)
            output = c / (bundle + '-assembly')
            run([flutter, 'assemble', f'--local-engine-src-path={view}', f'--local-engine={ARM_NAME}', f'--local-engine-host={HOST_NAME}',
                 '--suppress-analytics', f'--resource-pool-size={a.jobs}', f'--output={output}', '-dTargetFile=lib/main.dart', '-dBuildMode=release',
                 '-dTargetPlatform=linux-arm64', '-dDartObfuscation=false', '-dTrackWidgetCreation=true', '-dTreeShakeIcons=false',
                 'release_bundle_linux-arm64_assets'], cwd=source / directory, env=flutter_env)
            base = c / bundle
            (base / 'lib').mkdir(parents=True)
            (base / 'data').mkdir()
            shutil.copytree(output / 'flutter_assets', base / 'data/flutter_assets')
            shutil.copy2(output / 'lib/libapp.so', base / 'lib/libapp.so')
            shutil.copy2(arm / 'libflutter_engine.so', base / 'lib/libflutter_engine.so')
            shutil.copy2(arm / 'icudtl.dat', base / 'data/icudtl.dat')
        # Capture provenance while the canonical shared output lock is still held.
        snapshot_contract = aot_contract(engine_src)
        engine_inputs = {n: sha(arm / n) for n in ['libflutter_engine.so', 'icudtl.dat', 'args.gn', 'clang_x64/gen_snapshot', 'flutter_patched_sdk/platform_strong.dill']}
        host_inputs = {n: sha(host / n) for n in ['args.gn', 'gen/const_finder.dart.snapshot', 'font-subset', 'dart-sdk/bin/dart', 'gen_snapshot', 'flutter_patched_sdk/platform_strong.dill', 'dart-sdk/bin/dartaotruntime', 'dart-sdk/bin/snapshots/frontend_server_aot.dart.snapshot']}
        host_trees = {n: tree_hashes(host / n) for n in ['gen/dart-pkg/sky_engine']}
        if not all(host_trees.values()):
            raise RuntimeError('Missing host SDK tree inputs')
        flutter_tool_inputs = {n: sha(flutter_cache / n) for n in ['flutter_tools.snapshot', 'flutter.version.json', 'dart-sdk/bin/dart', 'dart-sdk/bin/snapshots/frontend_server_aot.dart.snapshot']}
        for app in ['shell', 'settings']:
            if sha(c / app / 'lib/libflutter_engine.so') != engine_inputs['libflutter_engine.so']:
                raise RuntimeError('Packaged engine differs from compiler session')
        native_sysroot_inputs = tree_hashes(a.sysroot)
        clang = engine_src / 'flutter/buildtools/linux-x64/clang/bin/clang'
        clang_identity = dict(path=str(clang), sha256=sha(clang), version=text([clang, '--version']))
        unchanged()
    rust_cache = a.rust_cache.resolve()
    disjoint(rust_cache, protected + [source, a.engine_build])
    rust_cache.mkdir(parents=True, exist_ok=True)
    env['CARGO_TARGET_DIR'] = str(rust_cache)
    print('Reusing Rust dependency cache:', rust_cache, flush=True)
    env['CARGO_BUILD_JOBS'] = str(a.jobs)
    common = ['cargo', '+' + toolchain, 'build', '--locked', '--offline', '--release', '--target', TARGET, '--jobs', str(a.jobs)]
    run(common + ['--manifest-path', source / 'compositor/Cargo.toml', '--features', 'flutter,ui-development', '--bin', 'deniald', '--bin', 'denialctl', '--bin', 'denial-ui'], env=env)
    run(common + ['--manifest-path', source / 'native_app/Cargo.toml', '--bin', 'denial-app'], env=env)
    (c / 'native').mkdir()
    for name in ['deniald', 'denialctl', 'denial-ui']:
        shutil.copy2(rust_cache / TARGET / 'release' / name, c / 'native' / name)
    shutil.copy2(rust_cache / TARGET / 'release/denial-app', c / 'settings/denial-settings')
    unchanged()
    payload = {str(path.relative_to(c)): sha(path) for root in ['native', 'shell', 'settings'] for path in sorted((c / root).rglob('*')) if path.is_file()}
    elfs = {name: elf(c / name, 'AArch64') for name in payload if (c / name).read_bytes()[:4] == b'\x7fELF'}
    sdk_assets = source / 'packages/denial_flutter_sdk/assets'
    required_assets = ['packages/denial_flutter_sdk/assets/' + str(path.relative_to(sdk_assets)) for path in sorted(sdk_assets.rglob('*')) if path.is_file() and path.suffix in ['.png', '.svg', '.ttf']]
    manifest = dict(mode='DEVELOPMENT', release_validated=False, runtime_validated=False, architecture=TARGET,
        source_lock=lock, source_lock_sha256=sha(source / 'prebuilt/flutter-engine/SOURCE_LOCK.json'), flutter=fstate, skia=sstate,
        lock_divergence=bool(fstate['head'] != lock['flutter']['revision'] or sstate['head'] != lock['skia']['revision'] or fstate['status'] or sstate['status']),
        rust_cache=str(rust_cache), rust_toolchain=toolchain, rustc=text(['rustc', '+' + toolchain, '-Vv']), settings_schema=int(dart_schema),
        aot_contract=snapshot_contract, arm_args=arm_args, host_args=host_args, cross_snapshot_version=cross_snapshot_version, host_dart_version=host_dart_version,
        engine_inputs=engine_inputs, host_inputs=host_inputs, host_trees=host_trees, flutter_tool_inputs=flutter_tool_inputs,
        shared_lock=str(cache / 'build.lock'), gn_policy=gn_policy, clang=clang_identity,
        build_recipe_sha256=sha(c / 'build-recipe.py'), compiler_references_mutable=True,
        compiler_paths=dict(host=str(host), arm=str(arm), flutter_cache=str(flutter_cache)),
        embedder_header_sha256=sha(engine_src / 'flutter/shell/platform/embedder/embedder.h'),
        native_sysroot_inputs=native_sysroot_inputs,
        native_compilers={cmd: dict(path=shutil.which(cmd), version=text([cmd, '--version'])) for cmd in ['aarch64-linux-gnu-gcc', 'aarch64-linux-gnu-ar']},
        sysroot_pkgconfig=tree_hashes(a.sysroot / 'usr/lib/pkgconfig'),
        required_sdk_assets=required_assets,
        payload_sha256=payload, elf=elfs, abi_load='NOT RUN: AArch64 cannot be loaded natively on x86_64; device operator must validate before activation')
    (c / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (c / 'SHA256SUMS').write_text(''.join(f'{digest}  {name}\n' for name, digest in sorted(payload.items())) + f'{sha(c / "manifest.json")}  manifest.json\n' + f'{sha(c / "source-manifest.json")}  source-manifest.json\n' + ''.join(f'{sha(c / n)}  {n}\n' for n in ['source.patch', 'flutter.patch', 'skia.patch', 'build-recipe.py']))
    verify(c)

if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, OSError, ValueError) as error:
        print(f'moto70-build-denial: {error}', file=sys.stderr)
        sys.exit(1)
