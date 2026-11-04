# Device images

Working notes for installing a phone from images, without USB sessions to
partition, unpack and copy files by hand. Being tried on the Motorola Edge 70
(`roadstr`) first.

## What gets flashed

| Image | Built for | Holds |
|---|---|---|
| `boot.img`, `init_boot.img` | one device | that device's perpetua kernel with its modules, and its initramfs |
| `userdata.img` | one selection | the root filesystem: Arch Linux ARM, Denial, the services |

Only the active slot's `boot` and `init_boot`, and `userdata`. Never the
partition table, never an `_a` partition, never a slot command.

All through fastboot. Where the bootloader refuses `userdata` (the Edge 70),
everything goes in from fastbootd instead, which runs from the slot's stock
`boot` and `init_boot`. A phone fresh from Android has them; over an earlier
install the flash script first writes them back from the stock firmware
folder it is given.

## The builder

You pick a device, a distro (deferred: Arch Linux ARM only for now) and
whether Droidloom is in. The builder installs exactly those packages into an
empty root and packs the three images, plus a flash script.

Building chooses packages; it compiles nothing. Package install scripts and
pacman hooks are ARM programs, so the x86 build host runs them through QEMU.

Every file the image adds comes from a package, so `pacman -Syu` brings an
installed phone what a new image has. The phone's own system files are the
`phonewave-system` package (`recipes/phonewave-system`), whose files are
`rootfs/` at the paths they install to: `/usr/lib` where the program reads
it, so an override in `/etc` wins; `/etc` only where the program reads
nothing else, and there pacman keeps the user's edits. The build itself
writes only `fstab`, the partition layout, which every install owns.

The services every phone runs are listed in
`rootfs/usr/lib/systemd/system-preset/50-phonewave.preset`. When a package
first installs one, phonewave-system's pacman hook enables it, and starts it
on a running phone: the image's install turns them on, and so does the
update that brings a new one. Updates leave units alone after that, so what
the user turns off stays off. A device turns on the services its hardware
has from its boot image's tail.

Phones update from two signed repositories at `packages.denialwm.org`:
`[denial]` (`arch/$arch`), Denial's releases, and `[denial-phone]`
(`phone/$arch`), every package `recipes/` builds. Denial's
`tools/denial-builder phone-packages` builds `[denial-phone]` from Phonewave,
denial-apps and gpui-touch `main` on the package builder, signs it with the
Denial key and publishes it. The image itself installs the packages
`image/pack` built here, so a test image needs no publish.

## The kernel's modules

`boot.img` holds the kernel and, at the page after it, a read-only erofs
filesystem that `boot-tail` packs: the kernel's modules, the firmware that
overrides linux-firmware's, the units that belong to the kernel, such as
workarounds for its open issues, the services its hardware has, turned on by
links in `systemd/system/<target>.wants/`, and what services read about the
hardware, such as dmail's `dmail/device.toml`. The bootloader loads only the kernel the
header lists. The early userspace mounts the rest in place, from the
partition, at `/usr/lib/modules` (`fsoffset=` the kernel size, rounded up,
plus the header page), points `firmware_class.path` at its `firmware/` and
hands its `systemd/system/` to systemd through `/run`. Nothing is copied;
the image holds an empty `/usr/lib/modules`.

The kernel, its modules and their firmware are one partition write, so they
never disagree. Writing a new `boot.img` changes the files under the mounted
filesystem: reboot right after.

## Firmware

Vendor firmware is not in the image. The kernel loads it from the phone's
stock partitions, read-only, where Android keeps it. Getting the stock
release onto the phone is the user's part; we name the release we tested.

A device whose kernel needs it records it in its boot tail
(`devices/<codename>/boot/tail/`):

- `stock-firmware.conf`: the tested release, the build ID the phone shows
  under About phone and in the name of the vendor's stock package; the
  partition that holds it, without the slot suffix; its filesystem and
  mount options; the folder in it; where the kernel looks for it, under the
  tail's `firmware/`; the files without which the phone cannot come up.
- `stock-firmware.sha256`: the hashes of that release's files. The signed
  headers (`.mdt`) carry the hashes of their segments, so a few small files
  pin the whole set.

`boot-tail` packs `stock-firmware` with them. The early userspace runs it
before systemd: it mounts the booted slot's partition and binds the folder
where the kernel looks, then compares:

- the tested release: it says so, and the phone boots.
- other firmware: it prints which release this kernel was tested with, and
  the phone boots.
- a required file missing, or no partition: it prints which stock release
  to flash, and the phone stays in rescue (SSH over USB).

