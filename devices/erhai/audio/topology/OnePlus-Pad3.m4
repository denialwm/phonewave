# SPDX-License-Identifier: BSD-3-Clause
# Copyright, Linaro Ltd, 2023
include(`audioreach/audioreach.m4')
include(`audioreach/stream-subgraph.m4')
include(`audioreach/device-subgraph.m4')
include(`util/route.m4')
include(`util/mixer.m4')
include(`audioreach/tokens.m4')
dnl
dnl OnePlus Pad 3 ("erhai", SM8750): the stream gain into the speakers
dnl (MultiMedia1 -> SECONDARY_TDM_RX_0 -> 6x AW882xx) is capped, like the
dnl Pad 4's (its speakers are the only unprotected output). 8192 = 0 dB,
dnl the stock ceiling: the stock's level steps are the audiox_param sets
dnl (erhai/reference/stock-audio/odm/etc/audio/audiox_param/ in the vault), never a mixer level, and
dnl stock's PAL stream volume is a linear 0..1.0 factor, never above
dnl unity. The protection in front is the PipeWire chain; the kernel
dnl keeps the ADSP's stream gain within this maximum.
dnl
dnl The structure is the Pad 4's (reference iceland-mainline's
dnl audio/topology/OnePlus-Pad4.m4) with the speaker back end on
dnl SECONDARY_TDM_RX_0 instead of PRIMARY_TDM_RX_0 (stock's
dnl TDM-LPAIF-RX-SECONDARY), and without the WSA / DisplayPort links:
dnl the stock back ends are the TDM speakers (sub graph mirroring the
dnl stock DeviceRX_Speaker ADSP graph, reference
dnl caihong-linux-rootfs/external/audioreach-topology), the WCD939X HPH on
dnl RX_CODEC_DMA_RX_0, the microphone link TX_CODEC_DMA_TX_3 (one
dnl decimator, stock's record back end) and the reserved VA capture
dnl link. The MultiMedia order is the card's PCM device order (the
dnl kernel's q6apm creates the FE links from this sequence):
dnl hw,0 MM1 speaker, hw,1 MM2 headphones, hw,2 MM3 mic, hw,3 MM4 VA
dnl (reserved); the voice policy PCM is the kernel's, pinned at 13.
dnl
define(`VOL_CTRL_MAX', `8192')dnl
#
# Stream SubGraph  for MultiMedia Playback
#
#  ______________________________________________
# |               Sub Graph 1                    |
# | [WR_SH] -> [PCM DEC] -> [PCM CONV] -> [LOG]  |- Kcontrol
# |______________________________________________|
#
dnl Playback MultiMedia1 -> hw,0 (speakers)
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback-capped.m4, FRONTEND_DAI_MULTIMEDIA1,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004001, 0x00004001, 0x00006001, `110000')
dnl Playback MultiMedia2 -> hw,1 (headphones, WCD939X HPH)
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback-capped.m4, FRONTEND_DAI_MULTIMEDIA2,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004002, 0x00004002, 0x00006010, `110000')
dnl Capture MultiMedia3 -> hw,2 (microphones)
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-capture.m4, FRONTEND_DAI_MULTIMEDIA3,
	`S16_LE', 48000, 48000, 1, 2,
	0x00004003, 0x00004003, 0x00006020, `110000')
dnl Capture MultiMedia4 -> hw,3 (VA, reserved)
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-capture.m4, FRONTEND_DAI_MULTIMEDIA4,
	`S16_LE', 48000, 48000, 1, 2,
	0x00004004, 0x00004004, 0x00006030, `110000')
#
#
# Device SubGraph  for Secondary TDM Playback (speakers)
#
#        _____________________________________
#       |            Sub Graph                |
# Mixer-| [SAL]->[AW_SKT]->[MFC]->[AW_AFE]    |
#       | ->[SPLITTER]->[LOG]->[TDM EP]       |
#       |_____________________________________|
#
dnl The six AW882xx speakers on the stock back end TDM-LPAIF-RX-SECONDARY.
dnl The sub graph mirrors the stock's DeviceRX_Speaker ADSP graph (module
dnl chain and stock module instance ids, see
dnl subgraph-device-tdm-playback.m4): the 2-channel stream is upmixed to
dnl the eight 32-bit slots, the amplifiers consume their six wired slots.
dnl TDM endpoint: 8 slots x 32 bit, 48 kHz, the LPASS drives the frame
dnl sync (internal), data-out enable, one data lane.
dnl DEVICE_AUDIO_IF_SG_ADD(subgraph, device-name, dai-id,
dnl 	format, min-rate, max-rate, min-channels, max-channels,
dnl	interface-type, interface-index, sd-line-index, data-format,
dnl	sg-iid-start, cont-iid-start, mod-iid-start, mixer-prefix,
dnl	sync-src, data-out-enable, slot-mask, nslots, slot-width,
dnl	intf-mode, frame-sync-mode, invert-sync-pulse, sync-data-delay,
dnl	qaif-type, lane-mask, frame-sync-rate, bit-clk, inv-int-clk,
dnl	inv-ext-clk)
dnl The device name is the DAPM endpoint (the kernel's "Secondary TDM0
dnl Playback" widget); the mixer prefix is the kcontrol's, and the
dnl profiles' csets are "SECONDARY_TDM_RX Audio Mixer <MultiMedia>".
DEVICE_AUDIO_IF_SG_ADD(`audioreach/subgraph-device-tdm-playback.m4',
	`Secondary TDM0', SECONDARY_TDM_RX_0,
	`S32_LE', 48000, 48000, 8, 8,
	LPAIF_INTF_TYPE_LPAIF, I2S_INTF_TYPE_SECONDARY, 0, DATA_FORMAT_FIXED_POINT,
	0x00004006, 0x00004006, 0x00006060, `SECONDARY_TDM_RX',
	AUDIO_IF_SYNC_SRC_INTERNAL, AUDIO_IF_CTRL_DATA_OE_ENABLE,
	0xff, 8, 32,
	AUDIO_IF_INTF_MODE_TDM, AUDIO_IF_FRAME_SYNC_MODE_SHORT_SYNC,
	AUDIO_IF_CTRL_INVERT_SYNC_PULSE_DISABLE,
	AUDIO_IF_CTRL_SYNC_DATA_DELAY_DISABLE,
	AUDIO_IF_TYPE_QAIF, AUDIO_IF_LANE_MASK_1, 0,
	AUDIO_IF_I_BIT_CLK_EN, AUDIO_IF_INT_CLK_NORMAL,
	AUDIO_IF_EXT_CLK_NORMAL)
dnl
dnl WCD939X HPH Playback
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-playback.m4, `RX_CODEC_DMA_RX_0', RX_CODEC_DMA_RX_0,
	`S16_LE', 48000, 48000, 2, 2,
	LPAIF_INTF_TYPE_RXTX, CODEC_INTF_IDX_RX0, 0, DATA_FORMAT_FIXED_POINT,
	0x00004007, 0x00004007, 0x00006070)
dnl
dnl Microphone Capture (stock's record back end, one decimator)
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-capture.m4, `TX_CODEC_DMA_TX_3', TX_CODEC_DMA_TX_3,
	`S16_LE', 48000, 48000, 1, 2,
	LPAIF_INTF_TYPE_RXTX, CODEC_INTF_IDX_TX3, 0, DATA_FORMAT_FIXED_POINT,
	0x00004009, 0x00004009, 0x00006090)
dnl
dnl VA Capture (reserved; no device claims the link yet)
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-capture.m4, `VA_CODEC_DMA_TX_0', VA_CODEC_DMA_TX_0,
	`S16_LE', 48000, 48000, 1, 2,
	LPAIF_INTF_TYPE_VA, CODEC_INTF_IDX_TX0, 0, DATA_FORMAT_FIXED_POINT,
	0x00004008, 0x00004008, 0x00006080)
dnl

STREAM_DEVICE_PLAYBACK_MIXER(SECONDARY_TDM_RX_0, ``SECONDARY_TDM_RX'', ``MultiMedia1'')
STREAM_DEVICE_PLAYBACK_MIXER(RX_CODEC_DMA_RX_0, ``RX_CODEC_DMA_RX_0'', ``MultiMedia1'', ``MultiMedia2'')

STREAM_DEVICE_PLAYBACK_ROUTE(SECONDARY_TDM_RX_0, ``SECONDARY_TDM_RX Audio Mixer'', ``MultiMedia1, stream0.logger1'')
STREAM_DEVICE_PLAYBACK_ROUTE(RX_CODEC_DMA_RX_0, ``RX_CODEC_DMA_RX_0 Audio Mixer'', ``MultiMedia1, stream0.logger1'', ``MultiMedia2, stream1.logger1'')

dnl STREAM_DEVICE_CAPTURE_MIXER(stream-index, kcontrol1, kcontrol2... kcontrolN)
STREAM_DEVICE_CAPTURE_MIXER(FRONTEND_DAI_MULTIMEDIA3, ``VA_CODEC_DMA_TX_0'',``TX_CODEC_DMA_TX_3'' )
STREAM_DEVICE_CAPTURE_MIXER(FRONTEND_DAI_MULTIMEDIA4, ``VA_CODEC_DMA_TX_0'',``TX_CODEC_DMA_TX_3'' )
dnl STREAM_DEVICE_CAPTURE_ROUTE(stream-index, mixer-name, route1, route2.. routeN)
STREAM_DEVICE_CAPTURE_ROUTE(FRONTEND_DAI_MULTIMEDIA3, ``MultiMedia3 Mixer'', ``VA_CODEC_DMA_TX_0, device110.logger1'', ``TX_CODEC_DMA_TX_3, device120.logger1'')
STREAM_DEVICE_CAPTURE_ROUTE(FRONTEND_DAI_MULTIMEDIA4, ``MultiMedia4 Mixer'', ``VA_CODEC_DMA_TX_0, device110.logger1'', ``TX_CODEC_DMA_TX_3, device120.logger1'')
