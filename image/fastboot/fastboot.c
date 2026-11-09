// SPDX-License-Identifier: GPL-2.0-only
/*
 * fastboot for the early userspace's rescue, on every phone: the way to
 * install the root. It serves the fastboot protocol on a USB FunctionFS
 * function beside the rescue's SSH, flashes only the root partitions,
 * denial and userdata, and the boot and init_boot of the slot it booted
 * from, from raw or sparse images, and draws on the bootloader's
 * framebuffer how to use it. "package" is no partition: it stores a pacman
 * package in the installed root, which installs it as it next starts. The
 * font is the kernel's Terminus 16x32, which image/init-boot takes from
 * linux-perpetua.
 *
 * Usage: fastboot FFS_DIR PRODUCT SERIAL REASON
 * Returns once the descriptors are written, so the caller can bind the
 * gadget; a child serves until the phone reboots.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/fb.h>
#include <linux/reboot.h>
#include <linux/usb/functionfs.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

_Static_assert(__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__, "USB descriptors are little endian");

static const unsigned char font[256 * 64] = {
#include "font.inc"
};

/* A package arrives whole: the client splits only what is larger. */
#define MAX_DOWNLOAD (1u << 30)
#define CHUNK (1u << 20)

static const char *const partitions[] = { "denial", "userdata" };
static const char *const slotted[] = { "boot", "init_boot" };
static const char *product, *serial, *reason;
/* The slot the bootloader started, "_b", or empty when it names none. */
static char slot[8];

/* The screen: the framebuffer the bootloader left scanning out. */
static struct {
	uint8_t *mem;
	struct fb_var_screeninfo var;
	struct fb_fix_screeninfo fix;
	unsigned scale;
} fb;

static uint32_t pixel(unsigned v)
{
	struct fb_var_screeninfo *s = &fb.var;
	return (v >> (8 - s->red.length)) << s->red.offset |
	       (v >> (8 - s->green.length)) << s->green.offset |
	       (v >> (8 - s->blue.length)) << s->blue.offset |
	       (s->transp.length ? (0xffu >> (8 - s->transp.length)) << s->transp.offset : 0);
}

static void fill(unsigned x0, unsigned y0, unsigned w, unsigned h, uint32_t value)
{
	unsigned bytes = fb.var.bits_per_pixel / 8;

	for (unsigned y = y0; y < y0 + h && y < fb.var.yres; y++) {
		uint8_t *row = fb.mem + (size_t)(y + fb.var.yoffset) * fb.fix.line_length;
		for (unsigned x = x0; x < x0 + w && x < fb.var.xres; x++)
			memcpy(row + (size_t)(x + fb.var.xoffset) * bytes, &value, bytes);
	}
}

static void text(unsigned line, unsigned scale, unsigned grey, const char *s)
{
	unsigned w = 16 * scale, h = 32 * scale;
	unsigned y0 = fb.var.yres / 8 + line * 40 * fb.scale, x0 = w;
	uint32_t on = pixel(grey), off = pixel(0);

	fill(0, y0, fb.var.xres, h, off);
	for (unsigned i = 0; s[i] && x0 + (i + 1) * w <= fb.var.xres; i++) {
		const unsigned char *glyph = font + (unsigned char)s[i] * 64;
		for (unsigned r = 0; r < 32; r++) {
			unsigned bits = glyph[2 * r] << 8 | glyph[2 * r + 1];
			for (unsigned c = 0; c < 16; c++)
				fill(x0 + i * w + c * scale, y0 + r * scale, scale, scale,
				     bits & (0x8000 >> c) ? on : off);
		}
	}
}

static void status(const char *fmt, ...)
{
	char s[64];
	va_list ap;

	va_start(ap, fmt);
	vsnprintf(s, sizeof s, fmt, ap);
	va_end(ap);
	if (fb.mem)
		text(12, fb.scale, 0xa0, s);
}

