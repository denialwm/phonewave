#!/usr/bin/env python3
"""Package the shared devices kernel as an audited Pad 4 release, offline.

Usage: boot/package_pad4_devices.py N PREV
       (the iceland release linux-perpetua-build/releases/devices-N-iceland;
       PREV is the devices release the tablet runs)

Never accesses the tablet. BASE, the vault's boot/devicesPREV, is the release
the tablet runs; its images are the rollback images and the template of the
new ones, which go to the vault's boot/devicesN. The new init_boot supports
both kernels, so it can be installed and proven on the running kernel before
the boot image changes.
"""
from pathlib import Path
import difflib, gzip, hashlib, io, json, os, shutil, stat, subprocess, sys, tarfile
sys.dont_write_bytecode = True  # nothing is written next to the sources
import rescue_helpers as b

HERE = Path(__file__).resolve().parent
VAULT = Path(os.environ.get('PHONEWAVE_VAULT', '/mnt/development/denial-os/device-vault')) / 'iceland'
RELEASES = Path('/mnt/development/denial-os/linux-perpetua-build/releases')


def devices(n):
    """Release and build output of the iceland devices build N."""
    out = RELEASES / f'devices-{n}-iceland'
    return json.loads((out / 'manifest.json').read_text())['release'], out


if len(sys.argv) != 3:
    sys.exit(__doc__)
N, PREV = sys.argv[1:]
NEW, KBUILD = devices(N)
OLD, OLD_KBUILD = devices(PREV)
BASE = VAULT / f'boot/devices{PREV}'
OLD_MODULES = OLD_KBUILD / 'modules-root/lib/modules' / OLD
OBJ = Path('/mnt/exty/linux-perpetua/build/iceland')
# The boot chain's image writer in the fixed kernel source (devices/README).
ANDROID_GKI = Path('/mnt/development/denial-os/linux-perpetua/devices/boot/android-gki')
OUT = VAULT / f'boot/devices{N}'
ART = OUT / 'artifacts'
GCC = ['aarch64-linux-gnu-gcc', '-static', '-Os', '-s', '-Wall', '-Wextra', '-Werror']
HELPERS = ['usr/local/libexec/pad4-kernel-release', 'usr/local/libexec/pad4-usb-setup']


def put(path, data, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data.encode() if isinstance(data, str) else data)
    path.chmod(mode)


def sums(root):
    return ''.join(f'{b.sha(f)}  {f.relative_to(root)}\n'
                   for f in sorted(root.rglob('*')) if f.is_file())


def root_owned(info):
    info.uid = info.gid = 0
    info.uname = info.gname = 'root'
    return info


def update_sources(base, snapshot):
    """Devices base: reproduce its binaries; the release pair changes, and the
    rescue init only as source-change.diff records."""
    src = BASE / 'source'
    for f in src.iterdir():
        assert b.sha(f) == base['source_sha256'][f.name], f.name
    for key, binary in [('init_compile_command', BASE / 'mainline-init'),
                        ('usb_compile_command', BASE / 'pad4-usb-setup')]:
        command = base[key]
        assert command[-2] == '-o' and Path(command[-1]).name == binary.name
        # The command names the base release's directory as it was when
        # packaged; its sources are now in BASE.
        packaged = Path(command[-1]).parent
        command = [str(BASE / Path(a).relative_to(packaged)) if a.startswith(f'{packaged}/') else a
                   for a in command]
        b.run(*command[:-1], OUT / ('base-' + binary.name))
        assert b.sha(OUT / ('base-' + binary.name)) == b.sha(binary)
    assert b.sha(BASE / 'pad4-usb-setup') == base['rootfs_helpers'][HELPERS[1]]
    for name in ['usb_rescue_init.c', 'usb_rescue_systemd.c', 'pad4_kernel_pair.h']:
        assert b.sha(HERE / name) == b.sha(src / name), name
        shutil.copy2(src / name, snapshot / name)
    shutil.copy2(HERE / 'usb_rescue_persistent.c', snapshot / 'usb_rescue_persistent.c')
    change = ''.join(difflib.unified_diff(
        (src / 'usb_rescue_persistent.c').read_text().splitlines(keepends=True),
        (snapshot / 'usb_rescue_persistent.c').read_text().splitlines(keepends=True),
        'base/usb_rescue_persistent.c', 'new/usb_rescue_persistent.c'))
    if change:
        put(OUT / 'source-change.diff', change)
    usb = (src / 'pad4_usb_setup.c').read_text()
    assert usb.count(f'"{base["previous_release"]}"') == 1
    put(snapshot / 'pad4_usb_setup.c', usb.replace(f'"{base["previous_release"]}"', f'"{OLD}"'))


