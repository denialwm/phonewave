"""Host side of a packaged Pad 4 release, one step per call.

Usage: tools/deploy.py N STEP [ARGS], for the release in the vault's boot/devicesN.
Steps: audit NAME, fetch, stage, flash PART, reboot STAGE, wait RELEASE BOOT_ID,
health NAME. Writes only boot_b/init_boot_b, through the tablet's audited
pad4-flash with its own checks; never any other partition. Root steps use
the tablet's passwordless sudo -n; no password is held or sent. SSH runs
over Wi-Fi (ssh iceland) unless PAD4_HOST names another address.
"""
from pathlib import Path
import hashlib, json, os, select, shlex, subprocess, sys, tarfile, time

HERE = Path(__file__).resolve().parent
OUT = Path(os.environ.get('PHONEWAVE_VAULT', '/mnt/development/denial-os/device-vault')) / f'iceland/boot/devices{sys.argv[1]}'
CACHE = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache')) / f'phonewave/iceland/devices{sys.argv[1]}'
M = json.loads((OUT / 'manifest.json').read_text())
NEW, OLD = M['release'], M['previous_release']
REMOTE = '/var/lib/pad4-release/' + NEW
SSH = ['ssh', *(['-o', 'HostName=' + os.environ['PAD4_HOST']] if 'PAD4_HOST' in os.environ else []), 'iceland']


def sha(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def root(script, output=None, timeout=300):
    """Run Python as root on the tablet; stdout to OUTPUT if given."""
    command = SSH + ['sudo -n python3 -u -c ' + shlex.quote(script)]
    r = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout)
    if output:
        Path(output).write_bytes(r.stdout)
        Path(str(output) + '.stderr').write_bytes(r.stderr)
    assert r.returncode == 0, r.stderr.decode(errors='replace')[-3000:]
    return r.stdout.decode(errors='replace')


def audit(name):
    out = OUT / f'{name}-audit.json'
    root((HERE / 'audit-tablet.py').read_text(), out)
    a = json.loads(out.read_text())
    print(json.dumps({k: a[k] for k in ['release', 'boot_id', 'module_files_verified', 'denial']}))
    print('boot_b', a['partitions']['boot_b'][:12], 'init_boot_b', a['partitions']['init_boot_b'][:12])


def fetch():
    """Fresh rollback copies of both slot-B images, checked against the audit."""
    a = json.loads((OUT / 'preflash-audit.json').read_text())
    (OUT / 'rollback').mkdir(exist_ok=True)
    for n in ['boot', 'init_boot']:
        target = OUT / 'rollback' / f'{n}_b.img'
        root("import sys,shutil; shutil.copyfileobj(open('/dev/disk/by-partlabel/" + n +
             "_b','rb',buffering=0),sys.stdout.buffer,4*1024*1024)", target)
        assert sha(target) == a['partitions'][n + '_b'] == M['images'][n]['baseline_sha256'], n
        print(n, 'rollback copy verified', sha(target)[:12])


def stage():
    CACHE.mkdir(parents=True, exist_ok=True)
    with tarfile.open(CACHE / 'release.tar.gz', 'w:gz') as t:
        for name in ['artifacts/boot.img', 'artifacts/init_boot.img', 'artifacts/rootfs.tar.gz',
                     'artifacts/kernel.config', 'manifest.json', 'preflash-audit.json']:
            t.add(OUT / name, arcname=name)
        t.add(HERE / 'install-release.py', arcname='install-release.py')
    digest = sha(CACHE / 'release.tar.gz')
    staged = f'/home/logix/.cache/pad4-{NEW}-release.tar.gz'
    with (CACHE / 'release.tar.gz').open('rb') as f:
        subprocess.run(SSH + ['umask 077; cat > ' + shlex.quote(staged)], stdin=f, check=True)
    script = f'''from pathlib import Path
import hashlib,tarfile,subprocess
assert Path('/etc/pad4-root-id').read_text().strip()=='59b8e7b3:archroot-v1'
p=Path({staged!r})
assert hashlib.sha256(p.read_bytes()).hexdigest()=={digest!r}
root=Path({REMOTE!r})
root.mkdir(mode=0o700)
with tarfile.open(p) as t:t.extractall(root,filter='data')
subprocess.run(['python3',str(root/'install-release.py')],check=True)
'''
    print(root(script, OUT / 'install.log'))


