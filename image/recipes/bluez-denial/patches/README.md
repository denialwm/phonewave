# BlueZ patches

Patches the phones need on top of the BlueZ release Arch ships. The recipe
applies the ones `_patches` in `../PKGBUILD` lists, in that order. To change
the set, edit the list, raise `pkgrel` and run `image/pack bluez-denial`.

- `hog-read-report-maps-longer-than-512.patch`: BlueZ cuts every long GATT
  read at 512 octets, the GATT limit, Report Maps included, but the HID
  specification sets no such limit. The OnePlus Pad 3 Keyboard (22d9:3869)
  serves a 567-octet map (keyboard, consumer keys, a 5-finger touchpad) and
  got no input device over Bluetooth: `item fetching failed at offset
  511/512`. Reads the Report Map up to 4096 octets, the UHID limit. Not
  upstream yet (BlueZ master still truncates, 2026-11-15); AOSP has the same
  limit and works around it for one keyboard only (INTEROP_HOGP_LONG_REPORT).
