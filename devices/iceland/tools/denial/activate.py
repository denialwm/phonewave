"""Activate the staged buffer-return Denial on denialpad4 (root): back up the launcher and plugin selection, stop
pad4-denial.service, point the launcher at the new installation (bundle and Settings) and select its composed shell,
start, verify. The launcher carries no composition flags; layered and native window composition stay built-in
defaults. Stopping Denial ends the graphical session, which also restarts the Android cell. Refuses to stop
Denial while anything but a stoppable ttyGS0 agetty holds the /dev/console flock (PID 1 deadlock, 2026-10-16). No automatic rollback: on failure the new
selection stays and the backup path is printed."""
from pathlib import Path
import hashlib, json, os, shutil, subprocess, time

assert os.geteuid() == 0 and Path('/etc/pad4-root-id').read_text().strip() == '59b8e7b3:archroot-v1'
BOOT = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
target = Path('/opt/denial/pad4-buffer-return-20261016')
previous = Path('/opt/denial/pad4-apply-late-20261016')
old_settings = str(previous / 'settings/denial-settings')
new_settings = str(target / 'settings/denial-settings')
launcher = Path('/usr/local/bin/pad4-denial-login')
FLAGS = ['export DENIAL_NATIVE_WINDOW_COMPOSITION=1\n', 'export DENIAL_LAYERED_COMPOSITION=1\n']
selection = Path('/home/logix/.config/denial/plugin-composition.json')
plugin = target / 'plugin-bundle'
results = Path('/var/lib/pad4-denial-backups')

subprocess.run(['sha256sum', '--quiet', '-c', 'payload.sha256'], cwd=target, check=True)
assert not any(p.stat().st_mode & 0o222 for p in plugin.rglob('*')) and not plugin.stat().st_mode & 0o222
deployment = json.loads((target / 'deployment.json').read_text())
data = json.loads((plugin / 'denial-plugin-bundle.json').read_text())
engine = deployment['engine_sha256']
assert sha(plugin / 'lib/libflutter_engine.so') == data['engine_sha256'] == engine == sha(target / 'shell/lib/libflutter_engine.so')
assert sha(plugin / 'lib/libapp.so') == data['app_sha256'] == deployment['plugin_bundle_app_sha256']
native_sha = sha(target / 'native/deniald')

old_pid = int(subprocess.check_output(['systemctl', 'show', 'pad4-denial.service', '-p', 'MainPID', '--value']))
assert old_pid and Path(f'/proc/{old_pid}/exe').resolve() == previous / 'native/deniald'
backup = results / ('buffer-return-20261016-attempt-' + str(time.monotonic_ns()))
backup.mkdir(mode=0o700)
shutil.copy2(launcher, backup / 'pad4-denial-login')
shutil.copy2(selection, backup / 'plugin-composition.json')
(backup / 'unit-effective.txt').write_bytes(subprocess.check_output(['systemctl', 'cat', 'pad4-denial.service']))


def atomic(path, text):
    st = path.stat()
    pending = path.with_name(path.name + '.buffer-return-new')
    with pending.open('x') as stream:
        stream.write(text)
        stream.flush()
        os.fsync(stream.fileno())
    pending.chmod(st.st_mode & 0o777)
    os.chown(pending, st.st_uid, st.st_gid)
    pending.replace(path)


def console_lock_guard():
    """PID 1 takes the /dev/console flock when a TTY unit stops; a stuck (agetty) pre-exec child of serial-getty@ttyGS0 holding it deadlocks PID 1."""
    st = os.stat('/dev/console')
    key = f'{os.major(st.st_dev):02x}:{os.minor(st.st_dev):02x}:{st.st_ino}'

    def holders():
        result = []
        for line in Path('/proc/locks').read_text().splitlines():
            fields = line.split()
            if len(fields) > 5 and fields[5] == key:
                pid = fields[4]
                comm = Path(f'/proc/{pid}/comm').read_text().strip() if Path(f'/proc/{pid}/comm').exists() else '?'
                result.append({'pid': pid, 'comm': comm, 'line': line})
        return result
    before = holders()
    if before:
        assert all(h['comm'] == '(agetty)' for h in before), ('Unexpected /dev/console lock holder; not stopping Denial', before)
        subprocess.run(['systemctl', 'stop', 'serial-getty@ttyGS0.service'], check=True, timeout=10)
        after = holders()
        assert not after, ('/dev/console lock still held after stopping the ttyGS0 getty', after)
        return {'key': key, 'holders_before': before, 'stopped_getty': True}
    return {'key': key, 'holders_before': [], 'stopped_getty': False}


record = {'target': str(target), 'previous': str(previous), 'old_pid': old_pid, 'backup': str(backup), 'boot_id': BOOT,
          'utc_seconds_before': time.time()}
