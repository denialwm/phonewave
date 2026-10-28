# Nubia RedMagic 11 Pro (nx809j)

Bring-up on the shared devices kernel (linux-perpetua), from slot B. The
phone keeps its stock Android boot chain; the kernel replaces the
bootloader's device tree with its own (FDT handoff).

Private inputs live in the vault,
`${PHONEWAVE_VAULT:-/mnt/development/denial-os/device-vault}/nx809j`:
record.md, the add-device record; backups/ this unit's partition copies
(`backups/manifest.json`); survey/ what was read from the phone; ssh/ the
rescue host key once one exists.

## Partitions

Only `boot_b` and `init_boot_b` are written, one at a time. Never write
an `_a` partition, never switch the active slot, and never write vbmeta,
dtbo, vendor_boot, bootloader, firmware or calibration partitions. Slot A
is marked unbootable by the bootloader; if slot B ever becomes unbootable
too, ask the user before any slot command. The rollback for each partition
is its stock copy in `backups/slot_b/`.

Native credential storage has one explicit user-authorized exception:
`nx809j-secure-storage.service` may update encrypted secure files under
`persist/data` and relay trusted-app authenticated RPMB commits in its selected
canonical mode. Factory calibration files remain read-only; RPMB key
programming and raw partition flashing remain forbidden. A fresh persist
backup is held privately, but RPMB monotonic state has no verified rollback.
The user accepted that limitation for native PIN/fingerprint bring-up.

The stock ABL answers `fastboot flash` and `fastboot boot` with "unknown
command", and fastbootd reports the device locked and refuses downloads.
Images are written with `dd` from a running system: rooted Android, or
the rescue. Before each write, check the image against the packaging
manifest and that the running slot is `_b`; read the partition back and
compare hashes afterwards. If nothing boots, the partitions are restored
over EDL.

Slot B's boot attributes live in the GPT entry of `boot_b` on `sde`. This
bootloader leaves the successful bit set on every boot, so rescue boots
do not use up retries.

## Packaging

Build linux-perpetua through linux-perpetua-build
(`tools/devices-build nx809j N`), then:

```sh
boot/package.py RELEASE_DIR \
  --busybox STATIC_ARM64_BUSYBOX --dropbear STATIC_ARM64_DROPBEARMULTI \
  --authorized-keys ~/.ssh/id_ed25519.pub
```

It writes `boot_b.img` (the kernel) and `init_boot_b.img` (the rescue
ramdisk) to `~/.cache/phonewave/nx809j/rescue`, through Perpetua's
`android-gki` adapter with this unit's stock images as templates, and
records hashes in `manifest.json`. The ramdisk is LZ4 legacy: the
bootloader appends it to the vendor ramdisk, which is LZ4 legacy too and
has no end marker, so the kernel would read any other format as more LZ4
chunks and lose the rescue. `--root-partuuid UUID` makes the
ramdisk start an installed ext4 root from that GPT partition; without it
the phone stays in the RAM rescue. It never formats anything.

## RAM rescue

SSH as root on 10.77.73.2 over USB ECM; the phone's DHCP server gives the
host 10.77.73.1 with no gateway or DNS. An ACM serial console runs on the
same gadget. Without a host key in the vault, dropbear makes one on the
first connection; copy it to `ssh/dropbear_ed25519_host_key` in the vault
so later images keep it.

## Root filesystem

Arch Linux ARM on a partition of its own, following Perpetua's
`devices/arch-linux-arm.md`. Android keeps `userdata`: its f2fs was shrunk
online from rooted Android (`F2FS_IOC_RESIZE_FS`) and the end of `sda`
became a partition named `linux`, GPT PARTUUID
`2225dabe-3937-4455-83a4-58a737b38f2f`, ext4 labelled `ALARM`. The vault
keeps the partition tables from before and after (`backups/gpt*`). Android
still boots from slot B's stock images; formatting `userdata` from Android
is its own recovery and leaves `linux` alone.

Package with `--root-partuuid 2225dabe-3937-4455-83a4-58a737b38f2f` to boot
the installed root; without it the phone stays in the RAM rescue.

