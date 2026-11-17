# Copyright, Linaro Ltd, 2023
# SPDX-License-Identifier: BSD-3-Clause
include(`util/util.m4') dnl
dnl AR_SUBGRAPH(index, graph-id, sub-graph-id, perf-mode, dir, sid)
define(`AR_SUBGRAPH',
`'
`SectionVendorTuples."NAME_PREFIX.sub_graph$1_tuples" {'
`        tokens "audioreach_tokens"'
`        tuples."word.u32_data" {'
`                AR_TKN_U32_SUB_GRAPH_INSTANCE_ID STR($3)'
`                AR_TKN_DAI_INDEX STR($2)'
`                AR_TKN_U32_SUB_GRAPH_PERF_MODE STR($4)'
`                AR_TKN_U32_SUB_GRAPH_DIRECTION STR($5)'
`                AR_TKN_U32_SUB_GRAPH_SCENARIO_ID STR($6)'
`        }'
`}'
`'
`SectionData."NAME_PREFIX.sub_graph$1_data" {'
`        tuples "NAME_PREFIX.sub_graph$1_tuples"'
`}') dnl
