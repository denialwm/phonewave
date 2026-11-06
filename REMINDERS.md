# Reminders

Open work, kept until it is done. A reminder says where it was seen; it is
not part of any device's configuration.

- **USB charging current.** On the Motorola Edge 70 the ADSP starts a
  computer's USB port (SDP) at 100 mA, less than the phone draws. A udev rule
  used to raise it to the 500 mA USB 2.0 allows. Fix it in the kernel's
  battery manager driver.
- **Sleep.** The image turns suspend and hibernation off on every phone
  (`image/rootfs/etc/systemd/sleep.conf.d/70-phone-no-sleep.conf`). When
  suspend comes back, the kernel chooses the sleep state (the Edge 70 used
  deep sleep, `mem`), not a device file.
- **Wi-Fi power saving and scan addresses.** The Edge 70 had two
  NetworkManager settings: power saving on (`wifi.powersave=3`) and random
  scan addresses off (`wifi.scan-rand-mac-address=no`). Power saving lowered
  screen-on power by about 0.4 W (0.82 W against 1.23 W); a packet after an
  idle period waited up to about 0.7 s for the next DTIM. Find out whether
  ath11k still needs either, and set what it needs for every phone.
- **OnePlus Ace 3 (aston) onto the shared image.** Before the move, its
  root was saved in the vault, `aston/backups/aston-before-image-20261105/`:
  `system-owners.tar.zst` (/etc, /usr/local, /srv, the service state under
  /var/lib and every file no package owns, owners intact), `files/` (the same
  plus /home, /root and /opt, owners lost), `droidloom-20261105-113151.tar.zst`
  (Android's storage, for `droidloomctl restore`), the package and unit
  lists, and `system.txt` (partition hashes and command line). What
  `devices/aston/rootfs/` and the old root held, and where it goes:
  - Into the boot image's tail: the GPU, Wi-Fi and Bluetooth firmware and
    the voice profile (vault `aston/firmware/tail/`); the stock firmware
    on modem_b (`stock-firmware.conf`, Qualcomm build
    Kailua.LA.1.0.r1-02022-STD.PROD-1; the OnePlus release name is still to
    find); `dmail/device.toml`; the braun calibrations (`braun/`, in
    Phonewave since 2026-11-06, not yet in a built boot image). The modem
    runs from the shared `modem-setup`, rmtfs and tqftpserv; the old EFS
    copy in the backup's /var/lib/aston-modem is no longer used.
  - Into the kernel: the modules the nyx-mainline-battery, -thermal and -wifi
    units load by hand (bq27xxx, adc5-gen3, iio_hwmon, icc-osm-l3, the WCN7850
    chain) should load from the device tree; a tail unit only where they
    cannot.
  - Already in the image: denial.service, the settings polkit rule, the USB
    network, the real-time clamp sysctl, the haptics rule (hapticd's own
    rule takes any built-in force-feedback input), the Settings app,
    coredump storage off, resolved unicast only, msm GPU eviction off, the
    GPU floor and Wi-Fi power-save rules, radio-addr for Wi-Fi and
    Bluetooth, the dmail group, the always-on display switch rule; the
    tail carries `ambient-display.json`.
  - Shared since 2026-11-06: the astonsnd UCM profile (alsa-ucm-phonewave)
    and the image's WirePlumber rule.
  - To find a home: call audio's `asound.conf` and the AGM plugins it loads
    from /opt/nyx-audioreach (in `files/opt`); the libcamera patches in
    `devices/aston/packaging` (by sensor, so one package for every phone).
    The old enrollments in
    /var/lib/aston-fingerprint are in the backup; the fingerprint stack
    itself is the fingerprint-phonewave recipe and the boot image's tail now.
  - Packages the old root had that the image lacks, such as noto-fonts-cjk,
    noto-fonts-emoji, firefox, chromium and fprintd: `pacman-explicit.txt`.
  - Retired with the move: `devices/aston/tools/`, `boot/package_kernel.py`,
    `boot/firstboot/` and the old denial-session.target.
- **RedMagic (nx809j) onto the shared image.** Moved 2026-11-06: sda13,
  `linux` until then, is named `denial` and holds the image; Android keeps
  `userdata`. Before the move the old root was saved in the vault,
  `nx809j/backups/nx809j-before-image-20261106/` (as the Ace 3's, owners
  intact), and Android's storage in `droidloom-20261106/`, which is back on
  the phone with the Ace 3's local Google apps package. The image's fresh
  Android storage is beside it, `/var/lib/droidloom/users/1000/data.fresh-image`.
  What `devices/nx809j/rootfs/` and the old root held, and where it goes:
  - Shared since 2026-11-06, boot image 233-shared: the ADSP, CDSP and
    modem firmware from modem_b; the modem through the shared `modem-setup`,
    rmtfs on its own EFS partitions and tqftpserv (it waits for the SIM
    PIN); the sensors through hexagonrpcd from the stock partitions; the UCM
    profile; the braun calibrations in the tail; Denial's scale from the
    panel's density; the haptics, ambient-display and lockup panic rules,
    sudo for wheel, the dmail group.
  - Shared since 2026-11-06, boot image 233-fingerprint: the Goodix
    Gatekeeper and secure-storage units, in fingerprint-phonewave, turned on
    from the tail; the `gatekeeper` group. Canonical secure storage is on
    again, from its /etc drop-ins (services/fingerprint).
  - Retired with the move: `boot/package.py`, `boot/initramfs/`,
    `nx809j-denial.service`, `nx809j-usb` and its networkd profile,
    `outputs.conf`, `tools/sensor-files`. The phone
    is at 10.77.71.2 over USB like every phone, so two phones on this
    workstation at once collide.
- **Fingerprint on the Ace 3: what is left.** Seen 2026-11-05: the helper
  runs the worker once per request. The worker's resident mode
  (`uff-resident.h`, socket in `/run/phonewave-fingerprint`) has no unit.
- **Fingerprint enrollment on the RedMagic.** Its Gatekeeper keeps the lock
  PIN, and its reader adds a finger only with the PIN proven to it, which
  Denial's root session (`sudo deniald --fingerprint-settings`) does. Once
  its own fingerprint helper asks Gatekeeper itself, Settings drops the sudo
  path and Denial the root session.
- **Internet over USB on this workstation.** The phone resolves names over
  the cable alone (systemd-resolved's fallback servers), and the computer's
  USB connection is NetworkManager's "shared", but the workstation's
  `/etc/nftables.conf` drops every forwarded packet, so nothing past it
  answers. Seen on the Edge 70, 2026-11-05.
- **Google apps on a phone's Droidloom.** The Ace 3's restored Android data
  was set up with Google apps, which the public Droidloom packages leave out,
  so it runs a local `droidloom-gapps` package built from the same LiteGapps
  archive against the installed images (`droidloom-gapps build`, then
  `package --base-package-version`). That package names one Droidloom release
  exactly: every Droidloom update needs it rebuilt for the new release first.
- **Droidloom release name on package installs.** A restore on the packaged
  release 30 said the phone runs release "unknown": the backup's release check
  cannot read a package install's release.
