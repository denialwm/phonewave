# Copyright, Linaro Ltd, 2023
# SPDX-License-Identifier: BSD-3-Clause
# OnePlus Pad 3: the speaker back end as the stock DeviceRX_Speaker graph
# drives it, without the vendor's Awinic modules (not in the modem
# partition's ADSP): SAL limiter -> MFC -> logger -> TDM sink, on the
# secondary LPAIF interface. subgraph-device-i2s-playback-sal.m4 with the
# endpoint swapped for module_tdm.m4's TDM sink and the stock graph's MFC
# in front of the logger.
#
# The MFC upmixes the 2-channel stream to the 8 slots with the stock
# graph's channel mixer table (PARAM_ID_CHMIXER_COEFF, MFC 0x4026 of the
# stock graph): even slots the left channel, odd slots the right. The
# table travels as the widget's module configuration
# (SND_SOC_AR_TPLG_MODULE_CFG_TYPE), which the kernel sends after the
# MFC's media format. Its output channel types are the kernel's default
# map for an 8-channel back end (1..8).
include(`audioreach/audioreach.m4') dnl
include(`util/pcm.m4') dnl
include(`audioreach/subgraph.m4') dnl
include(`audioreach/container.m4') dnl
include(`audioreach/module_log.m4') dnl
include(`audioreach/module_tdm.m4') dnl
include(`audioreach/module_generic.m4') dnl
include(`util/mixer.m4') dnl
dnl
#
# SubGraph for Device Backend
#
#  _______________________________________
# |   Sub Graph                           |
# | [SAL] -> [MFC] -> [LOG] -> [TDM EP]   |
# |_______________________________________|

undefine(`SAL_MODULE_IID') dnl
undefine(`MFC_MODULE_IID') dnl
undefine(`LOG_MODULE_IID') dnl
undefine(`TDM_MODULE_IID') dnl
undefine(`SG_INDEX') dnl
undefine(`CONTAINER_INDEX') dnl
undefine(`MOD_INDEX') dnl

undefine(`DEVICE_PERF_MODE') dnl
undefine(`DEVICE_DIRECTION') dnl
undefine(`CONT_SENARIO_ID') dnl
undefine(`CONT_CAP') dnl
undefine(`CONT_POSITION') dnl
undefine(`DEVICE_DOMAIN_ID') dnl
undefine(`CONT_STACK_SIZE') dnl

define(`DEVICE_PERF_MODE', APM_SG_PERF_MODE_LOW_LATENCY) dnl'
define(`DEVICE_DIRECTION', APM_SUB_GRAPH_DIRECTION_TX) dnl'
define(`CONT_SENARIO_ID', APM_SUB_GRAPH_SID_AUDIO_PLAYBACK) dnl'
define(`CONT_CAP', `ifelse(LPASS_VER_COMPAT, 110000, APM_CONTAINER_TYPE_ID_GC, APM_CONTAINER_CAP_ID_EP)') dnl'
define(`CONT_POSITION', APM_CONT_GRAPH_POS_STREAM)dnl'
define(`DEVICE_DOMAIN_ID', APM_PROC_DOMAIN_ID_ADSP) dnl'
define(`CONT_STACK_SIZE', 8192) dnl'
dnl
define(`LOG_MODULE_IID', MOD_IID_START) dnl
define(`TDM_MODULE_IID', eval(MOD_IID_START + 1)) dnl
define(`SAL_MODULE_IID', eval(MOD_IID_START + 2)) dnl
define(`MFC_MODULE_IID', eval(MOD_IID_START + 3)) dnl
define(`SG_INDEX', 1) dnl
define(`CONTAINER_INDEX', 1) dnl
define(`MOD_INDEX', 1) dnl
dnl
dnl DEVICE_PLAYBACK_ROUTE(stream-index, dai-name, mixer-prefix)
define(`DEVICE_PLAYBACK_ROUTE',
`'
`SectionGraph."NAME_PREFIX.$1 $3 Graph" {'
`        index STR($1)'
`        lines ['
`                "NAME_PREFIX.sal$1, , $3 Audio Mixer"'
`                "NAME_PREFIX.mfc$1, , NAME_PREFIX.sal$1"'
`                "NAME_PREFIX.logger$1, , NAME_PREFIX.mfc$1"'
`                "NAME_PREFIX.tdm_rx$1, , NAME_PREFIX.logger$1"'
`                "$2 Playback, , NAME_PREFIX.tdm_rx$1"'
`        ]'
`}')
dnl
dnl The MFC widget with its channel mixer table. Module configuration:
dnl size 96 (header 16 + one parameter record 16 + 60 bytes, padded to 64),
dnl type 0x01001006, two reserved words; the record: instance, parameter
dnl 0x0800101F, 60 bytes; the table (u16, little-endian pairs): 1 table,
dnl 8 outputs, 2 inputs, output types 1-8, input types FL FR, Q14
dnl coefficients per output (L, R).
define(`AR_MODULE_MFC_TDM8_LR',
`'
`SectionVendorTuples."NAME_PREFIX.mfc$1_tuples" {'
`        tokens "audioreach_tokens"'
`'
`        tuples."word.u32_data" {'
`                AR_TKN_U32_MODULE_INSTANCE_ID STR($4)'
`                AR_TKN_U32_MODULE_ID STR(MODULE_ID_MFC)'
`                AR_TKN_U32_MODULE_MAX_IP_PORTS STR($5)'
`                AR_TKN_U32_MODULE_MAX_OP_PORTS STR($6)'
`                AR_TKN_U32_MODULE_SRC_OP_PORT_ID STR($7)'
`                AR_TKN_U32_MODULE_DST_IN_PORT_ID STR($8)'
`                AR_TKN_U32_MODULE_SRC_INSTANCE_ID STR($4)'
`                AR_TKN_U32_MODULE_DST_INSTANCE_ID STR($9)'
`        }'
`}'
`'
`SectionData."NAME_PREFIX.mfc$1_data" {'
`        tuples "NAME_PREFIX.mfc$1_tuples"'
`}'
`'
`SectionData."NAME_PREFIX.mfc$1_cfg" {'
`        words "0x60,0x01001006,0x0,0x0,0x'eval(MOD_IID_START + 3, 16)`,0x0800101F,0x3c,0x0,0x00000001,0x00020008,0x00020001,0x00040003,0x00060005,0x00080007,0x00020001,0x00004000,0x40000000,0x00004000,0x40000000,0x00004000,0x40000000,0x00004000,0x40000000,0x0"'
`}'
`'
`SectionWidget."NAME_PREFIX.mfc$1" {'
`        index STR($1)'
`        type "src"'
`        no_pm "true"'
`        subseq "10"'
`        data ['
`                "NAME_PREFIX.sub_graph$2_data"'
`                "NAME_PREFIX.container$3_data"'
`                "NAME_PREFIX.mfc$1_data"'
`                "NAME_PREFIX.mfc$1_cfg"'
`        ]'
`}') dnl

AR_SUBGRAPH(SG_INDEX, DEVICE_DAI_ID, SG_IID_START, DEVICE_PERF_MODE, DEVICE_DIRECTION, CONT_SENARIO_ID)
AR_CONTAINER(CONTAINER_INDEX, CONT_IID_START, CONT_CAP, CONT_STACK_SIZE, CONT_POSITION, DEVICE_DOMAIN_ID)

dnl SAL ports as in the stock graph (subgraph-device-tdm-playback.m4).
AR_MODULE_GENERIC(MOD_INDEX, sal, SG_INDEX, CONTAINER_INDEX,
	MODULE_ID_SAL, SAL_MODULE_IID, 21, 1, 1, 2, MFC_MODULE_IID)
AR_MODULE_MFC_TDM8_LR(MOD_INDEX, SG_INDEX, CONTAINER_INDEX,
	MFC_MODULE_IID, 1, 1, 1, 2, LOG_MODULE_IID)
AR_MODULE_LOG(MOD_INDEX, SG_INDEX, CONTAINER_INDEX, 	LOG_MODULE_IID, 1, 1, 1, 2, 0x000019ab, 1, 0, TDM_MODULE_IID)
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
DEVICE_PLAYBACK_ROUTE(MOD_INDEX, `DEVICE_NAME', `MIXER_PREFIX')
