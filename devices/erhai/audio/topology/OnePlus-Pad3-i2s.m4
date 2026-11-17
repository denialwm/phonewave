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
dnl speaker amplifiers on the secondary MI2S as plain stereo I2S.
dnl hw,0 = MultiMedia1 capture <- TX_CODEC_DMA_TX_3 (WCD939x TX, stock's
dnl record back end; unchanged from OnePlus-Pad3-mic.m4).
dnl hw,1 = MultiMedia2 playback -> SECONDARY_MI2S_RX (stock's
dnl TDM-LPAIF-RX-SECONDARY pins, gpio121-124). The amplifiers' stock
dnl tuning picks the left or the right half of the I2S frame for each.
dnl The stream gain is capped at 8192 (0 dB, the stock ceiling) and the
dnl device graph carries the stock speaker graph's SAL limiter, which the
dnl kernel enables. The data line is SD1 (gpio124, i2s1_data1), measured on
dnl the unit to carry the stream; -D SPEAKER_SD_LINE=SD_LINE_IDX_I2S_SD0 selects
dnl gpio122.
dnl
ifdef(`SPEAKER_SD_LINE', , `define(`SPEAKER_SD_LINE', `SD_LINE_IDX_I2S_SD1')')dnl
define(`VOL_CTRL_MAX', `8192')dnl
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-capture.m4, FRONTEND_DAI_MULTIMEDIA1,
	`S16_LE', 48000, 48000, 1, 2,
	0x00004001, 0x00004001, 0x00006001, `110000')
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback-capped.m4, FRONTEND_DAI_MULTIMEDIA2,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004002, 0x00004002, 0x00006010, `110000')
DEVICE_SG_ADD(audioreach/subgraph-device-i2s-playback-sal.m4, `Secondary', SECONDARY_MI2S_RX,
	`S16_LE', 48000, 48000, 2, 2,
	LPAIF_INTF_TYPE_LPAIF, I2S_INTF_TYPE_SECONDARY, SPEAKER_SD_LINE, DATA_FORMAT_FIXED_POINT,
	0x00004006, 0x00004006, 0x00006060, `SECONDARY_MI2S_RX')
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-capture.m4, `TX_CODEC_DMA_TX_3', TX_CODEC_DMA_TX_3,
	`S16_LE', 48000, 48000, 1, 2,
	LPAIF_INTF_TYPE_RXTX, CODEC_INTF_IDX_TX3, 0, DATA_FORMAT_FIXED_POINT,
	0x00004009, 0x00004009, 0x00006090)
STREAM_DEVICE_PLAYBACK_MIXER(SECONDARY_MI2S_RX, ``SECONDARY_MI2S_RX'', ``MultiMedia2'')
STREAM_DEVICE_PLAYBACK_ROUTE(SECONDARY_MI2S_RX, ``SECONDARY_MI2S_RX Audio Mixer'', ``MultiMedia2, stream1.logger1'')
STREAM_DEVICE_CAPTURE_MIXER(FRONTEND_DAI_MULTIMEDIA1, ``TX_CODEC_DMA_TX_3'')
STREAM_DEVICE_CAPTURE_ROUTE(FRONTEND_DAI_MULTIMEDIA1, ``MultiMedia1 Mixer'', ``TX_CODEC_DMA_TX_3, device120.logger1'')
