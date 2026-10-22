# callaudio

Puts the sound card on its voice call profile while the modem has a call,
and back on the profile it had afterwards. Holds the call volume and the
microphone mute.

The modem and the DSP carry the speech; the host only starts the DSP's
voice graphs and routes the hardware. A card that can do that has a
`Voice Call` UCM verb: its devices sit on the kernel's policy PCM, and
enabling its microphone device asks the kernel to start the call graphs.
callaudio follows dmail's `org.dmail.Voice1.Calls` on the system bus and,
while any call is dialling, ringing on the other side, active or held,
switches the card's PipeWire device to that verb's profile, without making it
WirePlumber's stored choice. A call still ringing here keeps the normal
profile, so the ringtone plays. When the last call ends, or dmail goes away,
callaudio switches back, and on SIGTERM too.

Volume and mute are the controls the verb names as `PlaybackVolume` and
`CaptureSwitch`. The audio server never sees call speech, so it cannot apply
them itself.

## Build and install

Requires Rust 1.88+, libpipewire 0.3 and alsa-lib development files.

```sh
make
make check
sudo make install
systemctl --user enable --now callaudio
```

`build-aston.sh` cross-builds for the Ace 3 against the workspace sysroot.
callaudio runs in the user's session, beside PipeWire; the user needs access
to dmail (group `dmail`) and to the sound card (group `audio`). On a device
with no card with a `Voice Call` verb it exits at once.

## API

Session bus, name `org.callaudio`, object `/org/callaudio`, interface
`org.callaudio.CallAudio1`. Changes go out as PropertiesChanged.

| Property | Type | |
|---|---|---|
| `Active` | `b` | the card is on its voice call profile |
| `Volume` | `u`, read-write | 0 to `VolumeMax`, highest is loudest; kept between calls |
| `VolumeMax` | `u` | |
| `Muted` | `b`, read-write | the other side hears nothing; only during a call, and each call starts unmuted |

```sh
busctl --user set-property org.callaudio /org/callaudio org.callaudio.CallAudio1 Muted b true
```

## License

MIT
