# SPDX-License-Identifier: BSD-3-Clause
# Copyright, Linaro Ltd, 2023
include(`audioreach/audioreach.m4')
include(`audioreach/stream-subgraph.m4')
include(`audioreach/device-subgraph.m4')
include(`util/route.m4')
include(`util/mixer.m4')
include(`audioreach/tokens.m4')
dnl
dnl OnePlus Pad 3 ("erhai", SM8750), microphones only: no playback stream
dnl and no playback back end, so this topology cannot drive the speakers.
dnl hw,0 = MultiMedia1 capture <- TX_CODEC_DMA_TX_3 (WCD939x TX, stock's
dnl record back end). Structure from OnePlus-Pad3.m4 (capture half).
dnl
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-capture.m4, FRONTEND_DAI_MULTIMEDIA1,
	`S16_LE', 48000, 48000, 1, 2,
	0x00004001, 0x00004001, 0x00006001, `110000')
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-capture.m4, `TX_CODEC_DMA_TX_3', TX_CODEC_DMA_TX_3,
	`S16_LE', 48000, 48000, 1, 2,
	LPAIF_INTF_TYPE_RXTX, CODEC_INTF_IDX_TX3, 0, DATA_FORMAT_FIXED_POINT,
	0x00004009, 0x00004009, 0x00006090)
STREAM_DEVICE_CAPTURE_MIXER(FRONTEND_DAI_MULTIMEDIA1, ``TX_CODEC_DMA_TX_3'')
STREAM_DEVICE_CAPTURE_ROUTE(FRONTEND_DAI_MULTIMEDIA1, ``MultiMedia1 Mixer'', ``TX_CODEC_DMA_TX_3, device120.logger1'')