save = lambda: (backup / 'activation.json').write_text(json.dumps(record, indent=2) + '\n')
save()


def user_ctl(*args):
    return subprocess.check_output(['runuser', '-u', 'logix', '--', '/bin/sh', '-c', '. /usr/local/libexec/pad4-denial-user-env; exec "$@"',
                                    'ctl', str(target / 'native/denialctl'), '--json', *args], text=True, timeout=8)


try:
    text = launcher.read_text()
    assert text.count('bundle=' + str(previous) + '\n') == 1 and '--start-locked' not in text
    assert text.count(f'export DENIAL_SETTINGS_BINARY={old_settings}\n') == 1
    assert not any(flag in text for flag in FLAGS)
    record['console_lock'] = console_lock_guard()
    save()
    os.sync()
    subprocess.run(['systemctl', 'stop', 'pad4-denial.service'], check=True, timeout=30)
    assert subprocess.check_output(['systemctl', 'show', 'pad4-denial.service', '-p', 'MainPID', '--value'], text=True).strip() == '0'
    record['stopped'] = True
    save()
    text = text.replace('bundle=' + str(previous) + '\n', 'bundle=' + str(target) + '\n')
    text = text.replace(f'export DENIAL_SETTINGS_BINARY={old_settings}\n', f'export DENIAL_SETTINGS_BINARY={new_settings}\n')
    atomic(launcher, text)
    old = json.loads(selection.read_text())
    chosen = dict(old, selected=str(plugin), previous=old['selected'], pending=None, last_start_ms=0, recent_starts=0)
    atomic(selection, json.dumps(chosen, indent=2) + '\n')
    record.update(launcher_sha256=sha(launcher), selection=chosen)
    save()
    subprocess.run(['runuser', '-u', 'logix', '--', 'env', 'XDG_RUNTIME_DIR=/run/user/1000', 'DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus',
                    'systemctl', '--user', 'daemon-reload'], check=True, timeout=15)
    os.sync()
    subprocess.run(['systemctl', 'start', 'pad4-denial.service'], check=True, timeout=30)
    deadline = time.monotonic() + 45
    ui = None
    while time.monotonic() < deadline:
        pid = int(subprocess.check_output(['systemctl', 'show', 'pad4-denial.service', '-p', 'MainPID', '--value']) or 0)
        if pid and pid != old_pid and Path(f'/proc/{pid}/exe').exists() and sha(f'/proc/{pid}/exe') == native_sha:
            try:
                ui = json.loads(user_ctl('ui', 'status'))
            except (subprocess.SubprocessError, ValueError):
                time.sleep(.2)
                continue
            if ui['plugin_healthy'] and not ui['error'] and ui['plugin_bundle'] == str(plugin):
                break
        time.sleep(.2)
    else:
        record['last_ui_status'] = ui
        raise RuntimeError('Latest session did not become healthy; selection retained for diagnosis')
    maps = {l.split()[-1] for l in Path(f'/proc/{pid}/maps').read_text().splitlines() if l.endswith(('/libapp.so', '/libflutter_engine.so'))}
    expected = {str(target / 'shell/lib/libflutter_engine.so'): engine, str(plugin / 'lib/libapp.so'): data['app_sha256']}
    assert {p: sha(p) for p in maps} == expected, maps
    environ = dict(s.split('=', 1) for s in Path(f'/proc/{pid}/environ').read_bytes().decode().split('\0') if '=' in s)
    assert environ['DENIAL_SETTINGS_BINARY'] == new_settings
    assert not {'DENIAL_LAYERED_COMPOSITION', 'DENIAL_NATIVE_WINDOW_COMPOSITION'} & environ.keys()
    time.sleep(3)
    journal = subprocess.check_output(['journalctl', '--no-pager', '-o', 'cat', f'_PID={pid}'], text=True)
    flagged = [l for l in journal.splitlines() if ' ERROR ' in l or ' WARN ' in l or '[ERROR:' in l or '[FATAL:' in l]
    native = [l for l in journal.splitlines() if 'native client composition' in l]
    record.update(pid=pid, ui_status=ui, mapped_artifacts=expected,
                  flags={k: environ.get(k) for k in ('DENIAL_LAYERED_COMPOSITION', 'DENIAL_NATIVE_WINDOW_COMPOSITION', 'DENIAL_RENDER_AUDIT')},
                  native_composition=native[:4], journal_flagged=flagged[:60], utc_seconds_after=time.time(),
                  boot_id_after=Path('/proc/sys/kernel/random/boot_id').read_text().strip())
except BaseException as error:
    record['activation_error'] = repr(error)
    raise
finally:
    save()
    print(json.dumps(record, indent=2), flush=True)
