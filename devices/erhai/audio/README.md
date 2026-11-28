# The OnePlus Pad 3's audio on phonewave

The Pad 3 ("erhai", SM8750 "sun") audio, as the re-landing stands it up.
Everything stock is pinned in the vault under `erhai/reference/stock-audio/` (see its README); the
UCM2 profiles are in `services/audio/ucm/ucm2/Qualcomm/sm8750/OnePlus-Pad3/`
and shipped by `alsa-ucm-phonewave` — the packaging is the same as the
other phones, no image recipe change.

## Use cases

The card the re-landing builds (the in-tree q6apm machine on the sm8750
machine driver, model `OnePlus-Pad3`, driver name `sm8750` — that is why
the profiles sit under `Qualcomm/sm8750/`) exposes:

| Device | PCM | The profile's route |
|---|---|---|
| Speaker (HiFi) | `hw,1` 2ch | MM2 → SECONDARY_TDM_RX_0, the six AW88261 on the 8-slot 32-bit TDM |
| Headphones (HiFi) | `hw,2` 2ch | MM3 → RX_CODEC_DMA_RX_0, the WCD9395 HPH (USB-C analog) |
| Mic1 (HiFi) | `hw,0` 2ch, mono source | MM1 → TX_CODEC_DMA_TX_3, AMIC5 on ADC4 feeding both decimators |
| Mic2 (HiFi) | `hw,0` 2ch | the DMIC endfire pair, DMIC2 + DMIC3 |
| Mic3 (HiFi) | `hw,0` 2ch, mono source | a single DMIC, DMIC0, on both decimators |
| Headset (HiFi) | `hw,0` 2ch, mono source | the USB-C headset's microphone, AMIC2 on ADC2, jack detection |

`hw,0` is the only capture FE (MultiMedia1): the four capture devices
share it, the mono sources offered as channel 0 of the stereo capture
(alsa-ucm-conf's split PCM: PipeWire splits it itself, other ALSA clients
get a dsnoop).

The PCM device numbers come from the topology's MultiMedia order — the
kernel's q6apm creates the FE links from the m4's `STREAM_SG_PCM_ADD`
sequence (MM1..MM3 = devices 0..2).

The profile mirrors the NX809J's: PipeWire sets the volume in software,
the UCM pins the fixed gains, and the ADSP stream gain stays at unity
(8192, the stock ceiling — the stock's level steps are the audiox_param
sets below, not mixer levels).

The card has no `Voice Call` verb: the tablet has no modem, so a call is
a VoIP app's stream on the HiFi devices, and the kernel has no q6apm
voice backend to load a voice graph.

## Where each curve comes from

All stock values are from the vault-pinned stock capture (`erhai/reference/stock-audio/`):

- **Speaker** — `odm/etc/mixer_paths.xml`'s `speaker` path: every PA on
  `aw_dev_N_prof` = "Music" + `aw_dev_N_switch` = "Enable" (N = 0, 1, 2,
  3, 5, 6). Stock sets no `aw_dev_N_volume` anywhere; the per-band gains
  live in the ACF profiles (`firmware/qcom/sm8750/erhai/aw88261_acf.bin`,
  the vendor's `aw882xx_acf.bin`) and the level steps in the
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
  decimator 0, `ADC_HIFI` at zero ADC gain, decimator 84 (0 dB on the
  in-tree's 0..124 scale). The stock port takes a second mic (AMIC1 →
  decimator 1); the card offers one mono source instead, so both
  decimators take AMIC5 and the source is channel 0. Which SWR input
  carries ADC4 is the DT's: "ADC4_OUTPUT" routes to "TX SWR_INPUT3",
  which the active LPASS v9_2 tx-macro names "SWR_MIC3" (the old "ADC3"
  item name does not exist in the v9_2 enum — a cset for it fails
  silently and the codec TX DAPM chain never powers).
- **Mic2/Mic3** — the stock `handset-dmic-endfire` (`dual-mic`
  configuration: DMIC2 + DMIC3) and `dmic1` paths. Stock's 1-channel
  endfire *output* was its ACDB graph; the profile captures the raw
  stereo pair instead.
- **Headset** — the stock `headset-mic` path: AMIC2 → ADC2, `TX1 MODE`
  ADC_LP, ADC2 Volume 10, decimator 84. The USB-C jack detection is the
  WCD939x's MBHC (the DT's `qcom,usbcss` mode switch and the MBHC
  thresholds), reported by the card's "Headphone Jack" and "Mic Jack"
  controls.

## Unknowns to confirm

