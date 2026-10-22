// SPDX-License-Identifier: MIT
/*
 * radio-addr: gives a device's Wi-Fi and Bluetooth radios addresses of their
 * own, derived from the SoC serial number, for devices whose bootloader
 * passes none.
 *
 *   radio-addr wifi IFNAME   set IFNAME's MAC address, while it is down
 *   radio-addr bluetooth     set the public address of every Bluetooth
 *                            controller that is missing one, and keep doing
 *                            it for controllers that appear later
 *   radio-addr print         print both addresses
 *
 * Each address is the first six bytes of SHA-256 over a label and the serial,
 * so a device always gets the same ones and the two radios unrelated ones.
 */
#include <errno.h>
#include <net/if.h>
#include <net/if_arp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>

#define SERIAL_FILE "/sys/devices/soc0/serial_number"

/* SHA-256, FIPS 180-4. */
static const uint32_t sha256_k[64] = {
	0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
	0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
	0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
	0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
	0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
	0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
	0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
	0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
	0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
	0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
	0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

static uint32_t ror(uint32_t x, int n)
{
	return x >> n | x << (32 - n);
}

static void sha256_block(uint32_t h[8], const uint8_t *p)
{
	uint32_t w[64], s[8];

	for (int i = 0; i < 16; i++)
		w[i] = (uint32_t)p[i * 4] << 24 | p[i * 4 + 1] << 16 |
		       p[i * 4 + 2] << 8 | p[i * 4 + 3];
	for (int i = 16; i < 64; i++)
		w[i] = w[i - 16] + (ror(w[i - 15], 7) ^ ror(w[i - 15], 18) ^ w[i - 15] >> 3) +
		       w[i - 7] + (ror(w[i - 2], 17) ^ ror(w[i - 2], 19) ^ w[i - 2] >> 10);
	memcpy(s, h, sizeof(s));
	for (int i = 0; i < 64; i++) {
		uint32_t t1 = s[7] + (ror(s[4], 6) ^ ror(s[4], 11) ^ ror(s[4], 25)) +
			      ((s[4] & s[5]) ^ (~s[4] & s[6])) + sha256_k[i] + w[i];
		uint32_t t2 = (ror(s[0], 2) ^ ror(s[0], 13) ^ ror(s[0], 22)) +
			      ((s[0] & s[1]) ^ (s[0] & s[2]) ^ (s[1] & s[2]));

		memmove(s + 1, s, 7 * sizeof(*s));
		s[4] += t1;
		s[0] = t1 + t2;
	}
	for (int i = 0; i < 8; i++)
		h[i] += s[i];
}

static void sha256(const uint8_t *data, size_t length, uint8_t digest[32])
{
	uint32_t h[8] = { 0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
			  0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19 };
	uint8_t tail[128] = { 0 };
	size_t rest = length % 64, blocks = rest < 56 ? 1 : 2;
	uint64_t bits = (uint64_t)length * 8;

	for (size_t i = 0; i + 64 <= length; i += 64)
		sha256_block(h, data + i);
	memcpy(tail, data + length - rest, rest);
	tail[rest] = 0x80;
	for (int i = 0; i < 8; i++)
		tail[blocks * 64 - 1 - i] = bits >> (i * 8);
	for (size_t i = 0; i < blocks; i++)
		sha256_block(h, tail + i * 64);
	for (int i = 0; i < 32; i++)
		digest[i] = h[i / 4] >> (24 - i % 4 * 8);
}

/* The serial, without the newline sysfs ends it with. */
static int read_serial(char *serial, size_t size)
{
	const char *path = getenv("RADIO_ADDR_SERIAL_FILE") ?: SERIAL_FILE;
	FILE *file = fopen(path, "r");
	size_t length;

	if (!file || !fgets(serial, size, file)) {
		fprintf(stderr, "radio-addr: %s: %s\n", path,
			file ? "empty" : strerror(errno));
		if (file)
			fclose(file);
		return -1;
	}
	fclose(file);
	length = strcspn(serial, "\r\n");
	serial[length] = 0;
	if (!length) {
		fprintf(stderr, "radio-addr: %s: empty\n", path);
		return -1;
	}
	return 0;
}

/* Six bytes of SHA-256 over LABEL, a NUL and the serial, most significant first. */
static int derive(const char *label, uint8_t address[6])
{
	char serial[128];
	uint8_t input[256], digest[32];
	size_t length = strlen(label);

	if (read_serial(serial, sizeof(serial)))
		return -1;
	memcpy(input, label, length + 1);
	memcpy(input + length + 1, serial, strlen(serial));
	sha256(input, length + 1 + strlen(serial), digest);
	memcpy(address, digest, 6);
	return 0;
}

/* A locally administered unicast MAC: nobody assigned us a vendor prefix. */
static int wifi_address(uint8_t address[6])
{
	if (derive("radio-addr wifi", address))
		return -1;
	address[0] = (address[0] & 0xfc) | 0x02;
	return 0;
}

/* A public address whose lower half is not one of the reserved inquiry LAPs. */
static int bluetooth_address(uint8_t address[6])
{
	uint32_t lap;

	if (derive("radio-addr bluetooth", address))
		return -1;
	lap = (uint32_t)address[3] << 16 | address[4] << 8 | address[5];
	if (lap >= 0x9e8b00 && lap <= 0x9e8b3f)
		address[3] ^= 0x01;
	return 0;
}

static void print_address(const char *name, const uint8_t address[6])
{
	printf("%s %02x:%02x:%02x:%02x:%02x:%02x\n", name, address[0], address[1],
	       address[2], address[3], address[4], address[5]);
}

static int set_wifi(const char *ifname)
{
	struct ifreq ifr = { 0 };
	uint8_t address[6];
	int fd;

	if (wifi_address(address))
		return 1;
	if (strlen(ifname) >= sizeof(ifr.ifr_name)) {
		fprintf(stderr, "radio-addr: %s: name too long\n", ifname);
		return 1;
	}
	strcpy(ifr.ifr_name, ifname);
	ifr.ifr_hwaddr.sa_family = ARPHRD_ETHER;
	memcpy(ifr.ifr_hwaddr.sa_data, address, 6);
	fd = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
	if (fd < 0 || ioctl(fd, SIOCSIFHWADDR, &ifr)) {
		fprintf(stderr, "radio-addr: %s: %s\n", ifname, strerror(errno));
		return 1;
	}
	close(fd);
	return 0;
}

/* The Bluetooth management interface, include/net/bluetooth/mgmt.h. */
#define AF_BLUETOOTH_			31
#define BTPROTO_HCI			1
#define HCI_CHANNEL_CONTROL		3
#define MGMT_INDEX_NONE			0xffff
#define MGMT_EV_CMD_COMPLETE		0x0001
#define MGMT_EV_CMD_STATUS		0x0002
#define MGMT_EV_UNCONF_INDEX_ADDED	0x001d
#define MGMT_OP_READ_UNCONF_INDEX_LIST	0x0036
#define MGMT_OP_READ_CONFIG_INFO	0x0037
#define MGMT_OP_SET_PUBLIC_ADDRESS	0x0039
#define MGMT_OPTION_PUBLIC_ADDRESS	0x00000002

struct sockaddr_hci {
	sa_family_t hci_family;
	unsigned short hci_dev;
	unsigned short hci_channel;
};

static unsigned int le16(const uint8_t *p)
{
	return p[0] | p[1] << 8;
}

static uint32_t le32(const uint8_t *p)
{
	return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24;
}

static int mgmt_send(int fd, unsigned int opcode, unsigned int index,
		     const void *data, size_t length)
{
	uint8_t packet[6 + 6] = { opcode, opcode >> 8, index, index >> 8,
				  length, length >> 8 };

	memcpy(packet + 6, data, length);
	if (write(fd, packet, 6 + length) != (ssize_t)(6 + length)) {
		fprintf(stderr, "radio-addr: management write: %s\n", strerror(errno));
		return -1;
	}
	return 0;
}

/* Answers one reply or event; returns -1 when the socket must be given up. */
static int mgmt_handle(int fd, const uint8_t *packet, size_t length,
		       const uint8_t bdaddr[6])
{
	unsigned int event, index, opcode, status;
	const uint8_t *data = packet + 6;
	size_t size;

	if (length < 6 || le16(packet + 4) != length - 6)
		return 0;
	event = le16(packet);
	index = le16(packet + 2);
	size = length - 6;

	if (event == MGMT_EV_UNCONF_INDEX_ADDED)
		return mgmt_send(fd, MGMT_OP_READ_CONFIG_INFO, index, NULL, 0);
	if ((event != MGMT_EV_CMD_COMPLETE && event != MGMT_EV_CMD_STATUS) || size < 3)
		return 0;
	opcode = le16(data);
	status = data[2];
	data += 3;
	size -= 3;
	if (status) {
		fprintf(stderr, "radio-addr: hci%u: management command %#06x failed: status %#04x\n",
			index, opcode, status);
		return 0;
	}

	switch (opcode) {
	case MGMT_OP_READ_UNCONF_INDEX_LIST:
		if (size < 2 || size < 2 + 2 * le16(data))
			return 0;
		for (unsigned int i = 0; i < le16(data); i++)
			if (mgmt_send(fd, MGMT_OP_READ_CONFIG_INFO, le16(data + 2 + 2 * i),
				      NULL, 0))
				return -1;
		return 0;
	case MGMT_OP_READ_CONFIG_INFO:
		if (size < 10 || !(le32(data + 6) & MGMT_OPTION_PUBLIC_ADDRESS))
			return 0;
		return mgmt_send(fd, MGMT_OP_SET_PUBLIC_ADDRESS, index, bdaddr, 6);
	case MGMT_OP_SET_PUBLIC_ADDRESS:
		fprintf(stderr, "radio-addr: hci%u: public address set\n", index);
		return 0;
	}
	return 0;
}

static int run_bluetooth(void)
{
	struct sockaddr_hci addr = { AF_BLUETOOTH_, MGMT_INDEX_NONE, HCI_CHANNEL_CONTROL };
	uint8_t address[6], bdaddr[6], packet[6 + 65535];
	ssize_t length;
	int fd;

	if (bluetooth_address(address))
		return 1;
	/* The controller takes the address least significant byte first. */
	for (int i = 0; i < 6; i++)
		bdaddr[i] = address[5 - i];

	fd = socket(AF_BLUETOOTH_, SOCK_RAW | SOCK_CLOEXEC, BTPROTO_HCI);
	if (fd < 0 || bind(fd, (struct sockaddr *)&addr, sizeof(addr))) {
		fprintf(stderr, "radio-addr: management socket: %s\n", strerror(errno));
		return 1;
	}
	/* Asking for the list also subscribes to unconfigured index events. */
	if (mgmt_send(fd, MGMT_OP_READ_UNCONF_INDEX_LIST, MGMT_INDEX_NONE, NULL, 0))
		return 1;
	while ((length = read(fd, packet, sizeof(packet))) >= 0 || errno == EINTR)
		if (length >= 0 && mgmt_handle(fd, packet, length, bdaddr))
			return 1;
	fprintf(stderr, "radio-addr: management read: %s\n", strerror(errno));
	return 1;
}

int main(int argc, char **argv)
{
	uint8_t address[6];

	if (argc == 3 && !strcmp(argv[1], "wifi"))
		return set_wifi(argv[2]);
	if (argc == 2 && !strcmp(argv[1], "bluetooth"))
		return run_bluetooth();
	if (argc == 2 && !strcmp(argv[1], "print")) {
		if (wifi_address(address))
			return 1;
		print_address("wifi", address);
		if (bluetooth_address(address))
			return 1;
		print_address("bluetooth", address);
		return 0;
	}
	fputs("usage: radio-addr wifi IFNAME | radio-addr bluetooth | radio-addr print\n", stderr);
	return 2;
}