The messages go to the boot console, on the screen and in the kernel log.

## First boot

Unattended, before Denial, from systemd's own tools where they exist:

- grow the filesystem to the whole partition (`x-systemd.growfs`)
- machine-id: the image ships `/etc/machine-id` empty
- SSH host keys and the pacman keyring, once, on the phone
- clock floor: `/usr/lib/clock-epoch` carries the image's build date

Nothing in the image belongs to one phone: no keys, no machine-id, no user.

## USB

Every phone is `10.77.71.2` on its cable and hands the computer `10.77.71.1`
by DHCP, so any computer reaches it with nothing to set up; SSH listens there
and on Wi-Fi. Several phones on one computer need a pair each: a lab phone's
vault `private-rootfs/etc/` carries its own `systemd/network/10-phone-usb.network`
and `ssh/sshd_config.d/11-phone-usb.conf`, which `--dev-access` puts over the
image's. Its rescue init_boot is built per device with the same address.

## The first user

Setup is a mode of the shell: when no person's account exists, Denial shows
the welcome flow instead of the lock screen. No "configured" flag.

- Before an account exists, Denial runs as the system user `denial-setup`.
- Setup asks language, Wi-Fi, name and username, password, then offers a PIN
  and fingerprint, and the phone's name.
- A root helper creates the account in one step, password included, and
  refuses once a person's account exists.
- `denial.service` is the phone session, the same on every device. The
  helper names the account in its `denial.service.d/user.conf` drop-in and
  restarts it. It starts `denial-session`, which finds the screen's GPU; no
  device adds anything to it.
- SSH accepts keys only.

## Every phone

- Mesa main as `mesa-denial`, cross-built by `recipes/mesa-denial`: no Arch
  Linux ARM Mesa has the Adreno 722 yet. It provides and conflicts with Arch's
  Mesa packages, so no Arch Mesa replaces it, and only an image that lists it
  gets it. The recipe says how it retires once Arch's Mesa runs the phones.
  `recipes/mesa-denial/patches/` holds patches for GPUs main does not run
  right yet; the recipe applies only those it lists.
- No sleep: suspend and hibernation stay off until the kernels resume reliably.
- slot-success marks the Linux slot successful once a boot has run a minute
  with Denial and SSH up.
- hapticd finds a built-in vibrator by its bus; PipeWire adds a silent output
  while no sound card exists.
- The home screen starts with the clock and the image's apps
  (`rootfs/etc/skel/.config/denia-home/layout.json`).
- Denial may raise the Adreno's minimum clock while the user touches the
  screen (`70-phone-session.rules`).
- The lab tools in `../tools/` take the phone's SSH host.

## Trying it

Build, flash, look, change, rebuild. The loop has to be quick.

    image/build roadstr --dev-access
    /mnt/development/phonewave-cache/roadstr/image/flash.sh STOCK_DIR   # phone in fastboot

For a quicker try on a phone that already runs an image, `image/install-userdata
roadstr` writes only `userdata`, over SSH from the initramfs rescue.

- `packages/base`: what every image gets. `devices/<codename>`: the device's
  packages and boot images. `rootfs/`: the `phonewave-system`
  package's files; `image/pack phonewave-system` rebuilds it.
- Output and caches live in `/mnt/development/phonewave-cache`
  (`PHONEWAVE_CACHE`), never in home. The device's packages come from its
  `<codename>/packages/`.
- No root needed: the build runs in a user namespace, with the workstation's
  `/dev` and `/sys` out of reach of the install scripts. QEMU comes from
  Arch's `qemu-user-static`, checked against Arch's keyring; Arch Linux ARM's
  keyring is trusted only through its pinned build key.
- About two minutes once packages are cached. `--dev-access` puts root's SSH
  key and the user `logix` in; such an image never leaves this workstation.

## Until Denial catches up

- `noextract` keeps Denial's desktop Settings and Welcome off the phone, and
  `rootfs/etc/skel/.config/denial/welcome` keeps Welcome from autostarting.
  Both go once Settings is a package of its own.
- `denial.service` sets `DENIAL_CPU_PLACEMENT=1`, `DENIAL_UI_CPU_MIN=512` and
  `DENIAL_RENDER_AHEAD=1`, the mobile shell's defaults in Denial's `dev`. They
  go once the denial package the image installs has them.
- `rootfs/etc/skel/.config/denial/settings.json` is the document a mobile
  deniald creates on first start, the mobile look included. It goes once the
  denial package the image installs seeds that look itself.
