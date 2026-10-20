# rtc-offset

Keeps the system clock across reboots on devices whose real-time clock keeps
counting but cannot be set from Linux.

Qualcomm phones and tablets are the usual case. The PMIC RTC runs from the
battery while the device is off, but its count starts from an arbitrary base,
and unless the device tree grants `allow-set-time` or an nvmem offset cell,
`hwclock --systohc` cannot correct it. Without a correct RTC, every boot
starts at the time systemd-timesyncd saved at the last shutdown and stays
there until NTP answers.

rtc-offset stores the difference between the synchronized system clock and
the RTC count, and adds it back at boot. It is the same approach Android and
postmarketOS's swclock-offset use.

## How it works

| Unit | When | Does |
|---|---|---|
| `rtc-offset.service` | early boot, before systemd-timesyncd | `rtc-offset restore`: sets the clock to RTC count plus saved offset |
| `rtc-offset-sync.path` | first NTP synchronization of a boot | starts `rtc-offset-sync.service`, which runs `rtc-offset save` |
| `rtc-offset.service` stop | shutdown | `rtc-offset save` again |

`save` writes only when `/run/systemd/timesync/synchronized` exists, so an
unsynchronized boot never replaces a good offset. `restore` only moves the
clock forward. If the RTC restarted (a flat battery), RTC plus offset lands
behind the clock, and `restore` leaves the clock for NTP to correct.

The offset lives in `/var/lib/rtc-offset/offset`, in seconds. The RTC is
`/sys/class/rtc/rtc0`; set `RTC_OFFSET_DEVICE` in a unit drop-in for another.

## Install

```sh
make check
sudo make install
```

Enable it on devices that need it. Phonewave device trees do this under
`devices/<codename>/rootfs/`; elsewhere:

```sh
sudo systemctl enable rtc-offset.service rtc-offset-sync.path
```
