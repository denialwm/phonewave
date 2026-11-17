# Copyright, Linaro Ltd, 2023
# SPDX-License-Identifier: BSD-3-Clause
include(`audioreach/audioreach.m4') dnl
dnl stream_device_list: list the devices or streams from argument list
define(`stream_device_list',
`ifelse(`$#', `1', ,
`stream_device_list(shift($@))
		$2')') dnl
dnl 
dnl STREAM_DEVICE_CAPTURE_MIXER(stream-index)
define(`STREAM_DEVICE_CAPTURE_MIXER',
`'
`SectionWidget."`MultiMedia'eval($1 + 1)` Mixer'" {'
`    	reg "-1"'
`        index STR($1)'
`        type "mixer"'
`'
`        mixer ['
`		stream_device_list($@)'
`        ]'
`'
`	data ['
`                "stream$1.sub_graph1_data"'
`	]'
`}')
dnl
dnl STREAM_DEVICE_PLAYBACK_MIXER(dev-index, device-mixer, strem1, stream2... streamN)
define(`STREAM_DEVICE_PLAYBACK_MIXER',
`'
`SectionWidget."$2 Audio Mixer" {'
`    	reg "-1"'
`        index STR($1)'
`        type "mixer"'
`'
`        mixer ['
`		stream_device_list(shift($@))'
`        ]'
`'
`	data ['
`                "device$1.sub_graph1_data"'
`	]'
`}')
dnl
dnl DEVICE_CAPTURE_MIXER(index, name)
define(`DEVICE_CAPTURE_MIXER',
`'
`SectionControlMixer."$2" {'
`	reg "-1"'
`	index STR($1)'
`	max "1"'
`	invert "false"'
`'
`	channel."fl" {'
`                reg "-1"'
`    		 max "1"'
`        }'
`'
`	ops."ctl" {'
`		get "SND_SOC_AR_TPLG_FE_BE_GRAPH_CTL_MIX"'
`		put "SND_SOC_AR_TPLG_FE_BE_GRAPH_CTL_MIX"'
`		info "1"'
`	}'
`'
`	data ['
`                "NAME_PREFIX.sub_graph$1_data"'
`	]'
`}')
dnl
dnl STREAM_PLAYBACK_MIXER(stream-index, sg-index)
define(`STREAM_PLAYBACK_MIXER',
`'
`SectionControlMixer."`MultiMedia'eval($1 + 1)" {'
`	reg "-1"'
`	index STR($1)'
`	max "1"'
`	invert "false"'
`'
`	channel."fl" {'
`                reg "-1"'
`    		max "1"'
`        }'
`'
`	ops."ctl" {'
`		get "SND_SOC_AR_TPLG_FE_BE_GRAPH_CTL_MIX"'
`		put "SND_SOC_AR_TPLG_FE_BE_GRAPH_CTL_MIX"'
`		info "1"'
`	}'
`'
`	data ['
`                "NAME_PREFIX.sub_graph$2_data"'
`	]'
`}')
dnl
dnl STREAM_MASTER_PLAYBACK_VOLUME_CTRL(stream-index)
define(`STREAM_MASTER_PLAYBACK_VOLUME_CTRL',
`'
`SectionTLV."master_playback_vol_ctrl_tlv" {'
`	scale {'
`		min "0"'
`		step "100"'
`		mute "0"'
`	}'
`}'
`SectionControlMixer."Master Playback Volume" {'
`	Comment "Stream Global volume"'
`'
`	# control belongs to this index group'
`	index "1"'
`'
`	# Channel register and shift for Front Left/Right'
`	channel."FL" {'
`		reg "0"'
`		shift "0"'
`	}'
`	channel."FR" {'
`		reg "0"'
`		shift "0"'
`	}'
`'
`	# max control value and whether value is inverted'
`	max "65535"'
`	invert "false"'
`'
`	# control uses bespoke driver get/put/info ID 0'
`	ops."ctl" {'
`		info "volsw"'
`		get "257"'
`		put "257"'
`	}'
`	tlv "master_playback_vol_ctrl_tlv"'
`}')
