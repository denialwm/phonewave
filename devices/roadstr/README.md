# Motorola Edge 70 (roadstr)

The phone runs Arch Linux on linux-perpetua from slot B. Slot A keeps stock
Android, and the bootloader falls back to it while slot B is not marked
successful. boot_b holds the kernel with its device tree built in; init_boot_b
the init_boot every phone shares (`image/init-boot`). boot/ holds the boot
image scripts. The ADSP, Wi-Fi and GPU zap firmware come from the phone's
own `modem_b` (`image/stock-firmware`). Everything else on the phone comes
from the image every phone shares (`image/`).
`ssh roadstr` reaches the phone as root over USB, `ssh roadstr-wifi` over Wi-Fi.

Private inputs live in the vault, `${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/roadstr`:
stock/ the W1WR36.39-115 firmware, whose boot.img is the boot header template;
boot/ the built images, the initramfs busybox and dropbear, and rollback-6.6/
(the old 6.6 boot chain, a record only);
firmware/ the GPU, Wi-Fi, Bluetooth and fingerprint firmware; private-rootfs/
root's authorized_keys, which a `--dev-access` init_boot also carries, and sudoers; backups/
partition copies.

Packaging a kernel release N: `boot/build-boot.sh N [--name NAME]` builds it
through linux-perpetua-build unless the release exists, and writes the vault's
boot/boot-NAME.img: the kernel, then its modules, the GPU microcode, the
Bluetooth patch, the Wi-Fi board data and `boot/tail/`, as the filesystem
`image/boot-tail` packs. `boot/tail/stock-firmware.conf` and `.sha256` name
the stock firmware the kernel was tested with, W1WR36.39-115.

Installing a kernel by hand: write boot-NAME.img to
/dev/disk/by-partlabel/boot_b and compare its sha256. `tools/phone-reboot
roadstr` then checks that boot_b carries its kernel's modules and that slot B
is marked successful; `--reboot` reboots.
`boot/parity-check.sh | ssh roadstr sh` prints a read-only health report.

Fingerprint: fprintd runs on the `fingerprint-phonewave` packages, as on
every phone (services/fingerprint). The tail's `fingerprint/sensor.conf`
names the JIIOV sensor and `denial/fingerprint.json` its mark and panel;
`boot/build-boot.sh` joins the signed `ancapp64` into the tail. The kernel
gives `/dev/tee_fp` for the sensor and `/dev/fp_illumination` in the panel
driver. Preserve persist's `jiiov/` factory calibration and keep it
read-only.
