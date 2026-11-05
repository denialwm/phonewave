/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
#ifndef _UAPI_LINUX_JIIOV_TEE_FP_H
#define _UAPI_LINUX_JIIOV_TEE_FP_H

#include <linux/ioctl.h>
#include <linux/types.h>

/* Reset and power numbers match the stock anc_fps_mmi control ABI. No SPI
 * or biometric data passes through this node. Closing it removes power.
 * read() returns an interrupt sequence (__u32); poll() waits for changes.
 */
#define JIIOV_FP_IOC_RESET         _IO('a', 0)
#define JIIOV_FP_IOC_ENABLE_POWER  _IO('a', 11)
#define JIIOV_FP_IOC_DISABLE_POWER _IO('a', 12)

struct jiiov_fp_state {
	__u32 version;
	__u32 powered;
	__u32 irq_sequence;
	__u32 reset_sequence;
	__s32 irq_level;
	__u32 reserved;
};

#define JIIOV_FP_IOC_GET_STATE _IOR('J', 0, struct jiiov_fp_state)

#endif