static void screen(void)
{
	char path[64], name[64];
	int fd = open("/dev/fb0", O_RDWR | O_CLOEXEC);

	if (fd < 0 || ioctl(fd, FBIOGET_VSCREENINFO, &fb.var) ||
	    ioctl(fd, FBIOGET_FSCREENINFO, &fb.fix) ||
	    (fb.var.bits_per_pixel != 16 && fb.var.bits_per_pixel != 24 &&
	     fb.var.bits_per_pixel != 32)) {
		if (fd >= 0)
			close(fd);
		return;
	}
	fb.mem = mmap(NULL, fb.fix.smem_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	close(fd);
	if (fb.mem == MAP_FAILED) {
		fb.mem = NULL;
		return;
	}
	/* The text console would draw over the message. */
	for (int i = 0; i < 8; i++) {
		snprintf(path, sizeof path, "/sys/class/vtconsole/vtcon%d/name", i);
		FILE *f = fopen(path, "r");
		if (!f)
			continue;
		if (fgets(name, sizeof name, f) && strstr(name, "frame buffer")) {
			snprintf(path, sizeof path, "/sys/class/vtconsole/vtcon%d/bind", i);
			FILE *b = fopen(path, "w");
			if (b) {
				fputs("0", b);
				fclose(b);
			}
		}
		fclose(f);
	}
	/* The commands fit on one line: 38 characters across. */
	fb.scale = fb.var.xres / (38 * 16) ? fb.var.xres / (38 * 16) : 1;
	fill(0, 0, fb.var.xres, fb.var.yres, pixel(0));
	text(0, fb.scale + 1, 0xff, "Denial OS");
	text(2, fb.scale, 0xff, product);
	text(3, fb.scale, 0xff, reason);
	text(5, fb.scale, 0xff, "Install over USB:");
	text(6, fb.scale, 0xff, "fastboot flash userdata userdata.img");
	text(7, fb.scale, 0xa0, "or, on a denial partition:");
	text(8, fb.scale, 0xff, "fastboot flash denial denial.img");
	text(10, fb.scale, 0xa0, "SSH: root@10.77.71.2");
	status("Waiting for fastboot");
}

/* The bootloader passes the slot as androidboot.slot_suffix, in bootconfig
 * or on the command line. */
static void find_slot(void)
{
	char line[4096];
	FILE *f = fopen("/proc/bootconfig", "r");

	while (f && fgets(line, sizeof line, f))
		if (sscanf(line, "androidboot.slot_suffix = \"%7[_a-z]\"", slot) == 1)
			break;
	if (f)
		fclose(f);
	f = slot[0] ? NULL : fopen("/proc/cmdline", "r");
	if (f && fgets(line, sizeof line, f)) {
		char *s = strstr(line, "androidboot.slot_suffix=");
		if (s)
			sscanf(s + 24, "%7[_a-z]", slot);
	}
	if (f)
		fclose(f);
	if (slot[0] != '_' || !slot[1] || slot[2])
		slot[0] = 0;
}

/* The partition a name writes: denial and userdata as they are, boot and
 * init_boot on this slot, with or without its suffix. */
static int resolve(const char *name, char *part, size_t len)
{
	char suffixed[64];

	for (unsigned i = 0; i < sizeof partitions / sizeof *partitions; i++)
		if (!strcmp(name, partitions[i]))
			return snprintf(part, len, "%s", name), 0;
	for (unsigned i = 0; slot[0] && i < sizeof slotted / sizeof *slotted; i++) {
		snprintf(suffixed, sizeof suffixed, "%s%s", slotted[i], slot);
		if (!strcmp(name, slotted[i]) || !strcmp(name, suffixed))
			return snprintf(part, len, "%s", suffixed), 0;
	}
	return -1;
}

/* The partitions this fastboot writes, by name, as the rescue finds them. */
static int partition(const char *name, char *dev, size_t len, uint64_t *size)
{
	char path[300], line[128], part[64];
	unsigned long long sectors = 0;
	int found = 0;

	if (resolve(name, part, sizeof part))
		return -1;
	name = part;
	DIR *dir = opendir("/sys/class/block");
	if (!dir)
		return -1;
	found = 0;
	for (struct dirent *e; !found && (e = readdir(dir));) {
		snprintf(path, sizeof path, "/sys/class/block/%s/uevent", e->d_name);
		FILE *f = fopen(path, "r");
		if (!f)
			continue;
		while (!found && fgets(line, sizeof line, f)) {
			line[strcspn(line, "\n")] = 0;
			found = !strncmp(line, "PARTNAME=", 9) && !strcmp(line + 9, name);
		}
		fclose(f);
		if (found) {
			snprintf(dev, len, "/dev/%s", e->d_name);
			snprintf(path, sizeof path, "/sys/class/block/%s/size", e->d_name);
		}
	}
	closedir(dir);
	FILE *f = found ? fopen(path, "r") : NULL;
	if (!f)
		return -1;
	found = fscanf(f, "%llu", &sectors) == 1;
	fclose(f);
	*size = sectors * 512;
	return found ? 0 : -1;
}

/* A root the rescue mounted read-only goes before it is written. */
static int unmount(const char *dev)
{
	char source[256], target[256];
	int ret = 0;
	FILE *f = fopen("/proc/mounts", "r");

	if (!f)
		return -1;
	while (fscanf(f, "%255s %255s %*[^\n]", source, target) == 2)
		if (!strcmp(source, dev) && umount(target))
			ret = -1;
	fclose(f);
	return ret;
}

static int write_all(int fd, const uint8_t *p, uint64_t len, uint64_t off)
{
	while (len) {
		ssize_t n = pwrite(fd, p, len > CHUNK ? CHUNK : len, off);
		if (n <= 0)
			return -1;
		p += n, len -= n, off += n;
	}
	return 0;
}

struct sparse_header {
	uint32_t magic;
	uint16_t major, minor, file_hdr_sz, chunk_hdr_sz;
	uint32_t blk_sz, total_blks, total_chunks, image_checksum;
} __attribute__((packed));

struct chunk_header {
	uint16_t type, reserved;
	uint32_t chunk_sz, total_sz;
} __attribute__((packed));

/* Android's sparse format, as the client splits large images. */
static const char *write_sparse(int fd, const uint8_t *buf, uint32_t len, uint64_t size)
{
	struct sparse_header h;
	static uint8_t pattern[CHUNK];

	memcpy(&h, buf, sizeof h);
	if (h.major != 1 || h.file_hdr_sz < sizeof h || h.chunk_hdr_sz < sizeof(struct chunk_header) ||
	    !h.blk_sz || h.blk_sz % 4 || (uint64_t)h.total_blks * h.blk_sz > size)
		return "bad sparse header";
	const uint8_t *p = buf + h.file_hdr_sz, *end = buf + len;
	uint64_t off = 0, total = (uint64_t)h.total_blks * h.blk_sz;
	for (uint32_t i = 0; i < h.total_chunks; i++) {
		struct chunk_header c;
		if (p > end || (size_t)(end - p) < h.chunk_hdr_sz)
			return "truncated sparse image";
		memcpy(&c, p, sizeof c);
		const uint8_t *data = p + h.chunk_hdr_sz;
		uint64_t data_len = (uint64_t)c.total_sz - h.chunk_hdr_sz, bytes = (uint64_t)c.chunk_sz * h.blk_sz;
		if (c.total_sz < h.chunk_hdr_sz || data_len > (uint64_t)(end - data))
			return "truncated sparse chunk";
		if (c.type != 0xcac4 && bytes > total - off)
			return "sparse chunk past the image";
		switch (c.type) {
		case 0xcac1: /* raw */
			if (data_len != bytes || write_all(fd, data, bytes, off))
				return "write failed";
			break;
		case 0xcac2: /* fill */
			if (data_len != 4)
				return "bad fill chunk";
			for (size_t j = 0; j < sizeof pattern; j += 4)
				memcpy(pattern + j, data, 4);
			for (uint64_t done = 0; done < bytes;) {
				uint64_t n = bytes - done > sizeof pattern ? sizeof pattern : bytes - done;
				if (write_all(fd, pattern, n, off + done))
					return "write failed";
				done += n;
			}
			break;
		case 0xcac3: /* don't care */
			if (data_len)
				return "bad skip chunk";
			break;
		case 0xcac4: /* crc32 */
			bytes = 0;
			break;
		default:
			return "unknown sparse chunk";
		}
		off += bytes;
		p = data + data_len;
	}
	return NULL;
}

static int in, out;

static void reply(const char *fmt, ...)
{
	char s[65];
	va_list ap;

	va_start(ap, fmt);
	int n = vsnprintf(s, sizeof s, fmt, ap);
	va_end(ap);
	if (write(in, s, n > 64 ? 64 : n) < 0)
		perror("fastboot: reply");
}

/* denial-install tells the flash script it reached this fastboot. */
static int value(const char *name, char *s, size_t len)
{
	char dev[300];
	uint64_t size;

	if (!strcmp(name, "version"))
		snprintf(s, len, "0.5");
	else if (!strcmp(name, "product"))
		snprintf(s, len, "%s", product);
	else if (!strcmp(name, "serialno"))
		snprintf(s, len, "%s", serial);
	else if (!strcmp(name, "max-download-size"))
		snprintf(s, len, "0x%08x", MAX_DOWNLOAD);
	else if (!strcmp(name, "denial-install"))
		snprintf(s, len, "yes");
	else if (!strcmp(name, "current-slot") && slot[0])
		snprintf(s, len, "%s", slot + 1);
	else if (!strncmp(name, "has-slot:", 9)) {
		int has = 0;
		for (unsigned i = 0; slot[0] && i < sizeof slotted / sizeof *slotted; i++)
			has |= !strcmp(name + 9, slotted[i]);
		snprintf(s, len, "%s", has ? "yes" : "no");
	} else if (!strcmp(name, "is-userspace") || !strncmp(name, "is-logical:", 11))
		snprintf(s, len, "no");
	else if (!strncmp(name, "partition-size:", 15) &&
		 !partition(name + 15, dev, sizeof dev, &size))
		snprintf(s, len, "0x%llx", (unsigned long long)size);
	else if (!strncmp(name, "partition-type:", 15) &&
		 !partition(name + 15, dev, sizeof dev, &size))
		snprintf(s, len, "raw");
	else
		return -1;
	return 0;
}

static void getvar(const char *name)
{
	static const char *const all[] = { "version", "product", "serialno",
		"max-download-size", "denial-install", "is-userspace", "current-slot" };
	static const char *const per_partition[] = { "partition-size:", "partition-type:" };
	const char *names[] = { partitions[0], partitions[1], slotted[0], slotted[1] };
	char s[64], var[64];

	if (strcmp(name, "all")) {
		if (value(name, s, sizeof s))
			reply("FAILunknown variable");
		else
			reply("OKAY%s", s);
		return;
	}
	for (unsigned i = 0; i < sizeof all / sizeof *all; i++)
		if (!value(all[i], s, sizeof s))
			reply("INFO%s:%s", all[i], s);
	for (unsigned i = 0; i < sizeof names / sizeof *names; i++)
		for (unsigned j = 0; j < sizeof per_partition / sizeof *per_partition; j++) {
			snprintf(var, sizeof var, "%s%s", per_partition[j], names[i]);
			if (!value(var, s, sizeof s))
				reply("INFO%s:%s", var, s);
		}
	reply("OKAY");
}

/* Mounts the installed root, the one the early userspace would boot, at
 * /mnt/root for writing. */
static int mount_root(void)
{
	char dev[300];
	uint64_t size;

	mkdir("/mnt", 0755);
	mkdir("/mnt/root", 0755);
	for (unsigned i = 0; i < sizeof partitions / sizeof *partitions; i++) {
		if (partition(partitions[i], dev, sizeof dev, &size) || unmount(dev) ||
		    mount(dev, "/mnt/root", "ext4", MS_RDONLY, NULL))
			continue;
		if (!access("/mnt/root/usr/lib/phonewave-system/root", F_OK) &&
		    !mount(NULL, "/mnt/root", NULL, MS_REMOUNT, NULL))
			return 0;
		umount("/mnt/root");
	}
	return -1;
}

/* A pacman package for the installed system: kept in the root, where
 * phonewave-system's install-flashed installs it as the phone starts. */
static void store_package(const uint8_t *buf, uint32_t len)
{
	static const uint8_t zstd[] = { 0x28, 0xb5, 0x2f, 0xfd };
	static const char *const dirs[] = { "/mnt/root/var/lib/phonewave-system",
		"/mnt/root/var/lib/phonewave-system/flashed" };
	char path[128];
	int fd = -1;

	if (len < sizeof zstd || memcmp(buf, zstd, sizeof zstd)) {
		reply("FAILnot a .pkg.tar.zst package");
		return;
	}
	if (mount_root()) {
		reply("FAILno system installed: flash the image first");
		return;
	}
	status("Storing the package");
	for (unsigned i = 0; i < sizeof dirs / sizeof *dirs; i++)
		mkdir(dirs[i], 0755);
	for (unsigned n = 0; fd < 0 && n < 100; n++) {
		snprintf(path, sizeof path, "%s/%02u.pkg.tar.zst", dirs[1], n);
		fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0644);
	}
	int failed = fd < 0 || write_all(fd, buf, len, 0) || fsync(fd);
	if (fd >= 0)
		close(fd);
	if (failed && fd >= 0)
		unlink(path);
	failed |= umount("/mnt/root");
	if (failed) {
		status("package: write failed");
		reply("FAILwrite failed");
	} else {
		status("Stored the package");
		reply("OKAY");
	}
}

