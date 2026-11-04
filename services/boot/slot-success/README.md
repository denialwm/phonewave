# slot-success

Marks the Linux slot of an A/B Qualcomm phone or tablet as booted
successfully once the boot has proved itself.

The bootloader keeps a retry count and a successful flag for each slot, in
the GPT attributes of its boot partition. Each boot of a slot that is not
marked successful uses one retry. When none are left, the bootloader marks
the slot unbootable and boots the other one. Android marks its slot through
the boot control HAL once it has started; a Linux slot needs the same, or it
falls back to Android after a few boots. A fastboot flash or
`fastboot set_active` resets the count and clears the flag, so every new
kernel proves itself again.

## How it works

`slot-success.service` runs `slot-success mark` on every boot, after
multi-user.target. mark:

1. leaves the slots alone when the running slot is not the configured one,
   or when the slot is already marked;
2. waits until the system has been up for the settle time and systemd
   reports it running: every unit started and none failed. It waits at most
   five more minutes;
3. checks that each configured unit is active;
4. marks the slot with qbootctl and reads the mark back from the GPT.

The running slot is `androidboot.slot_suffix` from `/proc/bootconfig` or
`/proc/cmdline`. A kernel that gets neither, such as one with a built-in
command line, takes the active slot.

qbootctl is linux-msm's port of Qualcomm's boot control HAL, built static
and unmodified from the commit in the Makefile. Marking sets the successful
bit on the slot's entries in both GPT copies of every disk that holds them,
and updates both copies' checksums. slot-success only ever marks; it never
changes the active slot.

`slot-success status` prints the slot's state and exits 0 only when another
boot costs no retry. `slot-success running` prints the running slot.

## Configuration

`/etc/slot-success.conf`. Without it the service does not start.

```sh
# Linux boots from slot b; slot a keeps Android and is never marked.
slot=b
# A boot counts once Denial and SSH run and the system is up a minute.
units=denial.service sshd.service
settle=60
```

| Key | Value |
| --- | --- |
| `slot` | `a` or `b`, the Linux slot |
| `units` | units that must be active, separated by spaces |
| `settle` | seconds of uptime before a boot can count, 60 if unset |

## Build and install

Needs git and a C compiler with the Linux UAPI headers (`linux/bsg.h`); on
the device, systemd and util-linux (sfdisk, lsblk).

```sh
make CC=aarch64-linux-gnu-gcc
make check
sudo make install
```

`make check` runs shellcheck and the tests. Installing turns the service
on: it has no `[Install]` section, and does nothing until the
configuration exists.

## Devices

Each Phonewave device carries its configuration: pudding in
`devices/pudding/rootfs/etc/slot-success.conf`; devices built with
`image/`, roadstr among them, in the phonewave-system package
(`image/rootfs/etc/slot-success.conf`).

## License

slot-success is MIT. qbootctl is GPL-3.0-or-later; its license is
installed beside slot-success's.
