# Copyright, Linaro Ltd, 2023
# SPDX-License-Identifier: BSD-3-Clause
dnl stream_device_rout: list the devices or streams from argument list
define(`stream_device_route',
`ifelse(`$#', `1', ,
`stream_device_route(shift($@))
		"DNAME, $2"')') dnl
dnl stream_device_rout: list the devices or streams from argument list
define(`device_stream_route',
`ifelse(`$#', `1', ,
`device_stream_route(shift($@))
		"SNAME, $2"')') dnl

define(`STREAM_DEVICE_PLAYBACK_ROUTE',
`'
`define(`DNAME', `$2')'
`SectionGraph."DNAME Graph" {'
`        index STR($1)'
`        lines ['
`		stream_device_route(shift($@))'
`        ]'
`}')
dnl STREAM_DEVICE_CAPTURE_ROUTE(stream-index)
define(`STREAM_DEVICE_CAPTURE_ROUTE',
`'
`define(`SNAME', `$2')'
`SectionGraph."SNAME Graph" {'
`        index STR($1)'
`        lines ['
`		device_stream_route(shift($@))'
`        ]'
`}')
