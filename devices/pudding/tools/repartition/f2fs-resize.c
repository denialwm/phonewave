/* Shrink a mounted f2fs to NEW_BLOCK_COUNT blocks: f2fs-resize MOUNTPOINT NEW_BLOCK_COUNT */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define F2FS_IOC_RESIZE_FS _IOW(0xf5, 16, uint64_t)

int main(int argc, char **argv)
{
	if (argc != 3) {
		fprintf(stderr, "usage: %s MOUNTPOINT NEW_BLOCK_COUNT\n", argv[0]);
		return 2;
	}
	uint64_t blocks = strtoull(argv[2], NULL, 10);
	int fd = open(argv[1], O_RDONLY | O_DIRECTORY);
	if (fd < 0) {
		fprintf(stderr, "open %s: %s\n", argv[1], strerror(errno));
		return 1;
	}
	if (ioctl(fd, F2FS_IOC_RESIZE_FS, &blocks) < 0) {
		fprintf(stderr, "F2FS_IOC_RESIZE_FS %llu: %s\n", (unsigned long long)blocks, strerror(errno));
		return 1;
	}
	printf("resized %s to %llu blocks\n", argv[1], (unsigned long long)blocks);
	return 0;
}
