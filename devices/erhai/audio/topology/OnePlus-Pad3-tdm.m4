# SPDX-License-Identifier: BSD-3-Clause
# Copyright, Linaro Ltd, 2023
include(`audioreach/audioreach.m4')
include(`audioreach/stream-subgraph.m4')
include(`audioreach/device-subgraph.m4')
include(`util/route.m4')
include(`util/mixer.m4')
include(`audioreach/tokens.m4')
dnl
dnl OnePlus Pad 3 ("erhai", SM8750): the microphones and the six AW88261
dnl speaker amplifiers on the stock speaker back end,
dnl TDM-LPAIF-RX-SECONDARY: 48 kHz, 8 slots of 32 bits
dnl (odm/etc/backend_conf.xml), each amplifier on the slot of its channel
dnl (0-3, 5, 6; the kernel's speaker link and the amplifiers' tuning).
dnl hw,0 = MultiMedia1 capture <- TX_CODEC_DMA_TX_3 (WCD939x TX, as in
dnl OnePlus-Pad3-i2s.m4).
dnl hw,1 = MultiMedia2 playback (stereo) -> SECONDARY_TDM_RX_0, through the
dnl stock graph's SAL limiter and MFC (left on the even slots, right on
dnl the odd ones). The stream gain is capped at 8192 (0 dB).
dnl hw,2 = MultiMedia3 playback (stereo) -> RX_CODEC_DMA_RX_0, the WCD939x
dnl HPH outputs on the USB-C headset (stock CODEC_DMA-LPAIF_RXTX-RX-0,
dnl 48 kHz 16 bit stereo, the Pad 4's codec-DMA subgraph); stream gain
dnl capped at 8192 as well. Added after hw,0/hw,1 so their numbers stay.
dnl TDM endpoint (pcm_tdm_api.h PARAM_ID_TDM_INTF_CFG / TDM_LANE_CFG): the
dnl SoC drives bit clock and frame sync; data lane 1 (gpio124,
dnl i2s1_data1, the stock graph's PARAM_ID_TDM_LANE_CFG lane_mask 0x2);
dnl data out not shared. The frame sync is not in the stock calibration
dnl (the vendor HAL sets it): a frame-long sync, not inverted, with the
dnl data starting on it, which is what the amplifiers' tuning reads
dnl (MSB-justified, no bit delay). -D TDM_SYNC_MODE=0 (one-bit pulse),
dnl -D TDM_INVERT=1, -D TDM_DELAY=1 change it.
dnl
ifdef(`TDM_SYNC_MODE', , `define(`TDM_SYNC_MODE', `1')')dnl
ifdef(`TDM_INVERT', , `define(`TDM_INVERT', `0')')dnl
ifdef(`TDM_DELAY', , `define(`TDM_DELAY', `0')')dnl
define(`VOL_CTRL_MAX', `8192')dnl
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-capture.m4, FRONTEND_DAI_MULTIMEDIA1,
	`S16_LE', 48000, 48000, 1, 2,
	0x00004001, 0x00004001, 0x00006001, `110000')
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback-capped.m4, FRONTEND_DAI_MULTIMEDIA2,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004002, 0x00004002, 0x00006010, `110000')
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback-capped.m4, FRONTEND_DAI_MULTIMEDIA3,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004003, 0x00004003, 0x00006020, `110000')
DEVICE_AUDIO_IF_SG_ADD(`audioreach/subgraph-device-tdm-playback-sal.m4',
	`Secondary TDM0', SECONDARY_TDM_RX_0,
	`S32_LE', 48000, 48000, 8, 8,
	LPAIF_INTF_TYPE_LPAIF, I2S_INTF_TYPE_SECONDARY, 0, DATA_FORMAT_FIXED_POINT,
	0x00004006, 0x00004006, 0x00006060, `SECONDARY_TDM_RX_0',
	AUDIO_IF_SYNC_SRC_INTERNAL, AUDIO_IF_CTRL_DATA_OE_DISABLE,
	0xff, 8, 32,
	0, TDM_SYNC_MODE, TDM_INVERT, TDM_DELAY,
	0, AUDIO_IF_LANE_MASK_1, 0,
	0, 0, 0)
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-playback.m4, `RX_CODEC_DMA_RX_0', RX_CODEC_DMA_RX_0,
	`S16_LE', 48000, 48000, 2, 2,
	LPAIF_INTF_TYPE_RXTX, CODEC_INTF_IDX_RX0, 0, DATA_FORMAT_FIXED_POINT,
	0x00004007, 0x00004007, 0x00006070)
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-capture.m4, `TX_CODEC_DMA_TX_3', TX_CODEC_DMA_TX_3,
	`S16_LE', 48000, 48000, 1, 2,
	LPAIF_INTF_TYPE_RXTX, CODEC_INTF_IDX_TX3, 0, DATA_FORMAT_FIXED_POINT,
	0x00004009, 0x00004009, 0x00006090)
STREAM_DEVICE_PLAYBACK_MIXER(SECONDARY_TDM_RX_0, ``SECONDARY_TDM_RX_0'', ``MultiMedia2'')
STREAM_DEVICE_PLAYBACK_ROUTE(SECONDARY_TDM_RX_0, ``SECONDARY_TDM_RX_0 Audio Mixer'', ``MultiMedia2, stream1.logger1'')
STREAM_DEVICE_PLAYBACK_MIXER(RX_CODEC_DMA_RX_0, ``RX_CODEC_DMA_RX_0'', ``MultiMedia3'')
STREAM_DEVICE_PLAYBACK_ROUTE(RX_CODEC_DMA_RX_0, ``RX_CODEC_DMA_RX_0 Audio Mixer'', ``MultiMedia3, stream2.logger1'')
STREAM_DEVICE_CAPTURE_MIXER(FRONTEND_DAI_MULTIMEDIA1, ``TX_CODEC_DMA_TX_3'')
STREAM_DEVICE_CAPTURE_ROUTE(FRONTEND_DAI_MULTIMEDIA1, ``MultiMedia1 Mixer'', ``TX_CODEC_DMA_TX_3, device120.logger1'')
