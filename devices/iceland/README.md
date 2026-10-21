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

Deploying Denial: tools/denial/build-recipe.py builds a candidate from
/mnt/development/denial-os/denial, build-composed-shell.py composes the shell from the vault
template, and package.py packs payload.tar.zst; all three write under
~/.cache/phonewave/iceland/denial. Copy the archive, archive.sha256, stage.py and
activate.py to the tablet's ~/.cache/pad4-deploy-buffer-return-20261016, then run
stage.py and activate.py there with sudo -n. Activation restarts the graphical
session. The Denial runtime libraries in /opt are copied from the vault.