static void flash(const char *name, const uint8_t *buf, uint32_t len)
{
	char dev[300], part[64];
	uint64_t size;
	const char *error = NULL;

	if (!strcmp(name, "package")) {
		store_package(buf, len);
		return;
	}
	if (resolve(name, part, sizeof part) || partition(part, dev, sizeof dev, &size)) {
		reply("FAILonly denial, userdata, package, boot and init_boot");
		return;
	}
	name = part;
	if (unmount(dev)) {
		reply("FAIL%s is in use", name);
		return;
	}
	int fd = open(dev, O_WRONLY | O_CLOEXEC);
	if (fd < 0) {
		reply("FAILcannot open %s", name);
		return;
	}
	status("Writing %s", name);
	if (len >= sizeof(struct sparse_header) && *(const uint32_t *)buf == 0xed26ff3a)
		error = write_sparse(fd, buf, len, size);
	else if (len > size)
		error = "image larger than the partition";
	else if (write_all(fd, buf, len, 0))
		error = "write failed";
	if (fsync(fd) && !error)
		error = "write failed";
	close(fd);
	if (error) {
		status("%s: %s", name, error);
		reply("FAIL%s", error);
	} else {
		status("Wrote %s", name);
		reply("OKAY");
	}
}

static void serve(void)
{
	static char cmd[4096];
	uint8_t *buf = mmap(NULL, MAX_DOWNLOAD, PROT_READ | PROT_WRITE,
			    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	uint32_t have = 0;

	if (buf == MAP_FAILED)
		exit(1);
	for (;;) {
		/* Blocks while the cable is out. */
		ssize_t n = read(out, cmd, sizeof cmd - 1);
		if (n <= 0) {
			if (n < 0 && errno != ESHUTDOWN && errno != EINTR)
				usleep(100000);
			continue;
		}
		cmd[n] = 0;
		if (!strncmp(cmd, "getvar:", 7)) {
			getvar(cmd + 7);
		} else if (!strncmp(cmd, "download:", 9)) {
			unsigned long want = strtoul(cmd + 9, NULL, 16);
			if (!want || want > MAX_DOWNLOAD) {
				reply("FAILdownload size");
				continue;
			}
			reply("DATA%08lx", want);
			status("Receiving %lu MiB", want >> 20);
			have = 0;
			while (have < want) {
				ssize_t r = read(out, buf + have, want - have > CHUNK ? CHUNK : want - have);
				if (r <= 0)
					break;
				have += r;
			}
			if (have == want)
				reply("OKAY");
			else
				have = 0;
		} else if (!strncmp(cmd, "flash:", 6)) {
			if (have)
				flash(cmd + 6, buf, have);
			else
				reply("FAILnothing downloaded");
		} else if (!strcmp(cmd, "reboot") || !strcmp(cmd, "reboot-bootloader")) {
			reply("OKAY");
			status("Rebooting");
			sync();
			if (cmd[6])
				syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
					LINUX_REBOOT_CMD_RESTART2, "bootloader");
			reboot(RB_AUTOBOOT);
		} else {
			reply("FAILunknown command");
		}
	}
}

