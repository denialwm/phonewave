# The OnePlus Pad 3's audio on phonewave

The Pad 3 ("erhai", SM8750 "sun") audio, as the re-landing stands it up.
Everything stock is pinned in the vault under `erhai/reference/stock-audio/` (see its README); the
UCM2 profiles are in `services/audio/ucm/ucm2/Qualcomm/sm8750/OnePlus-Pad3/`
and shipped by `alsa-ucm-phonewave` — the packaging is the same as the
other phones, no image recipe change.

## Use cases

The card the re-landing builds (the R2 in-tree machine on the sm8750
machine driver, model `OnePlus-Pad3`, driver name `sm8750` — that is why
the profiles sit under `Qualcomm/sm8750/` and `conf.d/sm8750/`) exposes:

| Device | PCM | The profile's route |
|---|---|---|
| Speaker (HiFi) | `hw,0` 2ch | MM1 → SECONDARY_TDM_RX, the six AW882xx on the 8-slot 32-bit TDM |
| Headphones (HiFi) | `hw,1` 2ch | MM2 → RX_CODEC_DMA_RX_0, the WCD939X HPH (USB-C analog jack) |
| Mic1 (HiFi) | `hw,2` 1ch | MM3 → TX_CODEC_DMA_TX_3, AMIC5 on decimator 0 |
| Mic2 (HiFi) | `hw,2` 2ch | the DMIC endfire pair, DMIC2 + DMIC3 |
| Mic3 (HiFi) | `hw,2` 1ch | a single DMIC, DMIC0 |

`hw,3` (MM4 → VA_CODEC_DMA_TX_0) is reserved for the VA capture link;
no device claims it yet.

The PCM device numbers come from the topology's MultiMedia order — the
kernel's q6apm creates the FE links from the m4's `STREAM_SG_PCM_ADD`
sequence (MM1..MM4 = devices 0..3).
The m4 is `topology/OnePlus-Pad3.m4` (see "Topology"); **confirm the
numbers on first boot** (`cat /proc/asound/cards`, then play/capture
each device).

The profile mirrors the NX809J's: PipeWire sets the volume in software,
the UCM pins the fixed gains, and the ADSP stream gain stays at unity
(8192, the stock ceiling — the stock's level steps are the audiox_param
sets below, not mixer levels).

## Where each curve comes from

All stock values are from the vault-pinned stock capture (`erhai/reference/stock-audio/`):

- **Speaker** — `odm/etc/mixer_paths.xml`'s `speaker` path: every PA on
  `aw_dev_N_prof` = "Music" + `aw_dev_N_switch` = "Enable" (N = 0, 1, 2,
  3, 5, 6). Stock sets no `aw_dev_N_volume` anywhere; the per-band gains
  live in the ACF profiles (`firmware/qcom/sm8750/aw882xx_acf.bin`, wired
  in with the voice profile) and the level steps in the
  `odm/etc/audio/audiox_param/` `SPK_<SCENARIO>_lvl1..5` sets (OPAX,
  Erhai_20250730_V16). The TDM framing (48k/32/8) is
  `odm/etc/backend_conf.xml`'s `TDM-LPAIF-RX-SECONDARY`.
- **Headphones** — `odm/etc/mixer_paths.xml`'s `headphones` path,
  translated to the in-tree wcd939x/lpass-rx-macro control names
  (`RX_HPH_PWR_MODE` → `RX_HPH PWR Mode`, `Compander` → `..._COMP
  Switch`); stock's RX digital volume 82 (−1 dB on the vendor 0..84
  scale) is 39 on the in-tree −84..40 scale; HPHL/HPHR Volume 16 is
  stock's fixed analog level. Level steps: the `HP_*` audiox_param sets.
