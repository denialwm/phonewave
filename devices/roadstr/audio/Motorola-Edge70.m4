# SPDX-License-Identifier: BSD-3-Clause
# The Motorola Edge 70's AudioReach topology, qcom/eliza/Motorola-Edge70-tplg.bin.
# Built by ../boot/build-boot.sh with infiniti-mainline's audioreach-topology
# macros: m4 -I MACROS Motorola-Edge70.m4 | alsatplg -c /dev/stdin -o ...
# The speaker path converts the 16-bit streams to the 32-bit I2S slots.
include(`audioreach/audioreach.m4')
include(`audioreach/stream-subgraph.m4')
include(`audioreach/device-subgraph.m4')
include(`util/route.m4')
include(`util/mixer.m4')
include(`audioreach/tokens.m4')
dnl Playback MultiMedia1
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback.m4, FRONTEND_DAI_MULTIMEDIA1,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004001, 0x00004001, 0x00006001, `110000')
dnl Playback MultiMedia2
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-vol-playback.m4, FRONTEND_DAI_MULTIMEDIA2,
	`S16_LE', 48000, 48000, 2, 2,
	0x00004002, 0x00004002, 0x00006010, `110000')
dnl Capture MultiMedia3
STREAM_SG_PCM_ADD(audioreach/subgraph-stream-capture.m4, FRONTEND_DAI_MULTIMEDIA3,
	`S16_LE', 48000, 48000, 1, 2,
	0x00004003, 0x00004003, 0x00006020, `110000')
dnl
dnl Both amplifiers on the island's I2S: stock's MI2S-LPAIF_RXTX-RX-PRIMARY,
dnl 32-bit slots, data line SD0. The kernel drives it as quaternary MI2S.
DEVICE_SG_ADD(subgraph-device-i2s-mfc-playback.m4, `Quaternary', QUATERNARY_MI2S_RX,
	`S32_LE', 48000, 48000, 2, 2,
	LPAIF_INTF_TYPE_RXTX, I2S_INTF_TYPE_PRIMARY, SD_LINE_IDX_I2S_SD0, DATA_FORMAT_FIXED_POINT,
	0x00004005, 0x00004005, 0x00006050, `QUATERNARY_MI2S_RX')
dnl
dnl The digital microphones on the VA macro
DEVICE_SG_ADD(audioreach/subgraph-device-codec-dma-capture.m4, `VA_CODEC_DMA_TX_0', VA_CODEC_DMA_TX_0,
	`S16_LE', 48000, 48000, 1, 2,
	LPAIF_INTF_TYPE_VA, CODEC_INTF_IDX_TX0, 0, DATA_FORMAT_FIXED_POINT,
	0x00004006, 0x00004006, 0x00006060)
dnl
STREAM_DEVICE_PLAYBACK_MIXER(QUATERNARY_MI2S_RX, ``QUATERNARY_MI2S_RX'', ``MultiMedia1'', ``MultiMedia2'')
STREAM_DEVICE_PLAYBACK_ROUTE(QUATERNARY_MI2S_RX, ``QUATERNARY_MI2S_RX Audio Mixer'', ``MultiMedia1, stream0.logger1'', ``MultiMedia2, stream1.logger1'')
STREAM_DEVICE_CAPTURE_MIXER(FRONTEND_DAI_MULTIMEDIA3, ``VA_CODEC_DMA_TX_0'')
STREAM_DEVICE_CAPTURE_ROUTE(FRONTEND_DAI_MULTIMEDIA3, ``MultiMedia3 Mixer'', ``VA_CODEC_DMA_TX_0, device110.logger1'')
