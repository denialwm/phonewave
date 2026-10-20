#!/usr/bin/env python3
"""Activate the isolated JIIOV libfprint experiment; preserve packaged libfprint."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess

PW = Path(__file__).resolve().parents[4] / 'services/fingerprint/jiiov'
CACHE = Path(os.environ.get('XDG_CACHE_HOME') or Path.home() / '.cache') / 'phonewave/roadstr'
EVIDENCE = CACHE / 'fingerprint-evidence'
SPEC = importlib.util.spec_from_file_location('optical', Path(__file__).with_name('run-optical-test.py'))
optical = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(optical)
manifest = json.loads((EVIDENCE / 'libfprint-phone-manifest.json').read_text())
if manifest['exit_status'] != 0:
    raise SystemExit('Native libfprint build did not succeed')
prefix = manifest['prefix']
files = {
    'jiiov-native-probe': CACHE / 'fingerprint-native1/tee-probe',
    'jiiov-illumination-test': CACHE / 'fingerprint-native2/illumination-test',
    'jiiov-fprint-helper': PW / 'jiiov-fprint-helper.py',
}
commands = []
installed = {}
for name, local in files.items():
    remote = '/tmp/' + name + '-install'
    installed[name] = optical.stage(local, remote)
    commands.extend([f'install -m 755 {remote} /usr/local/libexec/{name}.new',
                     f'mv -T /usr/local/libexec/{name}.new /usr/local/libexec/{name}'])
config = f'''[Service]
Environment=LD_LIBRARY_PATH={prefix}/lib
Environment=FP_DRIVERS_ALLOWLIST=jiiov
Environment=JIIOV_DIAGNOSTIC_UI=1
RuntimeDirectory=jiiov-fprint
RuntimeDirectoryMode=0700
BindReadOnlyPaths=/run/user/1000/wayland-1:/run/jiiov-fprint/wayland-1
ReadWritePaths=/var/lib/moto70-fingerprint1/native-test-templates
DeviceAllow=/dev/smcinvoke rw
DeviceAllow=/dev/jiiov_fp rw
DeviceAllow=/dev/dma_heap/qcom,qseecom rw
DeviceAllow=/dev/dma_heap/qcom,qseecom-ta rw
'''
local_config = CACHE / 'fingerprint-native2/50-jiiov-native.conf'
local_config.write_text(config)
installed['service-dropin'] = optical.stage(local_config, '/tmp/jiiov-fprint-dropin-install')
command = '''set -eu
grep -q ZY22MMG59D /proc/bootconfig
install -d -m 755 /usr/local/libexec /etc/systemd/system/fprintd.service.d
install -d -m 700 /run/jiiov-fprint
''' + '\n'.join(commands) + '''
test "$(sha256sum /tmp/ancapp64.elf | cut -d ' ' -f 1)" = 412827b31fcdce11dd110610933410bd7da2be68e0d78a254499850526d122f8
install -m 400 /tmp/ancapp64.elf /var/lib/moto70-fingerprint1/ancapp64.elf.new
mv -T /var/lib/moto70-fingerprint1/ancapp64.elf.new /var/lib/moto70-fingerprint1/ancapp64.elf
/usr/local/libexec/jiiov-fprint-helper probe
/usr/local/libexec/jiiov-fprint-helper list
if test -e /etc/systemd/system/fprintd.service.d/50-jiiov-native.conf; then
 cp -a /etc/systemd/system/fprintd.service.d/50-jiiov-native.conf /var/lib/moto70-fingerprint1/fprintd-dropin.previous
fi
install -m 644 /tmp/jiiov-fprint-dropin-install /etc/systemd/system/fprintd.service.d/50-jiiov-native.conf
systemctl daemon-reload
systemctl restart fprintd.service
systemctl is-active fprintd.service
busctl call net.reactivated.Fprint /net/reactivated/Fprint/Manager net.reactivated.Fprint.Manager GetDevices
'''
log = EVIDENCE / 'fprintd-install-live.log'
with log.open('w') as output:
    result = subprocess.run(optical.SSH + [command], stdout=output, stderr=subprocess.STDOUT)
installed['exit_status'] = result.returncode
installed['library_prefix'] = prefix
(EVIDENCE / 'fprintd-install-manifest.json').write_text(json.dumps(installed, indent=2) + '\n')
print(log.read_text())
raise SystemExit(result.returncode)
