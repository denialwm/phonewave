# Device images

Working notes for installing a phone from images, without USB sessions to
partition, unpack and copy files by hand. Being tried on the Motorola Edge 70
(`roadstr`) first.

## What gets flashed

| Image | Built for | Holds |
|---|---|---|
| `boot.img` | one device | that device's perpetua kernel with its modules |
| `init_boot.img` | every phone | the early userspace (`image/init-boot`) |
| `userdata.img` | one selection | the root filesystem: Arch Linux ARM, Denial, the services |

Only the active slot's `boot` and `init_boot`, and `userdata` or `denial`. Never the
partition table, never an `_a` partition, never a slot command.

All through fastboot: the bootloader takes `boot` and `init_boot`, and the
root goes in from the init_boot's own fastboot, which takes `boot` and
`init_boot` too, on phones whose bootloader refuses to write them.

## Installing

The early userspace boots the first of the partitions `denial` and
`userdata` that holds a Phonewave root, which phonewave-system marks with
`/usr/lib/phonewave-system/root`. `denial` comes first: a user who made one
beside Android's `userdata` keeps Android's data. A phone fresh from Android
has no root on either, so it stops in rescue, and the rescue serves fastboot
(`fastboot/fastboot.c`). The screen, drawn on the framebuffer the bootloader
left lit, says why the phone stopped and what to run:

    fastboot flash userdata userdata.img
    fastboot flash denial denial.img

This fastboot writes only those two partitions and the `boot` and
`init_boot` of the slot the bootloader started (`androidboot.slot_suffix`,
never the other slot's), raw or sparse, and reboots;
`getvar denial-install` answers `yes` only here, which is how the flash
script knows the bootloader handed over. Every rescue serves it, whatever
stopped the boot, beside SSH at `10.77.71.2`. Over an earlier install the
phone boots that root instead: `install-userdata` asks it for the rescue,
which a root holding `/etc/phone-rescue` gets.

The init_boot is the same on every phone. The early userspace takes the
phone's codename from the device tree's first compatible, its name from the
model, and its USB addresses from the codename. `image/build` builds it into
each image; built with `--dev-access`, the rescue's SSH lets in root's keys
from every phone's vault, and without, nobody. The rescue draws its screen
only where the device tree describes the bootloader's framebuffer.

## The builder

You pick a distro (deferred: Arch Linux ARM only for now) and whether
Droidloom is in; the image is the same for every phone. The builder installs
exactly those packages into an empty root and packs the root and the shared
init_boot, plus a flash script. Each phone's boot image comes from its own
`devices/<codename>/boot/`.

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
`rootfs/usr/lib/systemd/system-preset/50-phonewave.preset`, and those of each
user's session, such as callaudio, in `user-preset/50-phonewave.preset`. When
a package first installs one, phonewave-system's pacman hook enables it, and
starts a system service on a running phone: the image's install turns them on, and so does the
update that brings a new one. Updates leave units alone after that, so what
the user turns off stays off. A device turns on the services its hardware
has from its boot image's tail.

Phones update from two signed repositories at `packages.denialwm.org`:
`[denial]` (`arch/$arch`), Denial's releases, and `[denial-phone]`
(`phone/$arch`), every package `recipes/` builds. Denial's
`tools/denial-builder phone-packages` builds `[denial-phone]` from Phonewave,
denial-apps and gpui-touch `main` on the package builder, signs it with the
Denial key and publishes it. A recipe whose versions are all published
already is downloaded, not built. The image itself installs the packages
`image/pack` built here, so a test image needs no publish, and
`tools/phone-install HOST RECIPE...` puts a recipe's packages on a running
phone.

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

Droidloom, in an image built with `--droidloom` or installed later from its
repository, is set up for the phone's user (the one `denial.service` runs
as) and starts with their session, as the Settings switch turns it on:
`phonewave-system`'s `droidloom-owner`, from a pacman hook when Droidloom is
installed and at boot until it is set up. Its setup reads the phone's GPU,
so it never runs inside the image build. Once set up it leaves Droidloom
alone, so Android the user turned off stays off.

## USB

Every phone is `10.77.71.2` on its cable and hands the computer `10.77.71.1`
by DHCP, so any computer reaches it with nothing to set up; SSH listens there
and on Wi-Fi. Several phones on one computer need a pair each: a lab phone's
vault `private-rootfs/etc/` carries its own `systemd/network/10-phone-usb.network`
and `ssh/sshd_config.d/11-phone-usb.conf`, which `--dev-access` puts over the
image's. The rescue is always at `10.77.71.2`.
Tools that write to a phone without checking its host key, such as
`install-userdata`, first read its device tree's model and stop on another
phone.