`rootfs/` installs the ECM+ACM gadget service and a networkd profile matched
by the device-side MAC, on the rescue's link: the phone is 10.77.73.2 and
leases 10.77.73.1 to the host, without a router, DNS or NTP. Enable `sshd`
and `systemd-networkd`; root and `logix` log in with keys only. Install the
exact release's module tree under `/usr/lib/modules`.

## Denial

`nx809j-denial.service` runs Denial as `logix` on tty1, in a logind
session, and starts at boot (`systemctl enable nx809j-denial`). It opens
the display and the GPU by path (`platform-9801000.display-controller-card`
and `-render`) and runs the set that `/var/lib/nx809j-denial/current`
names. Its output configuration starts from `/usr/share/nx809j/outputs.conf`
(scale 2). Besides the base image it needs `seatd` (libseat), `libinput`,
`wayland`, `xorg-xwayland`, `polkit`, `xkeyboard-config` and fonts
(`noto-fonts`, `noto-fonts-emoji`, `noto-fonts-cjk`, `ttf-dejavu`,
`ttf-liberation`); deniald stops at startup without Xwayland.

To install a build of the `dev` stream (`tools/build start denial aston`
builds the same AArch64 candidate for every phone):

```sh
B=$(readlink -f /mnt/development/denial-os/builds/aston/denial/latest)
id=denial-${B##*/}
ssh root@10.77.73.2 mkdir -p /var/lib/nx809j-denial/.staging-$id
rsync -a --chown=root:root $B/native $B/shell $B/settings $B/SHA256SUMS \
    $B/manifest.json $B/source-manifest.json $B/source.patch \
    $B/flutter.patch $B/skia.patch $B/build-recipe.py \
    root@10.77.73.2:/var/lib/nx809j-denial/.staging-$id/
ssh root@10.77.73.2 "set -e; cd /var/lib/nx809j-denial
    (cd .staging-$id && sha256sum --quiet -c SHA256SUMS && chmod -R a-w .)
    mv -T .staging-$id $id; echo previous=\$(readlink current)
    ln -sfn $id .current.new; mv -T .current.new current
    systemctl restart nx809j-denial"
```

## Modem

dmail (services/modem/dmail) owns it. rootfs/ carries
`/usr/lib/dmail/modem-setup`, the `nx809j-modem-rmtfs` and
`nx809j-modem-tqftpserv` units it starts, `/etc/dmail/device.toml` and
`nx809j-dmail.conf`, which puts logix in the `dmail` group. The data
endpoints are those of Kaanapali's IPA (AP to modem 2, modem to AP 24), the
same as the Ace 3. The modem firmware lives in
`/usr/lib/firmware/qcom/kaanapali/nubia/nx809j/`; `rmtfs`, `tqftpserv` and
`qrtr-lookup` in `/usr/local/bin`.

The EFS copy never enters this repo: on the phone `/var/lib/nx809j-modem/efs`
links to the copy directory, and `efs.handset` beside it holds the SoC serial,
written on the phone from `/sys/devices/soc0/serial_number` and never printed.
Install dmail with its `make install` layout, then run `systemd-sysusers`,
reload systemd and D-Bus and enable `dmail.service`.

## Sensors

steinerd (services/sensors/steinerd) offers orientation, acceleration,
rotation rate, compass heading, light and proximity from the ADSP's sensor
core: lsm6dsv accelerometer and gyroscope, stk3a7x light and proximity,
ak0991x magnetometer, through the core's AKM rotation vector for the
compass. The ADSP reads its sensor
configuration from the host when it starts, so
`nx809j-sensors-hexagonrpcd.service` runs `hexagonrpcd -s` from boot on
`/dev/fastrpc-adsp-secure`. hexagonrpcd (linux-msm/hexagonrpc `da7a374`,
GPL-3.0) is cross-built with meson, prefix `/usr/local`, rpath
`/usr/local/lib`, and installed with `libhexagonrpc` in `/usr/local`.

The files it serves are the phone's own and never enter this repo:
`tools/sensor-files`, run on the phone as root, copies them from the stock
`vendor_b` and persist, both read-only, into
`/usr/share/qcom/kaanapali/Nubia/NX809J`. Restart the phone afterwards. Then
install steinerd with its `make install` layout and enable `steinerd.service`;
`steinerctl list accel proximity ambient_light` shows the three sensors.

## Camera

