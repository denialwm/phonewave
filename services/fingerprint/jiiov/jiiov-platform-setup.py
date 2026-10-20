#!/usr/bin/python3
"""Load the audited Roadstr sensor module and arm FOD/double-tap gestures."""
import hashlib
import os
from pathlib import Path
import stat
import subprocess

RELEASE = '6.6.98-moto70-droidloom2-4k'
MODULE_HASH = '6d95ea698bbc8846c060cf34315ea6305e338cc2ef8b01faaf45cc8ed7988611'


def main():
    if os.geteuid() != 0 or 'ZY22MMG59D' not in Path('/proc/bootconfig').read_text():
        raise RuntimeError('This hardware profile is for the validated Roadstr only')
    if os.uname().release != RELEASE:
        raise RuntimeError('Revalidate the fingerprint module after a kernel update')
    module = Path('/usr/lib/modules') / RELEASE / 'extra/anc_fps_mmi.ko'
    if hashlib.sha256(module.read_bytes()).hexdigest() != MODULE_HASH:
        raise RuntimeError('Fingerprint module differs from the audited release')
    if not Path('/sys/module/anc_fps_mmi').exists():
        # The boot manifest supplies the module files without a kmod name
        # index. Load the already hash-verified object, as in the sensor tests.
        subprocess.run(['/usr/bin/insmod', str(module)], check=True)
    major, minor = map(int, Path('/sys/class/jiiov_fp/jiiov_fp/dev').read_text().strip().split(':'))
    device = Path('/dev/jiiov_fp')
    if not device.exists():
        os.mknod(device, stat.S_IFCHR | 0o600, os.makedev(major, minor))
    metadata = device.lstat()
    if not stat.S_ISCHR(metadata.st_mode) or metadata.st_rdev != os.makedev(major, minor):
        raise RuntimeError('Unexpected fingerprint device node')
    device.chmod(0o600)
    # These are independent enable commands, not an aggregate bitmask.
    # Keep zero-tap/FOD enabled and add hardware-decoded double-tap wake.
    gesture = Path('/sys/class/touchscreen/primary/gesture')
    gesture.write_text('17\n')  # 0x11: FOD enable
    gesture.write_text('49\n')  # 0x31: double-tap enable
    print('Audited JIIOV module ready; touchscreen FOD and double-tap gestures armed')


if __name__ == '__main__':
    main()
