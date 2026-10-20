"""Read-only runtime health snapshot; changes nothing and triggers no UI."""
from pathlib import Path
import json, os, subprocess


def read(p, default=None):
    try:
        return Path(p).read_text().strip()
    except OSError:
        return default


def run(*args, timeout=20):
    try:
        r = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
        return r.stdout.strip()
    except (OSError, subprocess.TimeoutExpired) as e:
        return f'ERROR {e}'


assert read('/etc/pad4-root-id') == '59b8e7b3:archroot-v1'
units = ['pad4-hardware', 'pad4-usb', 'pad4-wifi', 'pad4-audio', 'pad4-gpu-thermal',
         'pad4-vm-tuning', 'pad4-denial', 'pad4-supervooc', 'bluetooth', 'NetworkManager',
         'serial-getty@ttyGS0']
user = ['runuser', '-u', 'logix', '--', 'env', 'XDG_RUNTIME_DIR=/run/user/1000']
psu = Path('/sys/class/power_supply')
bat = psu / 'qcom-battmgr-bat'
thermal = {}
for z in Path('/sys/class/thermal').glob('thermal_zone*'):
    t = read(z / 'type', '')
    if t.startswith(('cpu', 'gpuss')):
        thermal[t] = int(read(z / 'temp', '0'))
gpu = next(Path('/sys/class/devfreq').glob('*.gpu'), None)
policies = {}
for pol in sorted(Path('/sys/devices/system/cpu/cpufreq').glob('policy*')):
    policies[pol.name] = {k: read(pol / k) for k in
                          ['scaling_driver', 'scaling_governor', 'cpuinfo_max_freq', 'scaling_cur_freq', 'related_cpus']}
klog = run('journalctl', '-k', '-b', '--no-pager', '-o', 'short-monotonic')
result = dict(
    release=os.uname().release,
    boot_id=read('/proc/sys/kernel/random/boot_id'),
    cmdline=read('/proc/cmdline'),
    handoff={n: read('/sys/firmware/devicetree/base/chosen/' + n, '').rstrip('\0')
             for n in ['linux,fdt-handoff', 'pad4,handoff'] if Path('/sys/firmware/devicetree/base/chosen/' + n).exists()},
    system=run('systemctl', 'is-system-running'),
    failed=run('systemctl', '--failed', '--no-legend', '--plain'),
    units={u: run('systemctl', 'show', u, '-p', 'ActiveState', '-p', 'SubState', '-p', 'Result',
                  '-p', 'ConditionResult', '--value').split('\n') for u in units},
    deniald=run('pgrep', '-x', 'deniald'),
    droidloom=run(*user, 'systemctl', '--user', 'show', 'droidloom.service', 'droidloom-recovery.service',
                  '-p', 'Id', '-p', 'ActiveState', '-p', 'NRestarts'),
    wifi=run('nmcli', '-t', '-f', 'DEVICE,TYPE,STATE,CONNECTION', 'device'),
    wifi_link=run('sh', '-c', 'for i in /sys/class/net/*/phy80211; do d=${i%/phy80211}; iw dev ${d##*/} link; done'),
    bluetooth=dict(hci=sorted(p.name for p in Path('/sys/class/bluetooth').glob('hci*')),
                   address=read('/sys/class/bluetooth/hci0/address'),
                   powered=run('sh', '-c', 'bluetoothctl show | grep -E "Powered|Name"')),
    battery={n: read(bat / n) for n in ['capacity', 'status', 'current_now', 'voltage_now', 'temp', 'charge_type']},
    supplies={p.name: read(p / 'online') for p in psu.iterdir() if (p / 'online').exists()},
    cpu_max_c=max((v for k, v in thermal.items() if k.startswith('cpu')), default=0) / 1000,
    gpu_max_c=max((v for k, v in thermal.items() if k.startswith('gpuss')), default=0) / 1000,
    gpu_zones=sum(1 for k in thermal if k.startswith('gpuss')),
    gpu_devfreq=None if gpu is None else {k: read(gpu / k) for k in ['governor', 'cur_freq', 'max_freq', 'min_freq', 'available_frequencies']},
    cpu_capacity=[int(read(f'/sys/devices/system/cpu/cpu{i}/cpu_capacity', '0')) for i in range(8)],
    cpufreq=policies,
    sound=read('/proc/asound/cards'),
    touch='NVTCapacitiveTouchScreen' in read('/proc/bus/input/devices', ''),
    dsi=read('/sys/class/drm/card0-DSI-1/status'),
    dp={p.name: read(p / 'status') for p in Path('/sys/class/drm').glob('card0-DP-*')},
    usb_gadget=read('/sys/kernel/config/usb_gadget/opd2514_rescue/UDC'),
    peri=[l for l in klog.splitlines() if 'Peri' in l or 'TMEL' in l][-12:],
    kernel_errors=run('journalctl', '-k', '-b', '-p', 'err', '--no-pager', '-o', 'short-monotonic').splitlines()[:80],
    kernel_warnings=len(run('journalctl', '-k', '-b', '-p', 'warning', '--no-pager').splitlines()),
    init_ms=next((l for l in klog.splitlines() if 'Run /' in l or 'switch_root' in l.lower()), None),
)
print(json.dumps(result, indent=2), flush=True)
