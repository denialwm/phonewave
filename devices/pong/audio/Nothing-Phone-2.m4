# Copyright, Linaro Ltd, 2023
# SPDX-License-Identifier: BSD-3-Clause
#
# Nothing Phone (2): stereo playback over the tertiary MI2S to the two TFA9873
# amplifiers (the earpiece and the speaker), derived from SM8450-HDK.m4.
include(`audioreach/audioreach.m4')
include(`audioreach/stream-subgraph.m4')
include(`audioreach/device-subgraph.m4')
include(`util/route.m4')
include(`util/mixer.m4')
include(`audioreach/tokens.m4')
dnl Playback MultiMedia1
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback.m4, FRONTEND_DAI_MULTIMEDIA1,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004001, 0x00004001, 0x00006001)
dnl Tertiary MI2S Playback
DEVICE_SG_ADD(audioreach/subgraph-device-i2s-playback.m4, `Tertiary', TERTIARY_MI2S_RX,
	`S16_LE', 48000, 48000, 2, 2,
	LPAIF_INTF_TYPE_LPAIF, I2S_INTF_TYPE_TERTIARY, SD_LINE_IDX_I2S_SD1, DATA_FORMAT_FIXED_POINT,
	0x00004006, 0x00004006, 0x00006060, `TERTIARY_MI2S_RX')
STREAM_DEVICE_PLAYBACK_MIXER(TERTIARY_MI2S_RX, ``TERTIARY_MI2S_RX'', ``MultiMedia1'')
STREAM_DEVICE_PLAYBACK_ROUTE(TERTIARY_MI2S_RX, ``TERTIARY_MI2S_RX Audio Mixer'', ``MultiMedia1, stream0.logger1'')
