# Copyright, Linaro Ltd, 2023
# SPDX-License-Identifier: BSD-3-Clause
dnl STREAM_SG_PCM_ADD(stream, stream-dai-id,  
dnl 	format, min-rate, max-rate, min-channels, max-hcannels,
dnl	sg-iid-start, cont-iid-start, mod-iid-start, lpass-ver-compat)
define(`STREAM_SG_PCM_ADD',
`undefine(`PCM_DAI_ID') dnl'
`undefine(`PCM_FORMAT') dnl'
`undefine(`PCM_MIN_RATE') dnl'
`undefine(`PCM_MAX_RATE') dnl'
`undefine(`PCM_MIN_CHANNELS') dnl'
`undefine(`PCM_MAX_CHANNELS') dnl'
`undefine(`SG_IID_START') dnl'
`undefine(`CONT_IID_START') dnl'
`undefine(`MOD_IID_START') dnl'
`define(`PCM_DAI_ID', `$2') dnl'
`define(`PCM_FORMAT', `$3') dnl'
`define(`PCM_MIN_RATE', `$4') dnl'
`define(`PCM_MAX_RATE', `$5') dnl'
`define(`PCM_MIN_CHANNELS', `$6') dnl'
`define(`PCM_MAX_CHANNELS', `$7') dnl'
`define(`SG_IID_START', `$8') dnl'
`define(`CONT_IID_START', `$9') dnl'
`define(`MOD_IID_START', `$10') dnl'
`define(`NAME_PREFIX', `stream$2') dnl'
`define(`LPASS_VER_COMPAT', `$11') dnl'
`include($1)') dnl

