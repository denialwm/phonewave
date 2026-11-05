/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
#ifndef _UAPI_LINUX_UFF_FP_H
#define _UAPI_LINUX_UFF_FP_H

#include <linux/ioctl.h>
#include <linux/types.h>

/* Supported subset of the factory UFF control ABI. Other commands fail.
 * read() returns a native __u32 interrupt sequence; poll() waits for a change.
 * No biometric data or trusted application commands pass through this node.
 */
#define UFF_FP_IOC_RESET         _IO('O', 2)
#define UFF_FP_IOC_ENABLE_POWER  _IO('O', 7)
#define UFF_FP_IOC_DISABLE_POWER _IO('O', 8)
#define UFF_FP_IOC_RD_IRQ_VALUE  _IOW('O', 13, __s32)

/* Native retained standby. Root must first put the sensor to sleep through
 * the signed TA. This preserves supply/reset state and gates its IRQ. Final
 * close still removes power. An active, non-standby sensor blocks suspend.
 */
#define UFF_FP_IOC_SET_STANDBY _IOW('U', 1, __u32)
struct uff_fp_state {
	__u32 version;
	__u32 powered;
	__u32 standby;
	__u32 reset_sequence;
};
#define UFF_FP_IOC_GET_STATE _IOR('U', 2, struct uff_fp_state)

#endif