def pack(name, template, payload):
    """Writes ART/NAME.img from the base release's image of the partition."""
    result = json.loads(b.run(ANDROID_GKI, name, template, payload, ART / f'{name}.img', OUT))
    return {'bytes': result['bytes'], 'sha256': result['sha256'],
            'baseline_sha256': result['template_sha256']}


def main():
    OUT.mkdir()
    base = json.loads((BASE / 'manifest.json').read_text())
    assert base['release'] == OLD
    for name in ['boot', 'init_boot']:
        assert b.sha(BASE / 'artifacts' / f'{name}.img') == base['images'][name]['sha256']

    # Prove the toolchain still reproduces the installed rescue binaries
    # from the base release's saved sources before building new ones.
    snapshot = OUT / 'source'
    snapshot.mkdir()
    update_sources(base, snapshot)

    init_command = [*GCC, f'-DRESCUE_MAINLINE_RELEASE="{OLD}"',
                    f'-DRESCUE_MAINLINE_RELEASE_ALT="{NEW}"',
                    snapshot / 'usb_rescue_systemd.c', '-o', OUT / 'mainline-init']
    b.run(*init_command)
    b.static_arm64(OUT / 'mainline-init')
    data = (OUT / 'mainline-init').read_bytes()
    assert OLD.encode() in data and NEW.encode() in data and b'linux,fdt-handoff' in data
    usb_command = [*GCC, f'-DRESCUE_MAINLINE_RELEASE_ALT="{NEW}"',
                   snapshot / 'pad4_usb_setup.c', '-o', OUT / 'pad4-usb-setup']
    b.run(*usb_command)
    b.static_arm64(OUT / 'pad4-usb-setup')

    # init_boot: the base ramdisk with only the init and both watchdogs replaced.
    entries = b.cpio_read(b.run('lz4', '-dc', BASE / 'init_boot-unpacked/ramdisk'))
    assert hashlib.sha256(entries['rescue/mainline-init'][1]).hexdigest() == b.sha(BASE / 'mainline-init')
    old_entries = entries.copy()
    entries['rescue/mainline-init'] = b.entry(stat.S_IFREG | 0o755, data)
    old_modules = OLD_MODULES
    modules = KBUILD / 'modules-root/lib/modules' / NEW
    for release, tree, path in [(OLD, old_modules, 'rescue/mainline/qcom-wdt.ko'),
                                (NEW, modules, 'rescue/mainline/alternate/qcom-wdt.ko')]:
        ko = tree / 'kernel/drivers/watchdog/qcom-wdt.ko.zst'
        assert b.run('modinfo', '-F', 'vermagic', ko).decode().startswith(release + ' ')
        assert not b.run('modinfo', '-F', 'depends', ko).strip()
        entries[path] = b.entry(stat.S_IFREG | 0o644, b.run('zstd', '-dc', ko))
    # The running init's alternate watchdog is the old release's own.
    assert entries['rescue/mainline/qcom-wdt.ko'][1] == old_entries['rescue/mainline/alternate/qcom-wdt.ko'][1]
    changed = {'rescue/mainline-init', 'rescue/mainline/qcom-wdt.ko',
               'rescue/mainline/alternate/qcom-wdt.ko'}
    assert entries.keys() == old_entries.keys()
    raw = b.cpio_write(entries)
    decoded = b.cpio_read(raw)
    for name, (fields, payload) in old_entries.items():
        if name in changed:
            continue
        assert decoded[name][1] == payload, name
        assert decoded[name][0][1:6] == fields[1:6], name
        assert decoded[name][0][7:11] == fields[7:11], name
    compressed = b.run('lz4', '-l', '-c', data=raw)
    assert b.run('lz4', '-dc', data=compressed) == raw

    # boot: the Pad 4's own Image alone, as the base kernel-only image.
    ART.mkdir()
    image = (KBUILD / 'artifacts/Image').read_bytes()
    assert image[0x38:0x3c] == b'ARM\x64'
    assert f'Linux version {NEW} '.encode() in image
    put(OUT / 'init_boot-ramdisk.lz4', compressed)
    images = {name: pack(name, BASE / f'artifacts/{name}.img', payload)
              for name, payload in [('boot', KBUILD / 'artifacts/Image'),
                                    ('init_boot', OUT / 'init_boot-ramdisk.lz4')]}
    shutil.copy2(KBUILD / 'artifacts/Image', ART / 'Image')
    shutil.copy2(KBUILD / 'artifacts/kernel.config', ART / 'kernel.config')
    banner = [line for line in b.run('strings', ART / 'Image').decode().splitlines()
              if line.startswith(f'Linux version {NEW} ')][0].removeprefix('Linux version ')
    notes = OUT / 'kernel.notes'
    b.run('llvm-objcopy', '-O', 'binary', '--only-section=.notes', OBJ / 'vmlinux', notes)
    assert f'Linux version {NEW} '.encode() in (OBJ / 'vmlinux').read_bytes()
    dtb = KBUILD / 'artifacts/iceland.dtb'
    assert dtb.read_bytes() in image
    shutil.copy2(dtb, ART / 'kaanapali-oneplus-iceland.dtb')

    # rootfs: the module closure, its manifests and the two release helpers.
    rootfs = OUT / 'rootfs'
    helper = (BASE / 'rootfs' / HELPERS[0]).read_text()
    assert b.sha(BASE / 'rootfs' / HELPERS[0]) == base['rootfs_helpers'][HELPERS[0]]
    old_pair = f'{base["previous_release"]}|{OLD}'
    assert helper.count(old_pair) == 1
    put(rootfs / HELPERS[0], helper.replace(old_pair, f'{OLD}|{NEW}'))
    b.run('sh', '-n', rootfs / HELPERS[0])
    put(rootfs / HELPERS[1], (OUT / 'pad4-usb-setup').read_bytes(), 0o755)
    destination = rootfs / 'usr/lib/modules' / NEW
    shutil.copytree(modules, destination, symlinks=True,
                    ignore=lambda directory, names: [n for n in names if n in {'build', 'source'}])
    assert not any(f.is_symlink() for f in destination.rglob('*'))
    early = []
    old_sums = BASE / 'rootfs/usr/share/pad4-kernel' / OLD / 'early-modules.sha256'
    for line in old_sums.read_text().splitlines():
        digest, path = line.split(None, 1)
        relative = Path(path.strip()).relative_to('usr/lib/modules/' + OLD)
        assert b.sha(old_modules / relative) == digest
        early.append(f'{b.sha(destination / relative)}  usr/lib/modules/{NEW}/{relative}\n')
    share = rootfs / 'usr/share/pad4-kernel' / NEW
    put(share / 'early-modules.sha256', ''.join(early))
    all_sums = ''.join(f'{b.sha(f)}  usr/lib/modules/{NEW}/{f.relative_to(destination)}\n'
                       for f in sorted(destination.rglob('*')) if f.is_file())
    put(share / 'modules-files.sha256', all_sums)
    put(ART / 'modules-files.sha256', all_sums)
    with tarfile.open(ART / 'rootfs.tar.gz', 'w:gz') as t:
        # Existing system directories are not archived: only files and the
        # new module tree's own directories, so extraction changes nothing else.
        for f in sorted(rootfs.rglob('*')):
            name = str(f.relative_to(rootfs))
            if f.is_file() or name.startswith(f'usr/lib/modules/{NEW}'):
                t.add(f, arcname=name, recursive=False, filter=root_owned)
    module_count = sum(1 for f in destination.rglob('*.ko*'))

    helpers = {name: b.sha(rootfs / name) for name in HELPERS}
    helpers[f'usr/share/pad4-kernel/{NEW}/early-modules.sha256'] = b.sha(share / 'early-modules.sha256')
    helpers[f'usr/share/pad4-kernel/{NEW}/modules-files.sha256'] = b.sha(share / 'modules-files.sha256')
    report = dict(serial='59b8e7b3', slot='b', release=NEW, previous_release=OLD,
                  images=images, kernel_sha256=hashlib.sha256(image).hexdigest(),
                  dtb_sha256=b.sha(dtb), kernel_notes_sha256=b.sha(notes),
                  kernel_manifest_sha256=b.sha(KBUILD / 'manifest.json'),
                  kernel_banner=banner, config_sha256=b.sha(ART / 'kernel.config'),
                  module_count=module_count,
                  module_files=all_sums.count('\n'),
                  modules_manifest_sha256=b.sha(share / 'modules-files.sha256'),
                  rootfs_sha256=b.sha(ART / 'rootfs.tar.gz'), rootfs_helpers=helpers,
                  changed_ramdisk_entries=sorted(changed),
                  init_compile_command=list(map(str, init_command)),
                  usb_compile_command=list(map(str, usb_command)),
                  source_sha256={f.name: b.sha(f) for f in sorted(snapshot.iterdir())},
                  charging='standard battmgr; the SuperVOOC/PPS stack is not part of this kernel',
                  deployment_order=['init_boot', 'reboot-and-verify-current-kernel',
                                    'boot', 'reboot-and-validate'])
    put(OUT / 'manifest.json', json.dumps(report, indent=2) + '\n')
    put(OUT / 'artifacts.sha256', sums(ART))
    print(f'{NEW}: boot {images["boot"]["sha256"][:12]}, init_boot '
          f'{images["init_boot"]["sha256"][:12]}, {module_count} modules; offline only')


if __name__ == '__main__':
    main()
