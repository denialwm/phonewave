# Copyright, Linaro Ltd, 2023
# SPDX-License-Identifier: BSD-3-Clause
include(`util/util.m4') dnl
dnl AR_CONTAINER(index, container-id, cap-id, stack-size, graph-pos, domain)
define(`AR_CONTAINER',
`'
`SectionVendorTuples."NAME_PREFIX.container$1_tuples" {'
`        tokens "audioreach_tokens"'
`        tuples."word.u32_data" {'
`                AR_TKN_U32_CONAINER_INSTANCE_ID STR($2)'
`                AR_TKN_U32_CONAINER_CAPABILITY_ID STR($3)'
`                AR_TKN_U32_CONAINER_STACK_SIZE STR($4)'
`                AR_TKN_U32_CONAINER_GRAPH_POS STR($5)'
`                AR_TKN_U32_CONAINER_PROC_DOMAIN STR($6)'
`        }'
`}'
`'
`SectionData."NAME_PREFIX.container$1_data" {'
`        tuples "NAME_PREFIX.container$1_tuples"'
`}') dnl
