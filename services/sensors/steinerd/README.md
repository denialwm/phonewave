# steinerd

Sensor service for Qualcomm phones. It reads the accelerometer, ambient light
and proximity sensors from the Snapdragon Sensor Core on the ADSP, over QMI on
QRTR, and offers them on D-Bus. Denial takes screen rotation from it.

The sensors belong to the ADSP: their buses, interrupts and power rails are
the DSP's, and Linux has no driver for them. steinerd is a client of the
sensor core and nothing more.

## What the phone needs

- The ADSP running, with QRTR (`CONFIG_QRTR`, `CONFIG_QRTR_SMD`).
- The sensor drivers on the ADSP read their configuration from the host at
  start, over FastRPC. `hexagonrpcd -s` serves it to the sensors PD from the
  phone's stock files: `sensors/config` and `sensors/sns_reg.conf` from the
  vendor partition (`/vendor/etc/sensors/config`, `sns_reg_config`), and
  `sensors/registry` from persist (`sensors/registry/registry`). It has to
  run from boot: drivers that found no configuration when the ADSP came up
  stay missing until the ADSP restarts. The device's `rootfs/` carries the
  unit; the files are the phone's own and are not in this repository.

`steinerctl list` shows what the core offers. If it lists only platform
sensors (`steinerctl list registry timer` answers, `accel` says none), the
ADSP came up without its configuration.

## Build and install

```sh
make
make check
sudo make install
sudo systemctl enable --now steinerd
```

`build-aarch64.sh` cross-builds for the phones against the workspace sysroot.

## API

System bus name `org.steinerd`, object `/org/steinerd`, interface
`org.steinerd.Sensors1`.

| Member | Type | Meaning |
|---|---|---|
| `Claim(s sensor)` | method | Starts `orientation`, `light` or `proximity` for the caller |
| `Release(s sensor)` | method | Ends the caller's claim |
| `HasOrientation`, `HasLight`, `HasProximity` | `b` | The sensor core offers that sensor |
| `Orientation` | `s` | The physical edge pointing up: `normal` (top edge), `bottom-up`, `left-up`, `right-up`, or `undefined` before the first reading |
| `Light` | `d` | Ambient light in lux |
| `Near` | `b` | Something is close to the screen |

A sensor runs while any client holds a claim and stops with the last one. A
claim also ends when its client leaves the bus. Values change only while
their sensor runs; properties announce changes with `PropertiesChanged`.

Orientation comes from the accelerometer at its lowest rate. Lying flat keeps
the last orientation; a new one must hold for 300 ms, and near a diagonal the
current one wins by 15 degrees.

```sh
steinerctl status
steinerctl watch orientation light
steinerctl list accel proximity ambient_light
steinerctl raw accel 30
```

`list` and `raw` talk to the sensor core directly, without the daemon.
