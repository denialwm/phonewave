// SPDX-License-Identifier: MIT
/*
 * reboot-to MODE: restart with MODE as the reboot command, which the
 * kernel's reboot-mode driver leaves for the bootloader ("bootloader" stops
 * in fastboot). Busybox reboot cannot pass one.
 */
#include <linux/reboot.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(int argc, char **argv)
{
	if (argc != 2) {
		fprintf(stderr, "usage: reboot-to MODE\n");
		return 2;
	}
	sync();
	syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
		LINUX_REBOOT_CMD_RESTART2, argv[1]);
	perror("reboot");
	return 1;
}