/* ep0: the gadget's events. Requests to the interface are stalled. */
static void *control(void *arg)
{
	int ep0 = *(int *)arg;
	struct usb_functionfs_event events[4];

	for (;;) {
		ssize_t n = read(ep0, events, sizeof events);
		if (n < 0) {
			if (errno != EINTR)
				usleep(100000);
			continue;
		}
		for (size_t i = 0; i < n / sizeof *events; i++)
			if (events[i].type == FUNCTIONFS_SETUP) {
				if (events[i].u.setup.bRequestType & USB_DIR_IN)
					(void)!read(ep0, NULL, 0);
				else
					(void)!write(ep0, NULL, 0);
			}
	}
	return NULL;
}

#define INTERFACE { \
	.bLength = USB_DT_INTERFACE_SIZE, .bDescriptorType = USB_DT_INTERFACE, \
	.bNumEndpoints = 2, .bInterfaceClass = 0xff, .bInterfaceSubClass = 0x42, \
	.bInterfaceProtocol = 0x03, .iInterface = 1 }
#define ENDPOINT(address, size) { \
	.bLength = USB_DT_ENDPOINT_SIZE, .bDescriptorType = USB_DT_ENDPOINT, \
	.bEndpointAddress = (address), .bmAttributes = USB_ENDPOINT_XFER_BULK, \
	.wMaxPacketSize = (size) }
