/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
#ifndef _UAPI_LINUX_ASTON_FP_ILLUMINATION_H
#define _UAPI_LINUX_ASTON_FP_ILLUMINATION_H

#include <linux/ioctl.h>
#include <linux/types.h>

/* One exclusive, transferable fd controls only the AA551 fingerprint region.
 * No DSI payloads, global brightness control or biometric data are exposed.
 * Enable requires an already powered display; final close disables the region.
 */
struct aston_fp_illumination_info {
	__u32 version;
	__u32 panel_width;
	__u32 panel_height;
	__u32 max_duration_ms;
	__u32 enabled;
	__u32 panel_on;
	char compatible[40];
};
#define ASTON_FP_ILLUMINATION_GET_INFO _IOR('a', 0, struct aston_fp_illumination_info)
#define ASTON_FP_ILLUMINATION_SET _IOW('a', 1, __u32)

#endif