| # | Unknown | How to confirm |
|---|---|---|
| 1 | Which TDM slot drives which physical speaker (the DT `sound-channel` is each PA's slot: aw_dev_0 = i2c9 0x34, aw_dev_1 = i2c4 0x35, aw_dev_2 = i2c9 0x35, aw_dev_3 = i2c4 0x34, aw_dev_5 = i2c4 0x36, aw_dev_6 = i2c9 0x36) | Play a known L/R/mono test through `hw,1`; read each PA's slot from `dmesg` at boot (the aw88261 probe logs its channel) and correlate with the speaker that moves |
| 2 | Which physical mics DMIC0..3 are (the endfire pair is assumed to be DMIC2 + DMIC3, stock's) | Record from Mic2 with both mics; identify the pair in the stereo image, or measure per-mic in a quiet room |
| 3 | The stock HPH/PA level mapping — that the ACF profile + audiox_param ceiling matches the stock loudness | Play the same file at the stock and phonewave maximum, SPL-meter the output, compare against the pinned `SPK_*`/`HP_*` level-set intent |

## Topology

`topology/OnePlus-Pad3-tdm.m4` builds the card's AudioReach (q6apm)
topology; the includes under `topology/audioreach/` and
`topology/util/` are the two validated references' (the commit history
records the provenance):

- the stream subgraphs, the codec-DMA device subgraphs and the
  volume-capped stream gain are the OnePlus Pad 4's (reference
  iceland-mainline's audio/topology, validated on the Pad 4), with the
  speaker stream capped at 8192 = 0 dB;
- the speaker device subgraph (`audioreach/
  subgraph-device-tdm-playback-sal.m4`) is the stock's DeviceRX_Speaker
  ADSP graph without the vendor's Awinic modules: [SAL] -> [MFC] ->
  [LOG] -> [TDM SINK], the caihong reference's (reference
  caihong-linux-rootfs/external/audioreach-topology) module set with the
  stock's endpoint configuration. The 2-channel MultiMedia2 stream is
  upmixed to the endpoint's eight 32-bit slots; the six wired PAs
  consume their slots. The framing (8 slots x 32 bit, 48 kHz, internal
  frame sync, one data lane) is field-validated on the caihong phone,
  which has the same 6x AW882xx TDM layout on the in-tree q6apm.

Build (from `topology/`, the recipe is the pudding's, see
devices/pudding/README.md):

```sh
m4 -I . OnePlus-Pad3-tdm.m4 > /tmp/OnePlus-Pad3.conf
alsatplg -c /tmp/OnePlus-Pad3.conf -o /tmp/OnePlus-Pad3-tplg.bin
```

The four "undefined sink/source widget" messages are the back-end
stream names (the kernel's BE dai widgets, not in the tplg): expected
and benign. The bin goes to
`/usr/lib/firmware/qcom/sm8750/OnePlus-Pad3-tplg.bin`, the machine
driver's `qcom,audioreach-firmware` path.

The in-tree parser (the linux-perpetua kernel's
sound/soc/qcom/qdsp6/topology.c) loads the graph as built:

- the stream subgraphs (shared-memory EPs, PCM dec/enc/cnv, MFC, LOG,
  the capped vol_ctrl pga) and the codec-DMA device subgraphs are all
  in the kernel's `audioreach_load_buffer` module switch;
- the TDM sink is the classic TDM module 0x0700100E, which the in-tree
  switch has no case for (mainline added none since the audio-if
  modules); the widget handler for aif_in discards the rc, so the
  graph still loads — the module is registered from its common tokens
  (id, instance, ports) and the stock instance carries the endpoint
  configuration in the ADSP firmware, as on the caihong
  phone. The endpoint's framing tokens (8 slots x 32 bit, internal
  sync) are in the tplg for the record;
- the SAL, MFC and LOG modules load from their common tokens; the
  stock's module params (the MFC channel mixer table) have no in-tree
  token case and are not forwarded to the ADSP — the caihong phone
  works with the same in-tree kernel, so the ADSP defaults suffice.

The kcontrols the profiles set (the rest are the in-tree codec
drivers' — the AW88261's `Speaker ChN *`, the WCD9395's RX/TX macro
controls):

| kcontrol | where |
|---|---|
| `SECONDARY_TDM_RX_0 Audio Mixer MultiMedia2` | the HiFi verb, the speaker route |
| `stream1.vol_ctrl1 MultiMedia2 Playback Volume` (max 8192) | Speaker, the capped ADSP stream gain |
| `Speaker ChN PCM Playback Volume`, `Speaker ChN Profile Set` (N = 0, 1, 2, 3, 5, 6) | the amplifiers' level (ACF Music 350 = -2.5 dB) and profile |
| `RX_CODEC_DMA_RX_0 Audio Mixer MultiMedia3` | the HiFi verb, the headphone route |
| `stream2.vol_ctrl2 MultiMedia3 Playback Volume` (max 8192) | Headphones, the capped stream gain |
| `MultiMedia1 Mixer TX_CODEC_DMA_TX_3` | the HiFi verb, the mic route |

WirePlumber: the image's shared `51-phone-audio.conf`
(`image/rootfs/usr/share/wireplumber/wireplumber.conf.d/`, in
`phonewave-system`) sets the card to software volume and its
description; it matches this card's name, `alsa_card.OnePlus-Pad3`, next
to the other phones' `alsa_card.platform-sound`.

## The kernel side

The card needs the in-tree q6apm machine, which the erhai kernel
(the linux-perpetua PR this device lands with) has: the machine driver
name `sm8750`, card name `OnePlus-Pad3`, the three back ends
(SECONDARY_TDM_RX_0, RX_CODEC_DMA_RX_0, TX_CODEC_DMA_TX_3), the AW88261
driven over the stock TDM back end, and the `qcom,audioreach-firmware`
path `qcom/sm8750/OnePlus-Pad3-tplg.bin`. No voice board entry: without
a modem the Pad 3 has no use for the q6apm voice graph, and boot images
ship no `qcom/sm8750/OnePlus-Pad3-voice.bin`, so the q6apm-voice driver
adds neither its policy PCM nor its `Voice Call *` controls.

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