#define COMPANION { .bLength = USB_DT_SS_EP_COMP_SIZE, .bDescriptorType = USB_DT_SS_ENDPOINT_COMP }

static const struct {
	struct usb_functionfs_descs_head_v2 header;
	__le32 fs_count, hs_count, ss_count;
	struct {
		struct usb_interface_descriptor interface;
		struct usb_endpoint_descriptor_no_audio out, in;
	} __attribute__((packed)) fs, hs;
	struct {
		struct usb_interface_descriptor interface;
		struct usb_endpoint_descriptor_no_audio out;
		struct usb_ss_ep_comp_descriptor out_companion;
		struct usb_endpoint_descriptor_no_audio in;
		struct usb_ss_ep_comp_descriptor in_companion;
	} __attribute__((packed)) ss;
} __attribute__((packed)) descriptors = {
	.header = {
		.magic = FUNCTIONFS_DESCRIPTORS_MAGIC_V2,
		.length = sizeof descriptors,
		.flags = FUNCTIONFS_HAS_FS_DESC | FUNCTIONFS_HAS_HS_DESC | FUNCTIONFS_HAS_SS_DESC,
	},
	.fs_count = 3, .hs_count = 3, .ss_count = 5,
	.fs = { INTERFACE, ENDPOINT(1 | USB_DIR_OUT, 64), ENDPOINT(2 | USB_DIR_IN, 64) },
	.hs = { INTERFACE, ENDPOINT(1 | USB_DIR_OUT, 512), ENDPOINT(2 | USB_DIR_IN, 512) },
	.ss = { INTERFACE, ENDPOINT(1 | USB_DIR_OUT, 1024), COMPANION,
		ENDPOINT(2 | USB_DIR_IN, 1024), COMPANION },
};

