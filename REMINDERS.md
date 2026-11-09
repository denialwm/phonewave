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
  - The kernel loads the modules the old nyx-mainline-battery, -thermal and
    -wifi units loaded by hand (bq27xxx, adc5-gen3, iio_hwmon, icc-osm-l3,
    the WCN7850 chain) from the device tree: each has a node and a match
    table. The units are gone; the battery, temperatures and Wi-Fi are to
    check on the phone once it is reachable again.
  - Already in the image: denial.service, the settings polkit rule, the USB
    network, the real-time clamp sysctl, the haptics rule (hapticd's own
    rule takes any built-in force-feedback input), the Settings app,
    coredump storage off, resolved unicast only, msm GPU eviction off, the
    GPU floor and Wi-Fi power-save rules, radio-addr for Wi-Fi and
    Bluetooth, the dmail group, the always-on display switch rule; the
    tail carries `ambient-display.json`.
  - Shared since 2026-11-06: the astonsnd UCM profile (alsa-ucm-phonewave)
    and the image's WirePlumber rule.
  - Retired: `asound.conf` and the AGM plugins it loaded from
    /opt/nyx-audioreach (in `files/opt`), since calls run on the UCM
    "Voice Call" verb and callaudio, which every image turns on; the
    libcamera package and its sensor patches, since braun replaced libcamera
    and the tail's `braun/` carries the stock colour tuning. The old
    enrollments in
    /var/lib/aston-fingerprint are in the backup; the fingerprint stack
    itself is the fingerprint-phonewave recipe and the boot image's tail now.
  - Packages the old root had that the image lacks, such as noto-fonts-cjk,
    noto-fonts-emoji, firefox, chromium and fprintd: `pacman-explicit.txt`.
  - Retired with the move: `devices/aston/tools/`, `rootfs/`,
    `boot/package_kernel.py`, `boot/firstboot/`, callaudio's
    `build-aston.sh` and the old denial-session.target.
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
- **Pad 4 (iceland) onto the shared image.** Moved 2026-11-06, on
  `userdata` as before. The old root was too big to copy off (Android's
  storage is 108 GB), so the image was unpacked beside it on the same
  filesystem and the rescue swapped the two: the old root is on the tablet in
  `/old-root-20261106/`, with `var/tmp/old-root-explicit-packages.txt` in
  the new root listing its packages; home and Android's storage were moved
  back in place. What `devices/iceland/` and the old root held, and where it
  goes:
  - Into the boot image's tail: the GPU, touch, Wi-Fi, Bluetooth and Peri
    firmware and the audio topology (vault `iceland/firmware/tail/`); the
    ADSP firmware from modem_b (`stock-firmware.conf`), the same images the
    old root carried as `adsp-runtime.mbn`; rtc-offset turned on.
  - Into the kernel, devices-235: `rdinit=/init`, the Bluetooth driver
    options of `pad4-peri-probe.conf`, the ADSP firmware path and the
    90 °C GPU trips of `pad4-gpu-thermal`. The ADSP now starts when its
    driver probes, not after Denial as `pad4-audio-init` did; the reason
    for that order was not recorded.
  - Loaded by the kernel without help: the modules `pad4-hardware-init`,
    `pad4-display-init` and `pad4-wifi-init` loaded by hand.
  - Already in the image: zram, msm GPU eviction off, the USB network
    (10.77.71.2 instead of 192.168.77.2), Denial as the `denial` package
    instead of `/opt/denial` releases, the portal and polkit agent units.
  - Not carried over: `pad4-vm-tuning` (shmem huge pages `within_size`,
    defrag `never`), `pad4-supervooc` (its module option is gone; the kernel
    regulates SuperVOOC), the `IT` regulatory domain (see Wi-Fi country),
    `pad4-fonts.conf` and `pad4-flash`.
  - Retired with the move: `rootfs/`, `tools/` (deploy, audit, health,
    rescue, Denial release and recording tools), `boot/package_pad4_devices.py`
    and the USB rescue it packed into init_boot.
- **Fingerprint on the Ace 3: what is left.** Seen 2026-11-05: the helper
  runs the worker once per request. The worker's resident mode
  (`uff-resident.h`, socket in `/run/phonewave-fingerprint`) has no unit.
- **Fingerprint enrollment on the RedMagic.** Its Gatekeeper keeps the lock
  PIN, and its reader adds a finger only with the PIN proven to it, which
  Denial's root session (`sudo deniald --fingerprint-settings`) does. Once
  its own fingerprint helper asks Gatekeeper itself, Settings drops the sudo
  path and Denial the root session.
- **Wi-Fi country.** Settings sets it since 2026-11-06 (denial-apps
  `settings`: Date and time, Country or region), into wireless-regdb's
  `/etc/conf.d/wireless-regdom`; a time zone picked while none is set brings
  its country. A phone where nobody picked one stays on the world profile
  (`iw reg get`: country 00). ath11k and ath12k chips keep their own rules
  and take the user's country only with ATH_REG_DYNAMIC_USER_REG_HINTS, on
  in the mobile kernel fragment since 2026-11-06 (devices-236 on the Pad 4,
  which then reported IT). Other phones get the option with their next
  kernel build. Still open: phones with a modem could take the country from
  the network code.
