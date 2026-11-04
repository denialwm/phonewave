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
- **Fingerprint (JIIOV).** The Edge 70's optical sensor worked on the 6.6
  kernel stack. The worker, clients and libfprint patches are in
  `services/fingerprint/jiiov/`. The phone-side scripts (build libfprint on
  the phone, install it, optical tests) are in git at
  `9af1cc3:devices/roadstr/tools/fingerprint/`. Bring it up on
  linux-perpetua.
