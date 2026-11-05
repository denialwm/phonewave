/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
#ifndef _UAPI_LINUX_GOODIX_TEE_FP_H
#define _UAPI_LINUX_GOODIX_TEE_FP_H

#include <linux/ioctl.h>
#include <linux/types.h>

/* Power and reset numbers match the stock Goodix control ABI. No SPI or
 * biometric data passes through this node. Closing it removes power.
 * read() returns an interrupt sequence (__u32); poll() waits for changes.
 */
#define GOODIX_FP_IOC_RESET         _IO('g', 2)
#define GOODIX_FP_IOC_ENABLE_POWER  _IO('g', 7)
#define GOODIX_FP_IOC_DISABLE_POWER _IO('g', 8)

struct goodix_fp_state {
	__u32 version;
	__u32 powered;
	__u32 irq_sequence;
	__u32 reset_sequence;
	__s32 irq_level;
	__u32 reserved;
};

#define GOODIX_FP_IOC_GET_STATE _IOR('G', 0, struct goodix_fp_state)

#endif
