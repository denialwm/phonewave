# voicegen

Builds a phone's voice call profile, the firmware file with which the
kernel's q6apm voice session starts calls on the ADSP, from the phone's stock
audio calibration (ACDB). Nothing is captured from Android.

On Qualcomm AudioReach phones the modem hands speech to the ADSP, which plays
and records it once the host has opened and started two DSP graphs: RX
(modem to speakers) and TX (microphones to modem). Android's audio HAL builds
those graphs from the ACDB with Qualcomm's graph services. Those services are
open (ARGS, BSD-3-Clause), so voicegen runs them on the workstation, the way
the HAL does for a call, against `fakedsp.c`: a stand-in ADSP that records
every command and answers as a healthy DSP would. `voiceprofile.py` turns the
recording into the profile.

## Build

Needs gcc, autotools, git and Python 3.11 or later.

```sh
make
```

This clones ARGS at a fixed commit into `build/args`, builds it for the
workstation (`host/` stands in for Android's property calls), then builds
GPR with the DSP datalink replaced by the recorder, and `build/voicegen`.

## Run

```sh
./voiceprofile.py ../../../devices/aston/audio/voice.toml astonsnd-voice.bin
```

The device's `audio/voice.toml` names the ACDB (a path in the vault,
`$PHONEWAVE_VAULT`, default `/mnt/development/denial-os/device-vault`), the
key vectors of both graphs, the backend each graph runs on (`port`, as the
kernel's LPASS port names), and what the host sets itself: endpoint formats
and interfaces, format converters, modules to leave out. The profile and the
ACDB are vendor data: they stay in the vault, never in this repository.

The converters are set to the RX endpoint's format before the call starts.
When it starts, the voice processor applies the call calibration, which can
set a converter's sample width again. The last converter on each path into
the endpoint must therefore end in the endpoint's width: through the
calibration where it sets one, through `converters` where it does not. An
endpoint handed another width plays silence; voiceprofile.py refuses such a
profile and names the converter.

The kernel finds the profile as it finds the card's topology,
`qcom/<driver>/<card>-voice.bin` (`firmware` in voice.toml), and reads it
when the card probes: it goes in the boot image's firmware with the
topology. A card without one has no calls; nothing in the kernel names a
phone.

`build/voicegen ACDB list RX_KEYS TX_KEYS` prints the calibration key
vectors and tagged modules the ACDB has for two graphs, for choosing keys;
`audioreach.py RECORDING` lists a recording (set `FAKEDSP_LOG` to keep one).

## What the profile holds

Little-endian u32 fields throughout.

| Part | Contents |
|---|---|
| Header | magic `MVSA`, version 2, size of the graph part, number of commands |
| Board | RX port, TX port, RX rate, RX sample width, TX decimator mask, volume levels |
| Graphs | magic `MVOP`, version 1, RX size, TX size, then the two GRAPH_OPEN payloads |
| Commands | each: type, graph (0 RX, 1 TX), destination (1 APM, 4 VCPM, or a module), payload size, sequence number, 0, then the payload |

The board part is what the kernel's private voice links need: the two
backend links they copy, the RX link's format, the decimators the TX graph's
channels read (one per channel from decimator 0) and the number of call
volume levels the ACDB calibrates the RX graph for.

Command types: 1 open the graph, 2 set parameters, 3 register calibration,
4 prepare, 5 start. The kernel checks the order and that every voice call
subgraph has calibration registered, then runs the commands one by one when
a call starts. Its "Voice Call Volume" control, 0 to levels - 1, rewrites the
volume level of the calibration keys command.

The format is that of [marble-call-audio](https://github.com/Halhadus/marble-call-audio)
(GPL-2.0), whose kernel patches the q6apm voice session comes from.

## Dropped modules

Stock DSPs load some modules from the vendor partition at run time (aston:
TFA speaker protection, nonlinear compensation; nx809j: Awinic speaker
protection). Linux serves no such files.
`drop` leaves them out of the RX graph and `join` connects their neighbours.
A module the DSP cannot find makes GRAPH_OPEN fail with status 1; so do some
noise suppression variants, which is why aston's TX uses Fluence Endfire,
the stock choice.

## License

MIT. ARGS, which voicegen builds and links on the workstation, is
BSD-3-Clause; none of it ships to the phone.