The phone routes through the computer when the computer shares its
connection (NetworkManager's "shared" mode, say), below Wi-Fi and mobile
data. Names resolve through systemd-resolved: NetworkManager gives it Wi-Fi's
servers and dmail the carrier's, and with the cable alone, where no
connection names one, its fallback servers answer.

## The first user

Setup is a mode of the shell: before a person's account exists, Denial shows
its setup instead of the lock screen. No "configured" flag.

- Without `--dev-access`, or with `--setup`, the image's `denial.service`
  drop-in runs Denial as the system account `denial-setup`
  (`sysusers.d/phonewave-setup.conf`, its home `/var/lib/denial-setup`) and
  names the helper in `DENIAL_FIRST_USER_HELPER`, which turns setup on.
- Setup asks the language, Wi-Fi, time zone, name and username, password,
  a lock PIN (skippable) and the phone's name. Wi-Fi joined there is saved
  for the whole phone.
- `phonewave-system/first-user`, through pkexec, makes the account in one
  step, password included, and refuses once a person's account exists. It
  sets the PIN as Settings does, the phone's name, time zone and language,
  and names the account in `denial.service.d/user.conf`.
- With a PIN and a fingerprint reader, setup then offers a fingerprint
  (skippable): `first-user --fingerprint` runs deniald's fingerprint session
  for the new account, opened by its PIN.
- `first-user --start` restarts Denial as the new account, on its home
  screen. Droidloom, where installed, is set up for it. An account left
  without `--start` (setup stopped halfway) starts the same way when setup
  runs again.
- The account joins `wheel`, `audio`, `input`, `render`, `video`, `dmail`
  and `gatekeeper`, as `--dev-access` does for `logix`.
- `47-phone-setup.rules` lets `denial-setup`, at the phone, run the helper
  and change NetworkManager's connections; nothing else.
- `denial.service` is the phone session, the same on every device. It starts
  `denial-session`, which finds the screen's GPU; no device adds anything to
  it.
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
- radio-addr gives a built-in Wi-Fi radio and a Bluetooth controller the
  phone's own addresses, from the SoC serial, where the device tree gives
  none.
- Always-on display: a phone whose panel can hold a picture while off says
  how in its tail, `denial/ambient-display.json`, which phonewave-system
  copies to `/etc/denial` at boot; the panel driver's `ambient` switch is the
  video group's. Without the file Denial has no always-on display.
- Denial may turn Wi-Fi power save off while a screen is lit
  (`power_save_allowed`, the seat group's).
- The msm driver keeps GPU buffers out of swap; a hung kernel reboots after
  10 s; core dumps are logged, not stored; wheel uses sudo; the phone's user
  is in the `dmail` group, and in `gatekeeper`, which reaches a hardware PIN
  provider and Denial's fingerprint session where the phone has one.
- The home screen starts with the clock and the image's apps
  (`rootfs/etc/skel/.config/denia-home/layout.json`).
- Denial may raise the Adreno's minimum clock while the user touches the
  screen (`70-phone-session.rules`).
- The lab tools in `../tools/` take the phone's SSH host.

## Trying it

Build, flash, look, change, rebuild. The loop has to be quick.

    image/build --droidloom --dev-access --setup
    /mnt/development/phonewave-cache/image/out/flash.sh   # phone in the rescue

To try the streams' heads on a phone that runs an image, with no hands on
it (about 7 minutes): `image/pack` brings every recipe to its commit, then

    image/build --droidloom --local-builds --dev-access --setup
    image/install-userdata HOST

`--local-builds` installs the newest `tools/build` Denial and Droidloom
(with droidloom-gapps when that build packed it) in place of the published
releases. `install-userdata` sends the phone to its rescue, writes the root,
puts the old root's SSH host keys and its canonical secure storage
drop-ins (`/etc/systemd/system/*.d/80-canonical.conf`, see
`services/fingerprint`) in the new one and reboots; the phone starts in
setup. Without those drop-ins a Goodix phone's Gatekeeper cannot keep a PIN.

To try setup again on a phone that already runs the image, write nothing:
`tools/phone-setup-again HOST [PACKAGE...]` installs the given builds (say
`builds/<codename>/denial/latest/*.pkg.tar.zst`), removes the account setup
made with its fingers and Droidloom, and starts setup again in seconds
(`usr/lib/phonewave-system/setup-session`, which the image build runs too).
A new image is for changes to what the image holds.

One image for every phone: there is no per-device build. A phone's boot
image is its own (`../devices/<codename>/`); `flash.sh` writes only the root,
on `denial` where the phone has one, else on `userdata`, as the early
userspace boots it. For a quicker try on a phone that already runs an image,
`image/install-userdata HOST` writes the root the same way, over SSH from
the initramfs rescue, and keeps the phone's SSH host keys and secure
storage choice.

- `packages/base`: what every image gets. `rootfs/`: the `phonewave-system`
  package's files; `image/pack phonewave-system` rebuilds it.
- Output and caches live in `/mnt/development/phonewave-cache`
  (`PHONEWAVE_CACHE`), never in home; the image in `image/out/`.
- No root needed: the build runs in a user namespace, with the workstation's
  `/dev` and `/sys` out of reach of the install scripts. QEMU comes from
  Arch's `qemu-user-static`, checked against Arch's keyring; Arch Linux ARM's
  keyring is trusted only through its pinned build key.
- About two minutes once packages are cached. `--dev-access` puts in root's
  SSH keys from every phone's vault, the vault's `image/private-rootfs/`
  (the Wi-Fi network), the owner's wallpaper (`image/wallpaper/`) and the
  user `logix`; with `--setup` no account, and the phone's setup makes one.
  Such an image never leaves this workstation. A phone's own SSH host keys
  stay in its vault: the image makes new ones on first boot.

## Until Denial catches up

- `noextract` keeps Denial's desktop Settings and Welcome off the phone, and
  `rootfs/etc/skel/.config/denial/welcome` keeps Welcome from autostarting.
  Both go once Settings is a package of its own.
- `denial.service` sets `DENIAL_CPU_PLACEMENT=1`, `DENIAL_UI_CPU_MIN=512` and
  `DENIAL_RENDER_AHEAD=1`, the mobile shell's defaults in Denial's `dev`. They
  go once the denial package the image installs has them.
- `rootfs/etc/skel/.config/denial/settings.json` is the document a mobile
  deniald creates on first start, the mobile look included, with the owner's
  lock screen: wallpaper unblurred. It goes once the
  denial package the image installs seeds that look itself.
