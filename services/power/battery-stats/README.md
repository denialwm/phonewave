# battery-stats

Where the battery goes. `battery-statsd` books, for each hour, each state of
the phone and each build, how long the phone spent there, the energy it took
and what the hardware did meanwhile. `battery-stats` prints it.

The states:

| State | When |
|---|---|
| screen on | a connector is on and the backlight lit |
| screen off | awake with the screen dark |
| asleep | suspended |
| charging | a charger is online, or the battery charges or is full |

## How it counts

Every 10 seconds while the phone is awake, `battery-statsd` reads the battery's
`current_now` and `voltage_now` and adds the energy since the last reading.
It never wakes the phone: a sleep shows afterwards as the difference between
`CLOCK_BOOTTIME` and `CLOCK_MONOTONIC`. The energy of a sleep is the drop in
the gauge's own count (`energy_now`, else `charge_counter` or `charge_now`),
less the moment awake either side at the rate before it. Every bucket also
carries the gauge's count over its own time, so the two can be compared.

At the same moments it reads the kernel's running totals and books their
growth with the state:

| What | From |
|---|---|
| CPU busy time and frequency, per cluster | `/proc/stat`, cpufreq `stats/time_in_state` |
| CPU power, per cluster | the kernel's energy model in debugfs, when it is in µW: busy time times the mean power over the frequencies used |
| Devices powered: GPU, storage, display controller, Wi-Fi card | `power/runtime_active_time` and `runtime_suspended_time` |
| DDR time per frequency | debugfs `qcom_stats/ddr_stats` |
| Subsystems asleep: modem, ADSP, CDSP, ...; whole chip, CX and DDR while suspended | debugfs `qcom_stats` |
| Wake-ups and time held awake, per source | `/sys/class/wakeup` |
| Bytes through each interface on hardware | `/sys/class/net/*/statistics` |

Each line names the kernel release and the builds listed in
`/etc/battery-stats/tags`: one path per line, named by the link's target or the
file's first line. A device's rootfs carries its own, for example its Denial
`current` link.

## Files

| Path | What |
|---|---|
| `/var/lib/battery-stats/hours.jsonl` | Closed hours, one bucket per line; 31 days are kept |
| `/run/battery-stats/open.json` | The hours still open, rewritten every tick; a restarted daemon carries on from it |

A stop appends the open hours to the file. Buckets with the same hour, state
and build add up.

## Use

```sh
battery-stats             # the last 24 hours per state, with the hardware
battery-stats --last 7d
battery-stats hours       # a line per hour
battery-stats builds      # each build's average per state, for 31 days
battery-stats now         # the state and the power right now
```

## Build and install

```sh
make
make check
sudo make install
sudo systemctl enable --now battery-statsd
```

`build-aarch64.sh` cross-builds for the phones against the workspace
sysroot; then `make install BIN=target/aarch64-unknown-linux-gnu/release`.
battery-statsd runs as root, for debugfs.