static const struct {
	struct usb_functionfs_strings_head header;
	struct {
		__le16 code;
		char name[sizeof "fastboot"];
	} __attribute__((packed)) lang;
} __attribute__((packed)) strings = {
	.header = { .magic = FUNCTIONFS_STRINGS_MAGIC, .length = sizeof strings,
		    .str_count = 1, .lang_count = 1 },
	.lang = { 0x0409, "fastboot" },
};

int main(int argc, char **argv)
{
	char path[256];
	static int ep0;
	pthread_t thread;

	if (argc != 5) {
		fprintf(stderr, "usage: fastboot FFS_DIR PRODUCT SERIAL REASON\n");
		return 2;
	}
	product = argv[2], serial = argv[3], reason = argv[4];
	find_slot();
	snprintf(path, sizeof path, "%s/ep0", argv[1]);
	ep0 = open(path, O_RDWR | O_CLOEXEC);
	if (ep0 < 0 || write(ep0, &descriptors, sizeof descriptors) != sizeof descriptors ||
	    write(ep0, &strings, sizeof strings) != sizeof strings) {
		perror("fastboot: descriptors");
		return 1;
	}
	snprintf(path, sizeof path, "%s/ep1", argv[1]);
	out = open(path, O_RDONLY | O_CLOEXEC);
	snprintf(path, sizeof path, "%s/ep2", argv[1]);
	in = open(path, O_WRONLY | O_CLOEXEC);
	if (out < 0 || in < 0) {
		perror("fastboot: endpoints");
		return 1;
	}
	/* Ready: the caller binds the gadget while the child serves. */
	pid_t pid = fork();
	if (pid < 0)
		return 1;
	if (pid)
		return 0;
	setsid();
	screen();
	if (pthread_create(&thread, NULL, control, &ep0))
		return 1;
	serve();
}
