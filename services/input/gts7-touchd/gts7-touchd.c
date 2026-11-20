// SPDX-License-Identifier: GPL-2.0-only
/*
 * gts7-touchd: the touchscreen of the Galaxy Tab S7 FE (gts7xllite). Its FocalTech FT8203 (touch and display
 * driver in one chip) sits on spi@980000 with no kernel driver. This binds spidev to it, downloads the firmware into
 * the chip's RAM when it sits in its boot ROM, then reads one frame per gpio22 edge and feeds them to uinput as a
 * protocol B multitouch screen. It returns only when it gives up (no SPI device, an unknown chip); systemd restarts
 * it. Messages go to stdout, the journal. No libc: raw arm64 syscalls.
 */
typedef unsigned long ul;
typedef long sl;

#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_WRONLY 1
#define O_NONBLOCK 04000
#define O_NOCTTY 0400
#define O_DIRECTORY 040000	/* arm64 value, not the x86 one */

static sl sc(sl n, sl a, sl b, sl c, sl d, sl e)
{
	register sl x8 __asm__("x8") = n;
	register sl x0 __asm__("x0") = a;
	register sl x1 __asm__("x1") = b;
	register sl x2 __asm__("x2") = c;
	register sl x3 __asm__("x3") = d;
	register sl x4 __asm__("x4") = e;

	__asm__ volatile("svc #0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4) : "memory");
	return x0;
}

static sl sys_write(int fd, const void *b, ul n) { return sc(64, fd, (sl)b, n, 0, 0); }
static sl sys_read(int fd, void *b, ul n) { return sc(63, fd, (sl)b, n, 0, 0); }
static int sys_openat(const char *p, int fl) { return sc(56, AT_FDCWD, (sl)p, fl, 0, 0); }
static void sys_close(int fd) { sc(57, fd, 0, 0, 0, 0); }
void *memset(void *d, int c, ul n) { unsigned char *p = d; while (n--) *p++ = c; return d; }
void *memcpy(void *d, const void *s, ul n) { unsigned char *p = d; const unsigned char *q = s; while (n--) *p++ = *q++; return d; }
static ul slen(const char *s) { ul n = 0; while (s[n]) n++; return n; }
static int memcmp_(const void *a, const void *b, ul n) { const unsigned char *p = a, *q = b; while (n--) { if (*p != *q) return *p - *q; p++; q++; } return 0; }

static int out[2] = { 1, -1 };	/* stdout; a second descriptor is unused here */

static void put(const char *s)
{
	for (int i = 0; i < 2; i++)
		if (out[i] >= 0)
			sys_write(out[i], s, slen(s));
}

static void putnum(long v)
{
	char b[24];
	int i = 23;

	b[i] = 0;
	if (v < 0) { put("-"); v = -v; }
	do { b[--i] = '0' + v % 10; v /= 10; } while (v);
	put(b + i);
}

static long wr(const char *path, const char *val)
{
	int fd = sys_openat(path, O_WRONLY);
	long r;

	if (fd < 0)
		return fd;
	r = sys_write(fd, val, slen(val));
	sys_close(fd);
	return r < 0 ? r : 0;
}

#define O_RDWR 2
#define I2C_RDWR 0x0707
#define I2C_M_RD 1

struct i2c_msg { unsigned short addr, flags, len; unsigned char *buf; };
struct i2c_rdwr_data { struct i2c_msg *msgs; unsigned int nmsgs; };

static sl sys_ioctl(int fd, ul req, void *arg) { return sc(29, fd, req, (sl)arg, 0, 0); }
static sl sys_readlink(const char *p, char *b, ul n) { return sc(78, AT_FDCWD, (sl)p, (sl)b, n, 0); }

static void puthex(unsigned v)
{
	char b[3] = { "0123456789abcdef"[(v >> 4) & 15], "0123456789abcdef"[v & 15], 0 };

	put(b);
}

static int has(const char *h, const char *n)
{
	for (; *h; h++) {
		int i = 0;

		while (n[i] && h[i] == n[i])
			i++;
		if (!n[i])
			return 1;
	}
	return 0;
}

/* Opens the i2c-dev node of controller 984000.i2c, the bus of the SM5714 chips. */
static void dump_file(const char *path, const char *const *keys);

/*
 * The protocol is Samsung's ft820x/focaltech_spi.c: one full-duplex transfer { reg, 0xA0 (read + CRC), len>>8, len,
 * 3 dummy, len zero bytes, 2 CRC bytes }; the reply has a status byte at [3] (bits 0xA0 must be clear), the data at
 * [7..], then CRC16 (crc16k) little endian. The reset and irq gpios are never driven (resetting a TDDI can blank the
 * panel). Writes go only to the chip's RAM (the firmware download): it has no flash path here.
 */
#define SPI_IOC_WR_MODE 0x40016b01
#define SPI_IOC_WR_BITS 0x40016b03
#define SPI_IOC_WR_SPEED 0x40046b04
#define SPI_IOC_MSG1 0x40206b00

struct spi_xfer {
	unsigned long long tx, rx;
	unsigned int len, speed;
	unsigned short delay;
	unsigned char bits, cs_change, tx_nbits, rx_nbits, word_delay, pad;
};

static void sleep_ms(long ms)
{
	long ts[2] = { ms / 1000, (ms % 1000) * 1000000 };

	sc(101, (sl)ts, 0, 0, 0, 0);
}

/* Binds spidev to our touch device, and only to that one (it must sit on 980000.spi). */
static int touch_open(void)
{
	char l[160], node[24] = "/dev/spidev0.0";

	for (int n = 0; n < 10; n++) {
		char path[80] = "/sys/bus/spi/devices/spiN.0";

		path[24] = '0' + n;
		long r = sys_readlink(path, l, sizeof(l) - 1);

		if (r <= 0)
			continue;
		l[r] = 0;
		if (!has(l, "980000.spi"))
			continue;
		char ovr[96] = "/sys/bus/spi/devices/spiN.0/driver_override";
		char nm[8] = "spiN.0";

		ovr[24] = '0' + n;
		nm[3] = '0' + n;
		if (wr(ovr, "spidev") < 0)
			return -2;
		wr("/sys/bus/spi/drivers/spidev/bind", nm);	/* already bound is fine */
		node[11] = '0' + n;
		for (int t = 0; t < 20; t++) {
			int fd = sys_openat(node, O_RDWR);

			if (fd >= 0)
				return fd;
			sleep_ms(100);
		}
		return -3;
	}
	return -1;
}

/* Samsung's "crckermit": init 0xFFFF, poly 0x8408 reflected = CRC-16/MCRF4XX (check value of "123456789" is 0x6F91), NOT true KERMIT. */
static unsigned int crc16k(const unsigned char *d, int n)
{
	unsigned int c = 0xFFFF;

	while (n--) {
		c ^= *d++;
		for (int j = 0; j < 8; j++)
			c = (c & 1) ? (c >> 1) ^ 0x8408 : c >> 1;
	}
	return c & 0xFFFF;
}

/* Reads n (<= 80) registers starting at reg. 0 = ok, <0 = failed after 3 tries. */
static int ft_read(int fd, unsigned char reg, unsigned char *dst, int n)
{
	unsigned char tx[96], rx[96];
	int len = 7 + n + 2;
	struct spi_xfer x;

	for (int t = 0; t < 3; t++) {
		memset(tx, 0, sizeof(tx));
		memset(rx, 0, sizeof(rx));
		memset(&x, 0, sizeof(x));
		tx[0] = reg;
		tx[1] = 0xA0;
		tx[2] = n >> 8;
		tx[3] = n;
		x.tx = (unsigned long)tx;
		x.rx = (unsigned long)rx;
		x.len = len;
		x.speed = 6000000;
		x.bits = 8;
		if (sys_ioctl(fd, SPI_IOC_MSG1, &x) >= 0 && !(rx[3] & 0xA0) &&
		    crc16k(rx + 7, n) == (unsigned int)(rx[7 + n] | rx[8 + n] << 8)) {
			memcpy(dst, rx + 7, n);
			return 0;
		}
		sleep_ms(1);	/* the chip wants 150 us between transfers */
	}
	return -5;
}

/* uinput: a protocol B multitouch screen, 10 slots, 1600 x 2560 (rev7 tree: focaltech display-coords). */
struct ui_abs { unsigned short code; unsigned short pad; int val, min, max, fuzz, flat, res; };
struct ui_ev { long sec, usec; unsigned short type, code; int value; };

static int ui_fd = -1;

static void ui_abs_setup(int code, int max)
{
	struct ui_abs a = { code, 0, 0, 0, max, 0, 0, 0 };

	sys_ioctl(ui_fd, 0x401c5504, &a);
}

static void ui_create(void)
{
	static unsigned char setup[92];

	ui_fd = sys_openat("/dev/uinput", O_WRONLY | O_NONBLOCK);
	if (ui_fd < 0) {
		put("touch: no /dev/uinput, printing only\n");
		return;
	}
	/* UI_SET_*BIT take the bit number as the VALUE of the argument, not a pointer (passing &ev made every call fail EINVAL) */
	sys_ioctl(ui_fd, 0x40045564, (void *)3L);	/* UI_SET_EVBIT EV_ABS */
	sys_ioctl(ui_fd, 0x40045564, (void *)1L);	/* EV_KEY */
	sys_ioctl(ui_fd, 0x40045565, (void *)0x14aL);	/* UI_SET_KEYBIT BTN_TOUCH */
	sys_ioctl(ui_fd, 0x4004556e, (void *)1L);	/* UI_SET_PROPBIT INPUT_PROP_DIRECT */
	sys_ioctl(ui_fd, 0x40045567, (void *)0x2fL);	/* UI_SET_ABSBIT ABS_MT_SLOT */
	sys_ioctl(ui_fd, 0x40045567, (void *)0x39L);	/* ABS_MT_TRACKING_ID */
	sys_ioctl(ui_fd, 0x40045567, (void *)0x35L);	/* ABS_MT_POSITION_X */
	sys_ioctl(ui_fd, 0x40045567, (void *)0x36L);	/* ABS_MT_POSITION_Y */
	ui_abs_setup(0x2f, 9);
	ui_abs_setup(0x39, 65535);
	ui_abs_setup(0x35, 1599);
	ui_abs_setup(0x36, 2559);
	setup[0] = 3;	/* bustype BUS_USB is 3; value is cosmetic */
	setup[2] = 0x4d; setup[3] = 0x50;	/* vendor/product, cosmetic */
	{
		const char *nm = "gts7xllite touch";

		for (int i = 0; nm[i]; i++)
			setup[8 + i] = nm[i];
	}
	if (sys_ioctl(ui_fd, 0x405c5503, setup) < 0)
		put("touch: uinput setup failed\n");
	if (sys_ioctl(ui_fd, 0x5501, 0) < 0) {
		put("touch: uinput create failed\n");
		sys_close(ui_fd);
		ui_fd = -1;
	}
}

static void ui_send(int type, int code, int value)
{
	struct ui_ev e = { 0, 0, type, code, value };

	if (ui_fd >= 0)
		sys_write(ui_fd, &e, sizeof(e));
}

/*
 * Frames and checksum of the boot ROM download, ported from Samsung's ft820x/focaltech_spi.c (fts_write) and
 * focaltech_flash.c. Pure functions (no I/O) so initramfs/test-fw.py can check them on the host.
 */
#define FT_CHUNK 4088	/* spidev moves at most 4096 bytes per transfer: 7 header bytes + 4088 data */

/* Write frame: { cmd, 0x00 (write), n>>8, n, 3 dummy, n data bytes }; n is the data length WITHOUT the command byte. */
static int ft_frame_write(unsigned char *tx, unsigned char cmd, const unsigned char *d, int n)
{
	int len = 4;

	memset(tx, 0, 7);
	tx[0] = cmd;
	tx[1] = 0x00;
	tx[2] = n >> 8;
	tx[3] = n;
	if (n > 0) {
		len = 7 + n;
		memcpy(tx + 7, d, n);
	}
	return len;
}

/* Samsung's fts_ecc_cal_host: 16-bit big-endian words, polynomial AL2_FCS_COEF = 0x8408. */
static unsigned int ft_ecc(const unsigned char *d, unsigned int n)
{
	unsigned int ecc = 0;

	for (unsigned int i = 0; i < n; i += 2) {
		ecc ^= (d[i] << 8) | d[i + 1];
		for (int j = 0; j < 16; j++)
			ecc = (ecc & 1) ? (ecc >> 1) ^ 0x8408 : ecc >> 1;
		ecc &= 0xFFFF;
	}
	return ecc;
}

/* Diagnostic after a failed id read: the raw reply to the same READ frame (reg 0xA3, 1 byte) in all four SPI modes. Read frames only. */
static void touch_diag(int fd)
{
	static const unsigned int sp[2] = { 1000000, 6000000 };

	for (int m = 0; m < 4; m++) {
		for (int k = 0; k < 2; k++) {
			unsigned char tx[16] = { 0xA3, 0xA0, 0, 1 }, rx[16] = { 0 }, mode = m;
			struct spi_xfer x;
			long r;

			memset(&x, 0, sizeof(x));
			sys_ioctl(fd, SPI_IOC_WR_MODE, &mode);
			x.tx = (unsigned long)tx;
			x.rx = (unsigned long)rx;
			x.len = 10;
			x.speed = sp[k];
			x.bits = 8;
			r = sys_ioctl(fd, SPI_IOC_MSG1, &x);
			put("touch: diag mode ");
			putnum(m);
			put(" ");
			putnum(sp[k] / 1000);
			put(" kHz ioctl ");
			putnum(r);
			put(" rx");
			for (int i = 0; i < 10; i++) {
				put(" ");
				puthex(rx[i]);
			}
			put("\n");
			sleep_ms(2);
		}
	}
}

/* One write transfer to the chip, 3 tries like Samsung's fts_write (status byte bits 0xA0 must be clear). */
static int ft_write(int fd, unsigned char cmd, const unsigned char *d, int n)
{
	static unsigned char tx[4112], rx[4112];
	struct spi_xfer x;
	int len;

	if (n > FT_CHUNK)
		return -90;
	for (int t = 0; t < 3; t++) {
		memset(rx, 0, sizeof(rx));
		len = ft_frame_write(tx, cmd, d, n);
		memset(&x, 0, sizeof(x));
		x.tx = (unsigned long)tx;
		x.rx = (unsigned long)rx;
		x.len = len;
		x.speed = 6000000;
		x.bits = 8;
		if (sys_ioctl(fd, SPI_IOC_MSG1, &x) >= 0 && !(rx[3] & 0xA0))
			return 0;
		sleep_ms(1);
	}
	return -5;
}

#define FW_NAME "tsp_focaltech/ft8203_gts7xllite.bin"

/*
 * Samsung builds the firmware into its kernel, so the tablet has no copy once ours replaced it: the boot image's tail
 * carries it, in the firmware folder of the running kernel's modules. /usr/lib/firmware is the fallback.
 */
static int fw_open(void)
{
	/* Built afresh each time: recovery runs again after every panel power cycle. */
	char path[160] = "/usr/lib/modules/";
	ul n = sizeof("/usr/lib/modules/") - 1;
	int f = sys_openat("/proc/sys/kernel/osrelease", O_RDONLY), r;

	if (f >= 0) {
		r = sys_read(f, path + n, 64);
		sys_close(f);
		if (r > 1) {
			n += r - 1;	/* drop the newline */
			memcpy(path + n, "/firmware/" FW_NAME, sizeof("/firmware/" FW_NAME));
			f = sys_openat(path, O_RDONLY);
			if (f >= 0)
				return f;
		}
	}
	return sys_openat("/usr/lib/firmware/" FW_NAME, O_RDONLY);
}

/*
 * The chip answers 0xEF: it sits in its boot ROM and runs no firmware. Samsung's fts_fw_recovery(), need_reset = false:
 * check the ROM id (cmd 0x90 -> 82 01) and the boot state (reg 0xD0 == 0x01), then download the firmware into the
 * chip's PRAM (cmd 0xAD sets the address, 0xAE writes), check the ECC the ROM computes (cmd 0xCC, poll 0xCE == 0xA5,
 * read 0xCD) against ours, and only then start it (cmd 0x08). The chip variant row used is Samsung's {0x82, 0x01}:
 * spi_pe 0, half_length 1, no DRAM. Everything written goes to RAM; the driver has no flash erase or program path and
 * neither do we. A reset undoes it. Without the firmware file, or if any read check fails, NOTHING is written.
 */
static int ft_boot_recover(int fd)
{
	static unsigned char fw[98304 + 16];
	unsigned char id[2] = { 0 }, st = 0, v[2] = { 0 }, a[3], c[6];
	unsigned int size, cl, cn, ecc_tp, ecc_host;
	int n = 0, r, f = fw_open();

	if (f < 0) {
		put("touch: no firmware " FW_NAME " in the tail or /usr/lib/firmware, nothing is written\n");
		return -1;
	}
	while ((r = sys_read(f, fw + n, sizeof(fw) - 16 - n)) > 0)
		n += r;
	sys_close(f);
	cl = (fw[0x100] << 8) | fw[0x101];
	cn = (fw[0x102] << 8) | fw[0x103];
	size = cl * 2;
	if (n < 0x120 || n > 98304 || cl + cn != 0xFFFF || size < 0x120 || size > (unsigned int)n || size > 96 * 1024) {
		put("touch: firmware file is not what the driver expects, nothing is written\n");
		return -1;
	}
	if (ft_read(fd, 0x90, id, 2) || id[0] != 0x82 || id[1] != 0x01) {
		put("touch: boot ROM id is not 82 01, nothing is written\n");
		return -1;
	}
	if (ft_read(fd, 0xD0, &st, 1) || st != 0x01) {
		put("touch: boot state is not 01, nothing is written\n");
		return -1;
	}
	put("touch: boot ROM confirmed, writing ");
	putnum(size);
	put(" bytes of firmware to the chip RAM\n");
	for (unsigned int off = 0; off < size; off += FT_CHUNK) {
		int len = size - off < FT_CHUNK ? size - off : FT_CHUNK;

		a[0] = off >> 16;
		a[1] = off >> 8;
		a[2] = off;
		if (ft_write(fd, 0xAD, a, 3) || ft_write(fd, 0xAE, fw + off, len)) {
			put("touch: firmware write failed at ");
			putnum(off);
			put(", the firmware is not started\n");
			return -1;
		}
		sleep_ms(1);
	}
	c[0] = 0; c[1] = 0; c[2] = 0;	/* ECC start address 0 */
	c[3] = size >> 16; c[4] = size >> 8; c[5] = size;
	if (ft_write(fd, 0xCC, c, 6)) {
		put("touch: ecc command failed, the firmware is not started\n");
		return -1;
	}
	sleep_ms(2);
	for (int t = 0; t < 200; t++) {
		if (!ft_read(fd, 0xCE, v, 1) && v[0] == 0xA5)
			break;
		v[0] = 0;
		sleep_ms(1);
	}
	if (v[0] != 0xA5 || ft_read(fd, 0xCD, v, 2)) {
		put("touch: ecc did not finish, the firmware is not started\n");
		return -1;
	}
	ecc_tp = (v[0] << 8) | v[1];
	ecc_host = ft_ecc(fw, size);
	put("touch: ecc chip ");
	puthex(ecc_tp >> 8);
	puthex(ecc_tp);
	put(" host ");
	puthex(ecc_host >> 8);
	puthex(ecc_host);
	put("\n");
	if (ecc_tp != ecc_host) {
		put("touch: ecc mismatch, the firmware is NOT started\n");
		return -1;
	}
	if (ft_write(fd, 0x08, 0, 0)) {
		put("touch: start command failed\n");
		return -1;
	}
	sleep_ms(10);
	put("touch: firmware started\n");
	return 0;
}

/*
 * The chip's interrupt, gpio22 (rev7 tree focaltech,irq-gpio; Samsung requests IRQF_TRIGGER_FALLING): one falling edge
 * per new frame. Between frames register 0x01 still holds the last one, so a timed poll re-read the last "finger down"
 * frame right after a lift and pressed again (every tap ended on the next tap). Requested through the GPIO character
 * device as an input with falling-edge events: listening only, the line is never driven and its bias is left alone.
 * struct gpio_v2_line_request: offsets[0] at 0, consumer at 256, config.flags at 288, num_lines at 560, fd at 588 (592 B).
 */
static int touch_irq_open(void)
{
	static unsigned long long req[592 / 8];
	unsigned char *b = (unsigned char *)req;
	const char *nm = "gts7-touchd";
	char path[] = "/dev/gpiochip0", info[68];	/* struct gpiochip_info: name[32], label[32], lines */
	int c = -1;
	long r;

	/* The chip number moves (PM6350's gpio block took gpiochip0): find the TLMM by its label, as pinctrl-msm sets it. */
	for (; path[13] <= '9'; path[13]++) {
		c = sys_openat(path, O_RDWR);
		if (c < 0)
			continue;
		if (sys_ioctl(c, 0x8044b401, info) == 0 && !memcmp_(info + 32, "f100000.pinctrl", 16))	/* GPIO_GET_CHIPINFO_IOCTL */
			break;
		sys_close(c);
		c = -1;
	}
	if (c < 0)
		return -1;
	memset(req, 0, sizeof(req));
	*(unsigned int *)b = 22;
	for (int i = 0; nm[i]; i++)
		b[256 + i] = nm[i];
	req[288 / 8] = (1ULL << 2) | (1ULL << 5);	/* GPIO_V2_LINE_FLAG_INPUT | GPIO_V2_LINE_FLAG_EDGE_FALLING */
	*(unsigned int *)(b + 560) = 1;
	r = sys_ioctl(c, 0xc250b407, req);	/* GPIO_V2_GET_LINE_IOCTL */
	sys_close(c);
	return r < 0 ? -1 : *(int *)(b + 588);
}

/* >0: at least one new edge (all queued events are drained), 0: none within ms. */
static int touch_irq_wait(int irq, long ms)
{
	struct { int fd; short events, revents; } p = { irq, 1, 0 };	/* POLLIN */
	long ts[2] = { ms / 1000, (ms % 1000) * 1000000 };
	static unsigned char ev[48 * 16];	/* struct gpio_v2_line_event is 48 bytes */

	if (sc(73, (sl)&p, 1, (sl)ts, 0, 0) <= 0)	/* ppoll */
		return 0;
	sys_read(irq, ev, sizeof(ev));
	return 1;
}

static void touch_main(void)
{
	static unsigned char d[80];
	int fd = touch_open(), fails = 0, active[10] = { 0 }, lastx[10] = { 0 }, lasty[10] = { 0 }, tick = 0;
	unsigned char mode = 0, bits = 8;
	unsigned int speed = 6000000;
	unsigned char idh = 0, idl = 0;

	if (fd < 0) {
		put("touch: no spi device for 980000.spi / spidev, code ");
		putnum(fd);
		put("\n");
		return;
	}
	sys_ioctl(fd, SPI_IOC_WR_MODE, &mode);
	sys_ioctl(fd, SPI_IOC_WR_BITS, &bits);
	sys_ioctl(fd, SPI_IOC_WR_SPEED, &speed);
	{
		/* Pure register reads (pinctrl-msm debug file): level, function, pull of the touch pins. 13-15 SPI, 16 CS, 18 reset, 22 irq, 23 vendor check, 73 panel reset. */
		static const char *const pins[] = { " gpio13 ", " gpio14 ", " gpio15 ", " gpio16 ", " gpio18 ", " gpio22 ", " gpio23 ", " gpio73 ", 0 };

		dump_file("/sys/kernel/debug/gpio", pins);
	}
	for (int t = 0; t < 40; t++) {	/* up to 2 s: the TDDI firmware needs time after the reset is released */
		if (!ft_read(fd, 0xA3, &idh, 1) && !ft_read(fd, 0x9F, &idl, 1)) {
			put("touch: chip answered after ");
			putnum(t * 50);
			put(" ms\n");
			break;
		}
		idh = idl = 0;
		sleep_ms(50);
	}
	if (!idh) {
		put("touch: chip id read failed (spi/crc), nothing more is read\n");
		touch_diag(fd);
		return;
	}
	put("touch: chip id ");
	puthex(idh);
	puthex(idl);
	put("\n");
	if (idh == 0xEF && idl == 0xEF && !ft_boot_recover(fd)) {
		for (int t = 0; t < 40; t++) {	/* up to 2 s for the new firmware to come up */
			if (!ft_read(fd, 0xA3, &idh, 1) && idh == 0x82 && !ft_read(fd, 0x9F, &idl, 1))
				break;
			sleep_ms(50);
		}
		put("touch: chip id now ");
		puthex(idh);
		puthex(idl);
		put("\n");
	}
	if (idh != 0x82) {
		put("touch: not a FocalTech 82xx id, nothing more is read\n");
		return;
	}
	ui_create();
	int irq = touch_irq_open();

	put(irq >= 0 ? "touch: reading a frame on each gpio22 edge, touch the screen\n"
		     : "touch: no gpio22 edge events, polling every 10 ms (taps may stick)\n");
	for (;;) {
		/* Without an edge for 1 s, the read only checks the chip's health: its frame is old, its points are not used. */
		int fresh = irq < 0 || touch_irq_wait(irq, 1000);
		int seen[10] = { 0 }, pn, rerr = ft_read(fd, 0x01, d, 62);

		/*
		 * Like Samsung's fts_read_touchdata: an EF EF EF frame (or reads that keep failing) means the chip fell back to
		 * its boot ROM and lost the firmware. Lift all fingers and redo the RAM download in place; ft_boot_recover
		 * writes nothing unless the ROM id and boot state confirm it. Before this, read errors looped at 1 Hz forever.
		 */
		if ((!rerr && d[1] == 0xEF && d[2] == 0xEF && d[3] == 0xEF) || (rerr && ++fails % 25 == 0)) {
			put("touch: chip lost its firmware? frame ");
			put(rerr ? "read errors " : "EF EF EF ");
			putnum(fails);
			put(", trying boot recovery\n");
			for (int id = 0; id < 10; id++) {
				if (active[id]) {
					ui_send(3, 0x2f, id);
					ui_send(3, 0x39, -1);
					active[id] = 0;
				}
			}
			ui_send(0, 0, 0);
			if (ft_boot_recover(fd))
				sleep_ms(1000);
			continue;
		}
		if (rerr) {
			sleep_ms(20);
			continue;
		}
		fails = 0;
		if (!fresh)
			continue;
		pn = d[1] & 0x0F;	/* register 0x02, touch point count */
		for (int i = 0; i < 10; i++) {
			unsigned char *q = d + 2 + 6 * i;	/* register 0x03 + 6 per point */
			int id = q[2] >> 4, flag = q[0] >> 6;
			int x = ((q[0] & 0x0F) << 8) | q[1], y = ((q[2] & 0x0F) << 8) | q[3];

			if (id >= 10 || pn == 0 || pn > 10)
				break;
			if (flag == 1)	/* up */
				continue;
			seen[id] = 1;
			if (!active[id] || x != lastx[id] || y != lasty[id]) {
				ui_send(3, 0x2f, id);
				if (!active[id]) {
					ui_send(3, 0x39, id + 1);
					put("touch: down id ");
					putnum(id);
					put(" x ");
					putnum(x);
					put(" y ");
					putnum(y);
					put("\n");
				} else if (++tick % 15 == 0) {
					put("touch: move id ");
					putnum(id);
					put(" x ");
					putnum(x);
					put(" y ");
					putnum(y);
					put("\n");
				}
				ui_send(3, 0x35, x);
				ui_send(3, 0x36, y);
				active[id] = 1;
				lastx[id] = x;
				lasty[id] = y;
				ui_send(0, 0, 0);
			}
		}
		for (int id = 0; id < 10; id++) {
			if (active[id] && !seen[id]) {
				ui_send(3, 0x2f, id);
				ui_send(3, 0x39, -1);
				active[id] = 0;
				put("touch: up id ");
				putnum(id);
				put("\n");
				ui_send(0, 0, 0);
			}
		}
		if (irq < 0)
			sleep_ms(10);
	}
}

/*
 * The touch process (chip release, firmware download, uinput) must run before init hands over to systemd: it keeps its
 * spidev and uinput descriptors across the switch. It used to start at 10 s, which raced with a fast rootfs push
 * (the switch came first and there was no touch at all). Started once, from the main loop and again before a push.
 */
static void dump_file(const char *path, const char *const *keys)
{
	static char b[4096], line[256];
	int fd = sys_openat(path, O_RDONLY), n = 0;
	long r;

	put("--- ");
	put(path);
	put(" ---\n");
	if (fd < 0) {
		put("cannot open\n");
		return;
	}
	while ((r = sys_read(fd, b, sizeof(b))) > 0) {
		for (long i = 0; i < r; i++) {
			if (b[i] != '\n' && n < (int)sizeof(line) - 2) {
				line[n++] = b[i];
				continue;
			}
			line[n] = 0;
			n = 0;
			for (int k = 0; keys[k]; k++)
				if (has(line, keys[k])) {
					put(line);
					put("\n");
					break;
				}
		}
	}
	sys_close(fd);
}

/* Why does dwc3 fail at ep0? Registers of the controller and the state of the USB/PHY clocks. */

void _start(void)
{
	touch_main();
	sc(93, 1, 0, 0, 0, 0);	/* exit(1): systemd restarts it */
}
