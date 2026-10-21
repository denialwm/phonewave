# OnePlus Ace 3 (aston)

The phone runs Arch Linux on the shared devices kernel from slot B. rootfs/ holds
the files installed for this device, at their install path and byte for byte as
on the phone. boot/ packages a kernel release; tools/ runs on the host, reaches
the phone as root through `ssh aston` and writes to ~/.cache/phonewave/aston.

Private inputs live in the vault, `${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/aston`:
boot/ the installed and rollback boot images with the evidence each package
chains on, and the init_boot image; backups/ partition copies, among them the
stock slot-B partitions (aston-survey-20261018) and the modem EFS; firmware/
unpackaged and factory fingerprint firmware; private-rootfs/ the SSH key and
sudoers; reference/ device trees; build-deps/ the deployed fingerprint worker
and libfprint source, which no longer rebuild.

Packaging a kernel release: build linux-perpetua through linux-perpetua-build,
which leaves releases/devices-N-aston, then run `boot/package_kernel.py
--generation N --previous P`, where P is the release the phone runs. It checks
the vault's boot_b-devicesP.img against kernel-flash-evidence-devicesP.json,
uses it as the android-gki template and writes boot_b-devicesN.img,
boot-evidence-devicesN/ and boot-package-devicesN.json to the vault. Offline.

Installing it: `tools/stage_modules.py --generation N` installs the module
directory without loading anything. `tools/activate_kernel.py --generation N
--previous-generation P` copies the image over, checks init_boot_b against
boot-preflight.json, the current boot_b and the modules, then writes boot_b with
the phone's /var/lib/aston-release/remote_boot_flash.py (equal to tools/), which
backs up, reads back and records kernel-flash-evidence-devicesN.json.
`tools/reboot_candidate.py --generation N` fetches the backup, rechecks both
partitions and reboots. Only boot_b is written; the previous image is the
rollback. init_boot_b stays the devices10 image: boot/firstboot's static PID 1
as a root-owned newc cpio, lz4 -l, on the stock init_boot_b.

Fingerprint: build as services/fingerprint/uff/README.md says, then run
tools/fingerprint/run_probe.py (identify the sensor), stage_service.py (worker,
helper, presentation client, backend.json), build_libfprint.py (builds the
snapshot on the phone in /opt/libfprint-uff) and install_fprintd.py (selects it,
restarts fprintd). stage_compositor.py stages the deniald named by the cached
compositor-build.json; activate_compositor.py selects it through
denial.service.d/zz-fingerprint.conf and restarts the graphical session.

Modem: dmail (services/modem/dmail) owns it. rootfs/ carries
`/usr/lib/dmail/modem-setup`, the `aston-modem-rmtfs` and
`aston-modem-tqftpserv` units it starts, `/etc/dmail/device.toml` and
`aston-dmail.conf`, which puts logix in the `dmail` group. The EFS copy never
enters this repo: on the phone `/var/lib/aston-modem/efs` links to the copy
directory, and `efs.handset` beside it holds the SoC serial, written on the
phone from `/sys/devices/soc0/serial_number` and never printed. Install dmail
with its `make install` layout, then run `systemd-sysusers`, reload systemd
and D-Bus and enable `dmail.service`. The unit names match the cellular kit's
transient units, so `modem-setup` never starts a second rmtfs next to them.

Packages: on top of the image, `pacman -S` installs `noto-fonts-emoji`, the
colour emoji font the native apps draw emoji with.

Memory: zram-swap (services/memory/zram-swap) gives 7.3 GB of zstd swap, half
of memory; rootfs/ enables it with a swap.target.wants link and keeps GPU
buffers out of swap with `aston-msm-gpu-eviction.conf`, as on the Pad 4.
Since devices40 the kernel backs GPU buffers with huge pages
(`msm.transparent_hugepage`, on by default); `dmesg` says "Using Transparent
Hugepage".
