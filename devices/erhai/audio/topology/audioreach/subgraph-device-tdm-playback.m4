# Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
# SPDX-License-Identifier: BSD-3-Clause
include(`audioreach/audioreach.m4') dnl
include(`util/pcm.m4') dnl
include(`audioreach/subgraph.m4') dnl
include(`audioreach/container.m4') dnl
include(`audioreach/module_log.m4') dnl
include(`audioreach/module_mfc.m4') dnl
include(`audioreach/module_tdm.m4') dnl
include(`audioreach/module_generic.m4') dnl
include(`util/mixer.m4') dnl

undefine(`LOG_MODULE_IID') dnl
undefine(`TDM_MODULE_IID') dnl
undefine(`MFC_MODULE_IID') dnl
undefine(`SAL_MODULE_IID') dnl
undefine(`SPLITTER_MODULE_IID') dnl
undefine(`AW_AFE_MODULE_IID') dnl
undefine(`AW_SKT_MODULE_IID') dnl
undefine(`SG_INDEX') dnl
undefine(`CONTAINER_INDEX') dnl
undefine(`MOD_INDEX') dnl

define(`DEVICE_PERF_MODE', APM_SG_PERF_MODE_LOW_LATENCY) dnl
define(`DEVICE_DIRECTION', APM_SUB_GRAPH_DIRECTION_RX) dnl
define(`CONT_SENARIO_ID', 0xFFFFFFFF) dnl
define(`CONT_CAP', `ifelse(LPASS_VER_COMPAT, 110000, APM_CONTAINER_TYPE_ID_GC, APM_CONTAINER_CAP_ID_EP)') dnl
define(`CONT_POSITION', APM_CONT_GRAPH_POS_GLOBAL_DEV)dnl
define(`DEVICE_DOMAIN_ID', APM_PROC_DOMAIN_ID_ADSP) dnl
define(`CONT_STACK_SIZE', 4096) dnl
dnl Reuse the module IDs and connections from the stock DeviceRX_Speaker graph.
define(`SAL_MODULE_IID', 0x00004012) dnl
define(`LOG_MODULE_IID', 0x00004007) dnl
define(`SPLITTER_MODULE_IID', 0x0000418E) dnl
define(`TDM_MODULE_IID', 0x000042C8) dnl
define(`MFC_MODULE_IID', 0x000042C9) dnl
define(`AW_AFE_MODULE_IID', 0x000042CE) dnl
define(`AW_SKT_MODULE_IID', 0x00004322) dnl
define(`SG_INDEX', 1) dnl
define(`CONTAINER_INDEX', 1) dnl
define(`MOD_INDEX', 1) dnl

define(`DEVICE_PLAYBACK_ROUTE',
`'
`SectionGraph."NAME_PREFIX.$1 $3 Graph" {'
`        index STR($1)'
`        lines ['
`                "NAME_PREFIX.sal$1, , $3 Audio Mixer"'
`                "NAME_PREFIX.aw_skt$1, , NAME_PREFIX.sal$1"'
`                "NAME_PREFIX.mfc$1, , NAME_PREFIX.aw_skt$1"'
`                "NAME_PREFIX.aw_afe$1, , NAME_PREFIX.mfc$1"'
`                "NAME_PREFIX.splitter$1, , NAME_PREFIX.aw_afe$1"'
`                "NAME_PREFIX.logger$1, , NAME_PREFIX.splitter$1"'
`                "NAME_PREFIX.tdm_rx$1, , NAME_PREFIX.logger$1"'
`                "$2 Playback, , NAME_PREFIX.tdm_rx$1"'
`        ]'
`}')

AR_SUBGRAPH(SG_INDEX, DEVICE_DAI_ID, SG_IID_START, DEVICE_PERF_MODE, DEVICE_DIRECTION, CONT_SENARIO_ID)
AR_CONTAINER(CONTAINER_INDEX, CONT_IID_START, CONT_CAP, CONT_STACK_SIZE, CONT_POSITION, DEVICE_DOMAIN_ID)
AR_MODULE_GENERIC(MOD_INDEX, sal, SG_INDEX, CONTAINER_INDEX,
	MODULE_ID_SAL, SAL_MODULE_IID, 21, 1, 1, 2, AW_SKT_MODULE_IID)
AR_MODULE_LOG(MOD_INDEX, SG_INDEX, CONTAINER_INDEX,
	LOG_MODULE_IID, 1, 1, 1, 2, 0x00001586, 1, 0, TDM_MODULE_IID)
AR_MODULE_GENERIC(MOD_INDEX, splitter, SG_INDEX, CONTAINER_INDEX,
	MODULE_ID_SPLITTER, SPLITTER_MODULE_IID, 1, 1, 1, 2, LOG_MODULE_IID)
AR_MODULE_TDM_SINK(MOD_INDEX, SG_INDEX, CONTAINER_INDEX,
	TDM_MODULE_IID, 1, 0, 1, 0,
	DEVICE_INTF_TYPE, DEVICE_INTF_INDEX, DEVICE_DATA_FORMAT,
	DEVICE_AUDIO_IF_SYNC_SRC, DEVICE_AUDIO_IF_CTRL_DATA_OUT_ENABLE,
	DEVICE_AUDIO_IF_SLOT_MASK, DEVICE_AUDIO_IF_NSLOTS_PER_FRAME,
	DEVICE_AUDIO_IF_SLOT_WIDTH, DEVICE_AUDIO_IF_FRAME_SYNC_MODE,
	DEVICE_AUDIO_IF_CTRL_INVERT_SYNC_PULSE,
	DEVICE_AUDIO_IF_CTRL_SYNC_DATA_DELAY,
	DEVICE_AUDIO_IF_ACTIVE_LANE_MASK,
	`DEVICE_NAME', NONE_IID)
AR_MODULE_GENERIC_CAIHONG_AW_CAL(MOD_INDEX, aw_afe, SG_INDEX, CONTAINER_INDEX,
	MODULE_ID_AW_AFE, AW_AFE_MODULE_IID, 1, 1, 1, 2, SPLITTER_MODULE_IID)
AR_MODULE_GENERIC(MOD_INDEX, aw_skt, SG_INDEX, CONTAINER_INDEX,
	MODULE_ID_AW_SKT, AW_SKT_MODULE_IID, 1, 1, 1, 2, MFC_MODULE_IID)
AR_MODULE_MFC_STEREO_TO_8(MOD_INDEX, SG_INDEX, CONTAINER_INDEX,
	MFC_MODULE_IID, 1, 1, 1, 2, AW_AFE_MODULE_IID)
DEVICE_PLAYBACK_ROUTE(MOD_INDEX, `DEVICE_NAME', `MIXER_PREFIX')