- **Pad 4 Wi-Fi on one MAC.** With DBS the WCN7860 firmware splits each
  scan between its two MACs and gives the 2.4 GHz one 5180–5320 MHz, where
  on the Pad 4 it hears nothing (seen 2026-11-06 with ath12k MGMT debug: no
  frames on those channels; 5500 MHz and up came in). Networks there were
  in no scan, so the Pad 4 never joined its 5180 MHz network. Since
  devices-243 the Pad 4's Wi-Fi node has `qcom,no-dbs` and the chip runs on
  one MAC; it joins at boot. Clearing only the concurrent scan bits kept the
  split. Why that MAC hears nothing is unknown: stock Android leaves DBS on
  with the same firmware, through its own board file (`bdwlan.elf`) where
  ours is a board.bin matching no stock file, though the stock `bdwlan.elf`
  alone changed nothing. Pudding and the RedMagic have the same chip; whether
  their scans miss those channels is unchecked.
- **Pad 4 home screen: native app icons do nothing.** Seen 2026-11-06 on
  Denial 0.6.2: tapping or holding Calls, Camera, Messages, Sensors or
  Settings on the home grid does nothing, while Droidloom icons work and the
  same apps open from the desktop launcher. No launch request reached
  deniald. The home layout came from the old root (`~/.config/denia-home/
  layout.json`), the native apps last at slots 59–63. Cause unknown.
- **Pad 4 taskbar is a hand-built shell.** Denial 0.6.2's packaged shell
  has no taskbar and phones get no plugin manager, so the tablet runs the
  0.6.2 shell with `denial_taskbar` in place of the top bar, compiled here
  against the same ARM64 engine and activated with `denialctl ui activate`
  (`~/.local/share/denial/plugin-candidates/iceland-taskbar-0.6.2/bundle`).
  The next Denial release rejects it (source mismatch) and falls back to the
  packaged shell: phones need a way to compose plugins.
- **First-start setup: what is left.** Built 2026-11-06: Denial `dev`
  010c10e (setup in the shell), Phonewave `first-user`, the `denial-setup`
  account and its polkit rule. First real run 2026-11-07 on the Moto, an
  image built with `--dev-access --setup` and Denial from `tools/build`
  (0.2.1.r341.g010c10ef): the user went through setup ("it worked, quite
  nice for a first try"); the account, the device's name, time zone and the
  Wi-Fi joined in setup were all there after. Still open:
  - the published `denial` package has no setup until a release carries
    010c10e: an image without `--dev-access` built before then starts Denial
    as `denial-setup` with no way to make an account;
  - setup offers no PIN or fingerprint yet; Settings sets both after;
  - handing Droidloom to the new account is untried: the Moto image had no
    Droidloom;
  - the Pad 4 keeps `denial-setup`, its home and the test shell in
    `/var/lib/denial-setup`; nothing runs as it.
- **The rescue's screen on command-mode panels.** Seen 2026-11-07 on the
  RedMagic: the rescue drew its install screen into the bootloader's
  framebuffer, but the RM692H5 is a command-mode panel that shows only what
  the display controller sends, and the kernel turned the DSI clocks off as
  unused at 0.67 s; the panel went white. Installing works without the
  screen (flash.sh asks fastboot). The rescue should draw through the
  display driver (KMS) where the phone has one, on every phone.
- **USB gadget after an external monitor on the RedMagic.** After a
  monitor has been connected over USB-C, USB does not work until the phone
  reboots; unplugging and replugging the cable alone is fine (user,
  2026-11-09). Seen 2026-11-07: binding the gadget again failed,
  `cdc_ethernet bind returned -19` and `failed to start phone: -19`; the
  computer saw no device until the phone rebooted, which bound it at once.
- **Internet over USB on this workstation.** The phone resolves names over
  the cable alone (systemd-resolved's fallback servers), and the computer's
  USB connection is NetworkManager's "shared", but the workstation's
  `/etc/nftables.conf` drops every forwarded packet, so nothing past it
  answers. Seen on the Edge 70, 2026-11-05.
- **Google apps on a phone's Droidloom.** The Ace 3's restored Android data
  was set up with Google apps, which the public Droidloom packages leave out,
  so it runs a local `droidloom-gapps` package built from the same LiteGapps
  archive (the add-on in `droidloom/.work/gapps-arm64`). That package names one
  Droidloom release exactly. `tools/build droidloom` packs it for each test
  build; a published release needs it packed for that release first
  (`droidloom-gapps package --base-package-version`).
- **Droidloom release name on package installs.** A restore on the packaged
  release 30 said the phone runs release "unknown": the backup's release check
  cannot read a package install's release.