braun (services/camera/braun), installed with its `make install`, finds the
OV50E40 main camera, the OV50D40 ultrawide, the OV02F10 auxiliary camera and
the OV16A1Q front camera, and develops the Camera app's photos. rootfs/
carries `/etc/braun/ov50e40.toml`, `ov50d40.toml` and `ov02f10.toml`: each
camera's role, and the main camera's focus travel through its DW9785. None
has colour calibration yet. Never run the DW9785's firmware download: it
rewrites the motor's eFlash.

## Fingerprint

The stock Goodix ultrasonic stack uses `goodixfp64`; the kernel provides
power, reset, IRQ and `/dev/goodix_fp`. Native PIN setup, fingerprint
enrollment and lock-screen matching have succeeded. Preserve persist's
`goodix/` factory calibration and keep it read-only. Historical power-on
resets and the voltage correction are recorded in the private device vault.

fprintd (Arch's 1.94.5) runs on a libfprint built on the phone with
Phonewave's `uff` driver, which serves this sensor too:

```sh
python3 services/fingerprint/uff/build/snapshot_libfprint.py nx809j
python3 devices/nx809j/tools/fingerprint/build_libfprint.py
python3 devices/nx809j/tools/fingerprint/install_fprintd.py
```

The build needs `meson ninja gcc pkgconf glib2-devel` on the phone and
installs to `/opt/libfprint-uff/<hash>`; the system library stays.
`install_fprintd.py` points fprintd at it with
`/etc/systemd/system/fprintd.service.d/60-nx809j-native.conf`, which also
opens the TEE, `/dev/goodix_fp`, DMA buffers and touch input inside
fprintd's sandbox. The driver runs `/usr/local/libexec/goodix-fprint-helper`
for every operation, which `install_helper.py` installs after
`services/fingerprint/goodix/build.py`:

| Path | What |
|---|---|
| `/usr/local/libexec/goodix-fprint-helper` | Native Rust UFF1 bridge for this phone; no Python in the runtime path |
| `/usr/local/libexec/nx809j-fingerprint/` | Goodix worker, signed app and `goodix-contact` |
| `/etc/nx809j-fingerprint/backend.json` | Their paths and hashes, checked before every operation |
| `/var/lib/nx809j-fingerprint/calibration` | This unit's calibration, copied once from persist mounted read-only |
| `/var/lib/nx809j-fingerprint/templates` | The worker's private store |

The worker runs one-shot per operation (about 0.8 s for a list). For
verification it brings the sensor up and selects the templates while the
lock screen listens, screen on or off, and keeps it asleep until the touch.
After a rejected finger it stays up for the next attempt, up to 5 s, in
`/run/nx809j-fingerprint` (created at boot by `tmpfiles.d`).
Contact is `goodix-contact`: the TCM touchscreen reporting one finger held
on stock's circle, centre (608, 2024) px, radius 120, also with the panel
off; nothing is drawn or lit. A finger lifted before the image is captured
fails the attempt; after it, the comparison goes on without it.

Both Settings apps enroll through deniald's PIN-gated fingerprint session.
The fingerprint marker uses the stock circle above.

The native PIN provider uses `nx809j-gatekeeper.socket` and the per-connection
`nx809j-gatekeeper@.service`. Install the generated `goodix-gatekeeper-worker`
at `/usr/libexec/goodix-gatekeeper-worker`, install the units from rootfs/ and
enable the socket. Socket access is private to `logix`; the worker runs as root
and verifies the board. The Denial PIN record keeps the opaque handle and SID,
never the PIN. The updated sensor worker can request an enrollment HAT from
Denial's one-enrollment root broker without extending fprintd's API. This
integration has host tests and live PIN and fingerprint enrollment results.


The Gatekeeper socket and fprintd require `nx809j-secure-storage.service`.
The default mode uses a private overlay and rejects hardware writes. The user
explicitly authorized this phone's canonical mode: protected secure files in
persist/data and authenticated RPMB writes, with the rollback limitation in the
Careful section. Factory calibration remains read-only. The sensor worker
delegates the shared listener so Gatekeeper can verify an enrollment challenge
while the sensor is active.

The phone's fingerprint runtime uses native executables. The Python scripts in
the development checkout assemble and build artifacts, install them and run
tests; they are not part of the on-device unlock path. The bridge and worker
log unlock phase durations to fprintd's journal.
