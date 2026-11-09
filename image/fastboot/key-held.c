/*
 * Exits 0 while a volume key is held down. The early userspace asks it
 * before it starts the installed system: a held key stops the phone in
 * rescue, the way to install again on a phone whose system lets nobody in.
 */
#include <dirent.h>
#include <fcntl.h>
#include <linux/input.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

static int held(const unsigned char *keys, unsigned key)
{
	return keys[key / 8] >> (key % 8) & 1;
}

int main(void)
{
	DIR *dir = opendir("/dev/input");
	struct dirent *entry;

	if (!dir)
		return 1;
	while ((entry = readdir(dir))) {
		unsigned char keys[KEY_MAX / 8 + 1] = { 0 };
		char path[300];

		if (strncmp(entry->d_name, "event", 5))
			continue;
		snprintf(path, sizeof path, "/dev/input/%s", entry->d_name);
		int fd = open(path, O_RDONLY | O_CLOEXEC);
		if (fd < 0)
			continue;
		int got = ioctl(fd, EVIOCGKEY(sizeof keys), keys);
		close(fd);
		if (got >= 0 && (held(keys, KEY_VOLUMEUP) || held(keys, KEY_VOLUMEDOWN)))
			return 0;
	}
	return 1;
}