- **Mic1** — the stock resourcemanager's default IN device, the
  `handset-mic` path (`vendor/etc/audio/sku_sun/`): AMIC5 → ADC4 →
  decimator 0, `ADC_HIFI` at zero ADC gain, decimator 84 (0 dB; 40
  in-tree). The stock record backend is 1 channel
  (`backend_conf.xml`'s `CODEC_DMA-LPAIF_RXTX-TX-3`), so only decimator
  0 is captured.
- **Mic2/Mic3** — the stock `handset-dmic-endfire` (`dual-mic`
  configuration: DMIC2 + DMIC3) and `dmic1` paths. Stock's 1-channel
  endfire *output* was its ACDB graph; the profile captures the raw
  stereo pair instead.
- **Voice** — not profiled: the Pad 3 has no modem, so a call is a VoIP
  app's stream on the HiFi devices, and the card has no `Voice Call`
  verb (the conf.d file says why). `voice.toml` stays only as the
  record of the stock voice graph (the `OnePlus-Pad3-voice.bin` it
  generates from the pinned board ACDB, which no image ships): RX on
  SECONDARY_MI2S_RX 48k/32/8, TX on TX_CODEC_DMA_TX_3, decimator mask
  0x1 (decimator 0 only), six volume levels. The UL mic is the `handset-mic` first mic (AMIC5). The policy
  PCM is 2-channel S16 while the graph carries one mic channel; the
  second capture channel is the graph's copy, not a second mic.

## Unknowns to confirm on first boot

| # | Unknown | How to confirm |
|---|---|---|
| 1 | Which TDM slot drives which physical speaker (the DT `sound-channel` is each PA's slot: aw_dev_0 = i2c9 0x34, aw_dev_1 = i2c4 0x35, aw_dev_2 = i2c9 0x35, aw_dev_3 = i2c4 0x34, aw_dev_5 = i2c4 0x36, aw_dev_6 = i2c9 0x36) | Play a known L/R/mono test through `hw,0`; read each PA's slot from `dmesg` at boot (the aw882xx probe logs its channel) and correlate with the speaker that moves |
| 2 | The in-tree tx-macro's SWR input order — Mic1 assumes ADC4 rides the input named `ADC3` (its four `ADC0..3` inputs mapping to ADC1..ADC4) | `amixer -c <card> contents \| grep "TX SMIC MUX0"` for the control's range; record from `hw,2` and verify level in the test app; if the order differs, swap the `TX SMIC MUX0` value |
| 3 | USB-C jack detection — whether the in-tree wcd939x MBHC detects the USB-C analog plug (`qcom,usbcss` in the stock DT) | Plug the jack, then `amixer -c <card> sget 'MBHC'` / watch `dmesg` for the MBHC events; without detection, `wireplumber` will not switch to the Headphones device on plug |
| 4 | Bluetooth — not on this card, by design: A2DP and HFP are PipeWire's bluez5 devices over the WCN7860's HCI UART (SCO over HCI for HFP), not the stock LPASS BT SoundWire offload | See "Bluetooth audio" below |
| 5 | Which physical mics DMIC0..3 are (the endfire pair is assumed to be DMIC2 + DMIC3, stock's) | Record from Mic2 with both mics; identify the pair in the stereo image, or measure per-mic in a quiet room |
| 6 | The stock HPH/PA level mapping — that the ACF profile + audiox_param ceiling matches the stock loudness | Play the same file at the stock and phonewave maximum, SPL-meter the output, compare against the pinned `SPK_*`/`HP_*` level-set intent |

## Topology

`topology/OnePlus-Pad3.m4` builds the card's AudioReach (q6apm)
topology; the includes under `topology/audioreach/` and
`topology/util/` are the two validated references' (the commit history
records the provenance):

- the stream subgraphs, the codec-DMA device subgraphs and the
  volume-capped stream gain are the OnePlus Pad 4's (reference
  iceland-mainline's audio/topology, validated on the Pad 4), with the
  speaker stream capped at 8192 = 0 dB;
- the speaker device subgraph (`audioreach/
  subgraph-device-tdm-playback.m4`) is the caihong reference's (reference
  caihong-linux-rootfs/external/audioreach-topology) mirror of the
  stock's DeviceRX_Speaker ADSP graph: [SAL] -> [AW_SKT] -> [MFC] ->
  [AW_AFE] -> [SPLITTER] -> [LOG] -> [TDM SINK], with the stock's
  module instance ids (0x4012, 0x4322, 0x42C9, 0x42CE, 0x418E, 0x4007,
  0x42C8 — reused from the caihong phone's graph; if the stock erhai
  graph's differ, the first-boot symptom is a silent or distorted
  speaker). The 2-channel MultiMedia1 stream is upmixed to the
  endpoint's eight 32-bit slots; the six wired PAs consume their
  slots. The chain and the framing (8 slots x 32 bit, 48 kHz, internal
  frame sync, short sync, one data lane) are field-validated on the
  caihong phone, which has the same 6x AW882xx TDM layout on the
  in-tree q6apm.

Build (from `topology/`, the recipe is the pudding's, see
devices/pudding/README.md):

```sh
m4 -I . OnePlus-Pad3.m4 > /tmp/OnePlus-Pad3.conf
alsatplg -c /tmp/OnePlus-Pad3.conf -o /tmp/OnePlus-Pad3-tplg.bin
```

The four "undefined sink/source widget" messages are the back-end
stream names (the kernel's BE dai widgets, not in the tplg): expected
and benign. The bin goes to
`/usr/lib/firmware/qcom/sm8750/OnePlus-Pad3-tplg.bin`, the machine
driver's `qcom,audioreach-firmware` path (R2).

The in-tree parser (the linux-perpetua kernel's
sound/soc/qcom/qdsp6/topology.c) loads the graph as built:

- the stream subgraphs (shared-memory EPs, PCM dec/enc/cnv, MFC, LOG,
  the capped vol_ctrl pga) and the codec-DMA device subgraphs are all
  in the kernel's `audioreach_load_buffer` module switch;
- the TDM sink is the classic TDM module 0x0700100E, which the in-tree
  switch has no case for (mainline added none since the audio-if
  modules); the widget handler for aif_in discards the rc, so the
  graph still loads — the module is registered from its common tokens
  (id, instance, ports) and the stock instance 0x42C8 carries the
  endpoint configuration in the ADSP firmware, as on the caihong
  phone. The endpoint's framing tokens (8 slots x 32 bit, internal
  sync) are in the tplg for the record;
- the SAL / SPLITTER / AW_AFE / AW_SKT / MFC modules load from their
  common tokens; the stock's module params (the MFC stereo-to-8 flag,
  token 277, and the AW calibration token 278) have no in-tree token
  case and are not forwarded to the ADSP — the caihong phone works
  with the same in-tree kernel, so the ADSP defaults suffice.

