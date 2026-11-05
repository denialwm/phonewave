# Reminders

Open work, kept until it is done. A reminder says where it was seen; it is
not part of any device's configuration.

- **pacman warns `unknown key '%INSTALLED_DB%'`.** On the Ace 3, every
  pacman command warns for the 33 packages the image build installed: the
  build runs this workstation's pacman (CachyOS `7.1.0.r9-4`), which writes a
  key into `/var/lib/pacman/local/*/desc` that the phone's Arch Linux ARM
  pacman does not know. Install the image's packages with a pacman that
  writes what the phone's reads.
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
- **Fingerprint (JIIOV).** The Edge 70's optical sensor worked on the 6.6
  kernel stack. The worker, clients and libfprint patches are in
  `services/fingerprint/jiiov/`. The phone-side scripts (build libfprint on
  the phone, install it, optical tests) are in git at
  `9af1cc3:devices/roadstr/tools/fingerprint/`. Bring it up on
  linux-perpetua.
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
    find); `dmail/device.toml`, with dmail's `modem-setup`; the rmtfs and
    tqftpserv units once both are packages (the binaries are in the backup's
    /usr/local/bin; the modem's EFS copy is in the backup's
    /var/lib/aston-modem).
  - Into the kernel: the modules the nyx-mainline-battery, -thermal and -wifi
    units load by hand (bq27xxx, adc5-gen3, iio_hwmon, icc-osm-l3, the WCN7850
    chain) should load from the device tree; a tail unit only where they
    cannot.
  - Already in the image: denial.service, the settings polkit rule, the USB
    network, the real-time clamp sysctl, the haptics rule (hapticd's own
    rule takes any built-in force-feedback input), the Settings app.
  - To decide for every phone: coredump storage off, resolved unicast only,
    msm GPU eviction off, the udev rules that give Denial the GPU floor and
    the ath12k power-save switch, radio-addr for Wi-Fi (the image's package
    has a Bluetooth unit only), the dmail group for the phone's user.
  - To find a home: Denial's `ambient-display.json` and `fingerprint.json`
    and the panel's ambient rule (hardware facts Denial reads from /etc);
    audio, the astonsnd UCM, `asound.conf` and the AGM plugins it loads from
    /opt/nyx-audioreach (in `files/opt`), the WirePlumber rule; camera, the
    braun sensor files and the libcamera patches in `devices/aston/packaging`
    (by sensor, so one package for every phone). The old enrollments in
    /var/lib/aston-fingerprint are in the backup; the fingerprint stack
    itself is the fingerprint-uff recipe and the boot image's tail now.
  - Packages the old root had that the image lacks, such as noto-fonts-cjk,
    noto-fonts-emoji, firefox, chromium and fprintd: `pacman-explicit.txt`.
  - Retired with the move: `devices/aston/tools/`, `boot/package_kernel.py`,
    `boot/firstboot/` and the old denial-session.target.
- **Fingerprint on the Ace 3: what is left.** Seen 2026-11-05, with
  fingerprint-uff installed by hand (`tools/phone-install`):
  - The image does not install fingerprint-uff yet: `image/devices/aston`
    lists the device's own packages from the device's cache, and the recipe
    builds into the shared one.
  - The phone carries `/etc/polkit-1/rules.d/50-denial-fingerprint.rules`
    until a Denial release with the rule reaches it; remove it then.
  - The helper runs the worker once per request. The worker's resident mode
    (`uff-resident.h`, socket in `/run/aston-fingerprint`) has no unit.
- **Fingerprint enrollment on the RedMagic.** Its Gatekeeper keeps the lock
  PIN, and its reader adds a finger only with the PIN proven to it, which
  Denial's root session (`sudo deniald --fingerprint-settings`) does. Once
  its own fingerprint helper asks Gatekeeper itself, Settings drops the sudo
  path and Denial the root session.
- **Internet over USB.** The image routes through the computer on the cable
  (10.77.71.1) but gives the phone no DNS server, so until Wi-Fi is set up
  pacman cannot reach anything. Seen on the Ace 3, 2026-11-05.
- **Google apps on a phone's Droidloom.** The Ace 3's restored Android data
  was set up with Google apps, which the public Droidloom packages leave out,
  so it runs a local `droidloom-gapps` package built from the same LiteGapps
  archive against the installed images (`droidloom-gapps build`, then
  `package --base-package-version`). That package names one Droidloom release
  exactly: every Droidloom update needs it rebuilt for the new release first.
- **Droidloom release name on package installs.** A restore on the packaged
  release 30 said the phone runs release "unknown": the backup's release check
  cannot read a package install's release.
