# SPDX-License-Identifier: BSD-3-Clause
# Classic TDM hardware endpoint: MODULE_ID_TDM_SINK / MODULE_ID_TDM_SOURCE (0x0700100E / 0x0700100F), the modules
# an ADSP build without the Audio Interface (QAIF) module offers (the OnePlus Pad 4's stock firmware and its ACDB
# use them for TDM-LPAIF-RX-PRIMARY). The kernel's audioreach_tdm_set_media_format() sends PARAM_ID_TDM_INTF_CFG +
# HW_EP_MF + frame size factor from these tokens; the machine driver overrides slot mask / slots / slot width from
# the DT's dai-tdm-slot-* properties (q6tdm_set_tdm_slot). All tokens go in one u32 array.
include(`util/util.m4') dnl
dnl AR_MODULE_TDM_RX(index, sgidx, container-idx, iid, maxip-ports, max-op-ports, src-port, dst-port,
dnl	hw-if-type (lpaif type), hw-if-idx, fmt, dev-name, dst-iid,
dnl	sync-src, ctrl-data-out-enable, slot-mask, nslots-per-frame, slot-width, sync-mode, invert-sync-pulse,
dnl	sync-data-delay)
define(`AR_MODULE_TDM_RX',
`'
`SectionVendorTuples."NAME_PREFIX.tdm_rx$1_tuples" {'
`        tokens "audioreach_tokens"'
`'
`        tuples."word.u32_data" {'
`                AR_TKN_U32_MODULE_INSTANCE_ID STR($4)'
`                AR_TKN_U32_MODULE_ID STR(MODULE_ID_TDM_SINK)'
`                AR_TKN_U32_MODULE_MAX_IP_PORTS STR($5)'
`                AR_TKN_U32_MODULE_MAX_OP_PORTS STR($6)'
`                AR_TKN_U32_MODULE_SRC_OP_PORT_ID STR($7)'
`                AR_TKN_U32_MODULE_DST_IN_PORT_ID STR($8)'
`                AR_TKN_U32_MODULE_SRC_INSTANCE_ID STR($4)'
`                AR_TKN_U32_MODULE_DST_INSTANCE_ID STR($13)'
`                AR_TKN_U32_MODULE_HW_IF_TYPE STR($9)'
`                AR_TKN_U32_MODULE_HW_IF_IDX STR($10)'
`                AR_TKN_U32_MODULE_FMT_DATA STR($11)'
`                AR_TKN_U16_MODULE_SYNC_SRC STR($14)'
`                AR_TKN_U16_MODULE_CTRL_DATA_OUT_ENABLE STR($15)'
`                AR_TKN_U32_MODULE_SLOT_MASK STR($16)'
`                AR_TKN_U16_MODULE_NSLOTS_PER_FRAME STR($17)'
`                AR_TKN_U16_MODULE_SLOT_WIDTH STR($18)'
`                AR_TKN_U16_MODULE_SYNC_MODE STR($19)'
`                AR_TKN_U16_MODULE_CTRL_INVERT_SYNC_PULSE STR($20)'
`                AR_TKN_U16_MODULE_CTRL_SYNC_DATA_DELAY STR($21)'
`        }'
`}'
`'
`SectionData."NAME_PREFIX.tdm_rx$1_data" {'
`        tuples "NAME_PREFIX.tdm_rx$1_tuples"'
`}'
`'
`SectionWidget."NAME_PREFIX.tdm_rx$1" {'
`        index STR($1)'
`        type "aif_in"'
`        no_pm "true"'
`        stream_name "$12 TDM0 Playback"'
`        subseq "10"'
`        data ['
`                "NAME_PREFIX.sub_graph$2_data"'
`                "NAME_PREFIX.container$3_data"'
`                "NAME_PREFIX.tdm_rx$1_data"'
`        ]'
`}') dnl

dnl AR_MODULE_TDM_SINK(index, sgidx, container-idx, iid,
dnl max-ip-ports, max-op-ports, src-port, dst-port,
dnl hw-if-type, hw-if-idx, data-format, sync-src, ctrl-data-out-enable,
dnl slot-mask, slots-per-frame, slot-width, sync-mode,
dnl invert-sync-pulse, sync-data-delay, active-lane-mask, device-name, dst-iid)
define(`AR_MODULE_TDM_SINK',
`'
`SectionVendorTuples."NAME_PREFIX.tdm_rx$1_tuples" {'
`        tokens "audioreach_tokens"'
`'
`        tuples."word.u32_data" {'
`                AR_TKN_U32_MODULE_INSTANCE_ID STR($4)'
`                AR_TKN_U32_MODULE_ID STR(MODULE_ID_TDM_SINK)'
`                AR_TKN_U32_MODULE_MAX_IP_PORTS STR($5)'
`                AR_TKN_U32_MODULE_MAX_OP_PORTS STR($6)'
`                AR_TKN_U32_MODULE_SRC_OP_PORT_ID STR($7)'
`                AR_TKN_U32_MODULE_DST_IN_PORT_ID STR($8)'
`                AR_TKN_U32_MODULE_SRC_INSTANCE_ID STR($4)'
`                AR_TKN_U32_MODULE_DST_INSTANCE_ID STR($22)'
`                AR_TKN_U32_MODULE_HW_IF_TYPE STR($9)'
`                AR_TKN_U32_MODULE_HW_IF_IDX STR($10)'
`                AR_TKN_U32_MODULE_FMT_DATA STR($11)'
`                AR_TKN_U16_MODULE_SYNC_SRC STR($12)'
`                AR_TKN_U16_MODULE_CTRL_DATA_OUT_ENABLE STR($13)'
`                AR_TKN_U32_MODULE_SLOT_MASK STR($14)'
`                AR_TKN_U16_MODULE_NSLOTS_PER_FRAME STR($15)'
`                AR_TKN_U16_MODULE_SLOT_WIDTH STR($16)'
`                AR_TKN_U16_MODULE_SYNC_MODE STR($17)'
`                AR_TKN_U16_MODULE_CTRL_INVERT_SYNC_PULSE STR($18)'
`                AR_TKN_U16_MODULE_CTRL_SYNC_DATA_DELAY STR($19)'
`                AR_TKN_U32_MODULE_ACTIVE_LANE_MASK STR($20)'
`        }'
`}'
`'
`SectionData."NAME_PREFIX.tdm_rx$1_data" {'
`        tuples "NAME_PREFIX.tdm_rx$1_tuples"'
`}'
`'
`SectionWidget."NAME_PREFIX.tdm_rx$1" {'
`        index STR($1)'
`        type "aif_in"'
`        no_pm "true"'
`        stream_name "$21 Playback"'
`        subseq "10"'
`        data ['
`                "NAME_PREFIX.sub_graph$2_data"'
`                "NAME_PREFIX.container$3_data"'
`                "NAME_PREFIX.tdm_rx$1_data"'
`        ]'
`}') dnl
