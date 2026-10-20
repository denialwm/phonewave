# hapticd

A small Rust service for Linux vibration motors. Applications send D-Bus
requests; hapticd handles device access, timing, intensity and cancellation.
No desktop framework or client library is required.

## Build and install

Requires Rust 1.87+, pkg-config, libudev development files and D-Bus.

```sh
make
make check
sudo make install
sudo systemctl daemon-reload
sudo systemctl enable --now hapticd
```

The system service permits root, active local login sessions, and optional
`allow_uids` from its configuration. It uses logind to identify active sessions.
For systems without logind, configure the permitted UIDs explicitly.
`--session --config PATH` runs an isolated development instance for the same UID.

## Devices

Choose devices in `/etc/hapticd/config.json`. Paths are administrator-controlled;
clients only see IDs. The standard backend is Linux evdev force feedback:

```json
{
  "default_device": "builtin",
  "devices": [
    {"id": "builtin", "backend": "evdev", "path": "/dev/input/by-path/platform-vibrator-event"}
  ]
}
```

Use the real stable path for your actuator. Alternatively, tag an input event
device with udev properties `HAPTICD_DEVICE=1` and `HAPTICD_ID=builtin`.
Only tagged devices are discovered automatically, so game controllers are not
selected accidentally. `evdev` requires working `FF_RUMBLE` support.

Drivers with `duration` and `activate` sysfs attributes can use:

```json
{"id": "builtin", "backend": "sysfs", "path": "/sys/class/leds/vibrator"}
```

If that driver also exposes an amplitude control, add `"gain": "gain"` and
`"gain_max": 128`, using the attribute and safe maximum documented by the driver.
Without these fields, intensity is reported unsupported and all nonzero levels
mean on. Strength is relative; different motors need different calibration.
Restart after configuration changes. Run `sudo hapticctl rescan` after adding
or reconnecting devices; rescan requires idle actuators.

## API and examples

System bus name `org.hapticd`, object `/org/hapticd/Haptics`, interface
`org.hapticd.Haptics1`:

| Method | Arguments → result |
|---|---|
| `ListDevices` | none → `(id, name, backend, supports_intensity)[]` |
| `Vibrate` | device ID, duration `u32` ms, intensity `f64` → effect `u64` |
| `PlayPattern` | device ID, `(duration_ms, intensity)[]` → effect `u64` |
| `Cancel` | effect ID → nothing |
| `Rescan` | none → nothing; administrator only |

Empty device ID selects the configured default, or the sole available device.
Intensity is `0.0..=1.0`; zero is a pause. Requests allow 1–64 steps and at most
30 seconds total. A new request replaces the previous effect on that actuator.
The reply acknowledges starting playback; it does not wait for completion.
Only the submitting connection can cancel its effect. Disconnecting cancels all
its remaining effects. A stalled client cannot leave an indefinite vibration.
Properties: `DefaultDevice` and `MaxDurationMs`.

```sh
hapticctl list
hapticctl vibrate 15 0.3
hapticctl pattern 35:0.7,60:0,35:0.7
```

Keep a persistent D-Bus connection in applications. The CLI waits for playback
before exiting; a one-shot `busctl call` disconnects and cancels its request.
The daemon blocks while idle; it does not poll the motor.

Tests use a private D-Bus and temporary fake sysfs files. An optional kernel
force-feedback test creates a motorless virtual device via `/dev/uinput`:
`cargo test --test evdev -- --ignored` (requires access to `/dev/uinput`).
