# gts7-touchd

The touchscreen of the Samsung Galaxy Tab S7 FE (`gts7xllite`). Its FocalTech
FT8203, touch and display driver in one chip, sits on `spi@980000` and has no
kernel driver. gts7-touchd:

- binds `spidev` to that device, and only to that one;
- when the chip sits in its boot ROM (it answers 0xEF), downloads the
  firmware into the chip's RAM as Samsung's `fts_fw_recovery()` does: ROM id
  and boot state checked first, the ECC the ROM computes compared before the
  firmware is started. Nothing is written without the file or when a check
  fails, and nothing goes to flash;
- reads one touch frame per gpio22 edge and feeds them to `/dev/uinput` as a
  protocol B multitouch screen, "gts7xllite touch", 10 slots, 1600x2560;
- when frames turn to EF EF EF (the chip lost its firmware), downloads it
  again.

The firmware, `tsp_focaltech/ft8203_gts7xllite.bin`, is Samsung's and is not
here. Samsung builds it into the stock kernel, so the tablet has no copy once
Perpetua replaced it: the boot image's tail carries it in the running
kernel's `firmware/` (`devices/gts7xllite/boot/build-boot.sh`), and
`/usr/lib/firmware` is the fallback.

`gts7-touchd.service` has no `[Install]`: the gts7xllite tail turns it on.
It starts before Denial and the tty1 login, and holds them back until the
input device exists, at most 20 s. It exits only when it gives up (no SPI
device, an unknown chip); systemd restarts it a few times.

No libc: raw arm64 syscalls, built with clang for any workstation
(`make`). The protocol is Samsung's GPL `ft820x` driver's; GPL-2.0-only.
