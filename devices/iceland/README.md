# OnePlus Pad 4 (iceland)

The tablet runs Arch Linux on the shared devices kernel from slot B. rootfs/ holds
the files installed for this device, at their install path and byte for byte as
on the tablet. Its two binaries are not tracked: pad4-audio-init is built from
tools/pad4-audio-init.rs and pad4-usb-setup by boot/package_pad4_devices.py.
boot/ packages a kernel release; tools/ runs on the host.

Two SSH aliases reach the tablet: `ssh iceland` over Wi-Fi as logix, whose sudo -n
needs no password, and `ssh iceland-usb` as root over the USB gadget network.

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/iceland`: boot/devicesN holds each packaged release with its images, audits and rollback
copies; stock/ the OPD2514 stock images and OTA; backups/ partition copies;
firmware/ the unpackaged firmware at its install path; private-rootfs/ the SSH
keys and sudoers; denial/ the Denial composition template, ABI probe and runtime.

Packaging a kernel release: build linux-perpetua through linux-perpetua-build,
which leaves releases/devices-N-iceland and the vmlinux in
/mnt/exty/linux-perpetua/build/iceland. Then run `boot/package_pad4_devices.py N
PREV`, where PREV is the release the tablet runs. It works offline, takes its base
images from the vault's boot/devicesPREV and writes boot/devicesN. The new
init_boot boots either kernel, so it is installed and proven before boot changes.

Installing it: `tools/deploy.py N STEP` runs one step at a time and keeps every log
and audit in the vault. The order is health NAME, audit preflash, fetch, stage,
flash init_boot, reboot transition, wait RELEASE BOOT_ID, audit and health,
flash boot, reboot new-kernel, wait, audit and health again. Writes go through
the tablet's own pad4-flash, which checks the image and reads it back.

Only boot_b and init_boot_b are ever written. Never set_active, erase, format, GPT
edits or EDL. Tablet #1 was bricked that way.

Deploying Denial: the release name ID, for example pad4-dev-20261024, becomes
/opt/denial/ID on the tablet. On the workstation every step reuses one folder,
~/.cache/phonewave/iceland/denial/release, and one Rust cache beside it.
tools/denial/build-recipe.py build --denial SOURCE empties that folder and
builds a candidate from a Denial checkout or worktree, build-composed-shell.py
ID composes the shell from the vault template, and package.py ID PREVIOUS packs
payload.tar.zst, where PREVIOUS is the /opt/denial installation the tablet
runs. Copy the archive, archive.sha256, stage.py and activate.py over the
tablet's ~/.cache/pad4-deploy, then run `sudo -n python3 stage.py ID` and `sudo
-n python3 activate.py ID` there. Activation restarts the graphical session and
Droidloom with it. The Denial runtime libraries in /opt are copied from the
vault. The Settings launcher entry runs
/usr/local/bin/pad4-denial-settings, which starts the Settings of the running
release, so activation never needs to touch it.
The portal units, etc/systemd/user/denial-portal.service and its D-Bus service
file, name the release whose denial-portal and permission dialog they run:
repoint and reinstall both when either changes.