def flash(part):
    """Run the unmodified audited updater through SSH's remote PTY."""
    assert part in ['boot', 'init_boot']
    image = REMOTE + '/artifacts/' + part + '.img'
    command = 'sudo -n pad4-flash ' + part + ' ' + shlex.quote(image) + ' --write'
    p = subprocess.Popen(['ssh', '-tt'] + SSH[1:] + [command], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log, pending = bytearray(), bytearray()

    def until(tokens, timeout=120):
        end = time.monotonic() + timeout
        while True:
            for i, token in enumerate(tokens):
                at = pending.find(token)
                if at >= 0:
                    del pending[:at + len(token)]
                    return i
            assert time.monotonic() < end, 'Prompt timeout'
            if select.select([p.stdout], [], [], 1)[0]:
                data = os.read(p.stdout.fileno(), 65536)
                assert data, ('SSH ended', p.poll(), bytes(log).decode(errors='replace'))
                pending.extend(data)
                log.extend(data)
                (OUT / (part + '-write.log')).write_bytes(log)

    def send(s):
        p.stdin.write((s + '\n').encode())
        p.stdin.flush()
    try:
        until([b'Type exactly'])
        phrase = 'write ' + part + '_b ' + M['images'][part]['sha256'][:12]
        until([('"' + phrase + '": ').encode()])
        send(phrase)
        until([b'Write and complete readback verified. No reboot performed.'])
        p.wait(timeout=30)
        assert p.returncode == 0, p.returncode
    finally:
        if p.poll() is None:
            p.terminate()
            p.wait(timeout=10)
        (OUT / (part + '-write.log')).write_bytes(log)
    print(bytes(log).decode(errors='replace'))


def reboot(stage):
    """Reboot only from the exact expected kernel and partition pair."""
    boot = M['images']['boot']['sha256' if stage == 'new-kernel' else 'baseline_sha256']
    script = f'''from pathlib import Path
import hashlib,os,subprocess
def sha(p):
 with open(p,'rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
assert Path('/etc/pad4-root-id').read_text().strip()=='59b8e7b3:archroot-v1'
assert 'androidboot.slot_suffix = "_b"' in Path('/proc/bootconfig').read_text()
assert os.uname().release=={OLD!r}
assert sha('/dev/disk/by-partlabel/init_boot_b')=={M['images']['init_boot']['sha256']!r}
assert sha('/dev/disk/by-partlabel/boot_b')=={boot!r}
print('Reboot: verified',os.uname().release,'with init_boot',{M['images']['init_boot']['sha256'][:12]!r},'and boot',{boot[:12]!r},flush=True)
subprocess.Popen(['systemctl','reboot'])
'''
    print(root(script, OUT / f'reboot-{stage}.log', timeout=60))


def wait(expected, previous):
    args = SSH[:-1] + ['-o', 'ConnectTimeout=3', SSH[-1], 'uname -r; cat /proc/sys/kernel/random/boot_id']
    end = time.monotonic() + 180
    while time.monotonic() < end:
        try:
            r = subprocess.run(args, capture_output=True, text=True, timeout=8)
        except subprocess.TimeoutExpired:
            continue
        lines = r.stdout.splitlines()
        if r.returncode == 0 and len(lines) == 2 and lines[1] != previous:
            assert lines[0] == expected, lines
            print(json.dumps(dict(release=lines[0], boot_id=lines[1])), flush=True)
            return
        time.sleep(2)
    raise RuntimeError('Tablet did not return on the expected kernel')


def health(name):
    root((HERE / 'health.py').read_text(), OUT / f'{name}-health.json')
    print((OUT / f'{name}-health.json').read_text()[:6000])


if __name__ == '__main__':
    step, *rest = sys.argv[2:]
    {'audit': audit, 'fetch': fetch, 'stage': stage, 'flash': flash,
     'reboot': reboot, 'wait': wait, 'health': health}[step](*rest)