The kcontrols the profiles set (the rest are the in-tree codec
drivers' — the AW88261's `Speaker ChN *`, the WCD939X's RX/TX macro
controls):

| kcontrol | where |
|---|---|
| `SECONDARY_TDM_RX Audio Mixer MultiMedia1` | the HiFi verb, the speaker route |
| `MultiMedia1 Playback Volume` (max 8192) | Speaker, the capped ADSP stream gain |
| `RX_CODEC_DMA_RX_0 Audio Mixer MultiMedia1/2` | the headphone route |
| `MultiMedia2 Playback Volume` (max 8192) | Headphones, the capped stream gain |
| `MultiMedia3 Mixer TX_CODEC_DMA_TX_3` | the HiFi verb, the mic route |
| `MultiMedia4 Mixer VA_CODEC_DMA_TX_0` | the reserved VA link |

WirePlumber: `image/rootfs/usr/share/wireplumber/
wireplumber.conf.d/51-phone-audio.conf` sets the card to software
volume — the rule matches `alsa_card.OnePlus-Pad3` in addition to
`alsa_card.platform-sound` (the other phones' card name).

## Needs after the rebase

The profiles and the topology are userspace-only and complete; they
are inert until the R2 kernel work lands:

1. **The R2 in-tree machine** (driver name `sm8750`, card name
   `OnePlus-Pad3`): the machine driver with the four back ends
   (SECONDARY_TDM_RX_0, RX_CODEC_DMA_RX_0, TX_CODEC_DMA_TX_3,
   VA_CODEC_DMA_TX_0) and the `qcom,audioreach-firmware` path
   `qcom/sm8750/OnePlus-Pad3-tplg.bin`.
2. No voice board entry: without a modem the Pad 3 has no use for the
   q6apm voice graph, and boot images ship no
   `qcom/sm8750/OnePlus-Pad3-voice.bin`, so the q6apm-voice driver adds
   neither its policy PCM nor its `Voice Call *` controls.

The card name in the profiles is `OnePlus-Pad3`; if the R2 machine
renames the model, the `conf.d/` file and the directory move together
with it.

## Bluetooth audio

Bluetooth audio needs no sound card and no kernel audio back end. The
WCN7860's Bluetooth is on its HCI UART (`hci_qca`), and both profiles
ride it:

- **A2DP** (headphones, speakers; the Pad 3 as a sink too) is ACL data:
  BlueZ's built-in `a2dp`/`avrcp` plugins hand the transport to
  PipeWire's bluez5 plugin (`pipewire-audio`), which encodes on the host
  — SBC, AAC (libfdk-aac), aptX and aptX HD (libfreeaptx), LDAC
  (libldac), FastStream, Opus. (LE Audio, LC3 over ISO, is not tried
  yet.)
- **HFP** (headsets, VoIP calls) is PipeWire's native HFP back end over
  RFCOMM, with the speech on SCO over HCI: the kernel sets up the eSCO
  link with HCI Enhanced Setup Synchronous Connection and asks for the
  HCI data path (0) unless a socket opts into the controller's offload
  path (1, the stock BTFM/SLIMbus route, which this port does not have).
  CVSD, and mSBC/LC3-SWB wideband since `hci_qca` flags the WCN7860 as
  wideband-capable. WirePlumber switches a headset to HFP when an app
  opens its microphone.

Keep BlueZ's `Experimental` off (`/etc/bluetooth/main.conf`): BlueZ
5.88's own HFP hands-free plugin is experimental-only, and with it on it
would take the HFP UUID from PipeWire.

