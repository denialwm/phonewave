// SPDX-License-Identifier: MIT
/*
 * A stand-in ADSP for running the open AudioReach graph services (GSL) on a
 * workstation: GPR's datalink to the ADSP domain is replaced by a recorder
 * that writes every command, with its out-of-band payload, to a log file and
 * answers it as a healthy SPF would.
 *
 * Record format (all u32 little-endian): opcode, dst_port, src_port, token,
 * inband_size, oob_size, then inband bytes, then oob bytes.
 */
#define _GNU_SOURCE
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

#include "gpr_comdef.h"
#include "gpr_api_i.h"
#include "ipc_dl_api.h"

#ifndef AR_EOK
#define AR_EOK 0
#endif

#define GPR_IBASIC_RSP_RESULT 0x02001005
#define APM_CMD_GET_SPF_STATE 0x01001021
#define APM_CMD_RSP_GET_SPF_STATE 0x02001007
#define APM_CMD_SHARED_MEM_MAP_REGIONS 0x0100100C
#define APM_CMD_RSP_SHARED_MEM_MAP_REGIONS 0x02001001
#define APM_CMD_GET_CFG 0x01001007
#define APM_CMD_RSP_GET_CFG 0x02001000

/* Commands whose payload starts with apm_cmd_header_t. */
static int has_apm_header(uint32_t op)
{
	switch (op) {
	case 0x01001000: /* GRAPH_OPEN */
	case 0x01001006: /* SET_CFG */
	case 0x01001007: /* GET_CFG */
	case 0x01001008: /* REGISTER_CFG */
	case 0x01001009: /* DEREGISTER_CFG */
	case 0x0100100A: /* REGISTER_SHARED_CFG */
	case 0x0100100B: /* DEREGISTER_SHARED_CFG */
	case 0x0100100E: /* REGISTER_MODULE_EVENTS */
	case 0x0100101E: /* AMDB_CMD_REGISTER_MODULES */
	case 0x01001020: /* AMDB_CMD_LOAD_MODULES */
		return 1;
	}
	return 0;
}

/* GPR hands its table over on the stack: keep a copy. */
static gpr_to_ipc_vtbl_t gpr_vtbl_copy;
static const gpr_to_ipc_vtbl_t *gpr_vtbl = &gpr_vtbl_copy;
static FILE *out;
static uint32_t next_handle = 0x1000;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;

struct pkt {
	struct pkt *next;
	uint32_t len;
	uint8_t data[];
};
static struct pkt *head, *tail;
static pthread_cond_t more = PTHREAD_COND_INITIALIZER;

static void put32(FILE *f, uint32_t v) { fwrite(&v, 4, 1, f); }

static void reply(const uint8_t *req, uint32_t op, const void *payload, uint32_t size)
{
	uint32_t total = 24 + size;
	uint8_t *r = calloc(1, total);
	uint32_t header = 0 | (6 << 4) | (total << 8);
	memcpy(r, &header, 4);
	r[4] = req[5]; /* dst domain = request's src */
	r[5] = req[4];
	memcpy(r + 8, req + 12, 4); /* src port = request's dst port */
	memcpy(r + 12, req + 8, 4);
	memcpy(r + 16, req + 16, 4); /* token */
	memcpy(r + 20, &op, 4);
	memcpy(r + 24, payload, size);
	gpr_vtbl->receive(r, total);
}

static void handle(uint8_t *p, uint32_t len)
{
	uint32_t opcode, src_port, dst_port, token;
	memcpy(&src_port, p + 8, 4);
	memcpy(&dst_port, p + 12, 4);
	memcpy(&token, p + 16, 4);
	memcpy(&opcode, p + 20, 4);
	uint32_t hsize = ((p[0] >> 4) & 0xf) * 4;
	uint8_t *payload = p + hsize;
	uint32_t psize = len - hsize;

	const uint8_t *oob = NULL;
	uint32_t oob_size = 0;
	if (has_apm_header(opcode) && psize >= 16) {
		uint32_t lsw, msw, handle, size;
		memcpy(&lsw, payload, 4);
		memcpy(&msw, payload + 4, 4);
		memcpy(&handle, payload + 8, 4);
		memcpy(&size, payload + 12, 4);
		if (handle && (lsw || msw)) {
			oob = (const uint8_t *)(uintptr_t)(((uint64_t)msw << 32) | lsw);
			oob_size = size;
		}
	}
	pthread_mutex_lock(&lock);
	put32(out, opcode);
	put32(out, dst_port);
	put32(out, src_port);
	put32(out, token);
	put32(out, psize);
	put32(out, oob_size);
	fwrite(payload, 1, psize, out);
	if (oob_size)
		fwrite(oob, 1, oob_size, out);
	fflush(out);
	pthread_mutex_unlock(&lock);

	if (opcode == APM_CMD_GET_SPF_STATE) {
		uint32_t ready = 1;
		reply(p, APM_CMD_RSP_GET_SPF_STATE, &ready, 4);
	} else if (opcode == APM_CMD_SHARED_MEM_MAP_REGIONS) {
		uint32_t h = next_handle++;
		reply(p, APM_CMD_RSP_SHARED_MEM_MAP_REGIONS, &h, 4);
	} else if (opcode == 0x01001020 && psize > 16) {
		/* AMDB_CMD_LOAD_MODULES, in band: echo the module list as loaded,
		 * each with a nonzero handle. */
		uint32_t n = psize - 16;
		uint8_t *body = malloc(n);
		memcpy(body, payload + 16, n);
		memset(body + 4, 0, 4); /* error_code */
		uint32_t count;
		memcpy(&count, body + 8, 4);
		for (uint32_t i = 0; i < count && 12 + 12 * i + 12 <= n; i++) {
			uint32_t h = 0x100 + i;
			memcpy(body + 12 + 12 * i + 4, &h, 4);
		}
		reply(p, 0x02001008, body, n);
		free(body);
	} else if (opcode == APM_CMD_GET_CFG) {
		uint32_t status = 0;
		reply(p, APM_CMD_RSP_GET_CFG, &status, 4);
	} else {
		uint32_t result[2] = { opcode, 0 };
		reply(p, GPR_IBASIC_RSP_RESULT, result, 8);
	}
}

static void *worker(void *arg)
{
	(void)arg;
	for (;;) {
		pthread_mutex_lock(&lock);
		while (!head)
			pthread_cond_wait(&more, &lock);
		struct pkt *k = head;
		head = k->next;
		if (!head)
			tail = NULL;
		pthread_mutex_unlock(&lock);
		handle(k->data, k->len);
		free(k);
	}
	return NULL;
}

static uint32_t fake_send(uint32_t domain_id, void *buf, uint32_t length)
{
	(void)domain_id;
	struct pkt *k = malloc(sizeof(*k) + length);
	k->next = NULL;
	k->len = length;
	memcpy(k->data, buf, length);
	gpr_vtbl->send_done(buf, length);
	pthread_mutex_lock(&lock);
	if (tail)
		tail->next = k;
	else
		head = k;
	tail = k;
	pthread_cond_signal(&more);
	pthread_mutex_unlock(&lock);
	return AR_EOK;
}

static uint32_t fake_receive_done(uint32_t domain_id, void *buf)
{
	(void)domain_id;
	free(buf);
	return AR_EOK;
}

static ipc_to_gpr_vtbl_t fake_vtbl = {
	.send = fake_send,
	.receive_done = fake_receive_done,
};

static uint32_t fake_dl_init(uint32_t src, uint32_t dst, const gpr_to_ipc_vtbl_t *in,
			     ipc_to_gpr_vtbl_t **outp)
{
	(void)src;
	(void)dst;
	gpr_vtbl_copy = *in;
	*outp = &fake_vtbl;
	const char *path = getenv("FAKEDSP_LOG");

	out = fopen(path ? path : "fakedsp.bin", "wb");
	pthread_t t;
	pthread_create(&t, NULL, worker, NULL);
	return AR_EOK;
}

static uint32_t fake_dl_deinit(uint32_t src, uint32_t dst)
{
	(void)src;
	(void)dst;
	if (out)
		fclose(out);
	out = NULL;
	return AR_EOK;
}

/* The GPR platform wrapper, as gpr_init_lx_wrapper.c has it, with the ADSP
 * domain on the recorder. */
static struct ipc_dl_v2_t table[GPR_PL_NUM_TOTAL_DOMAINS_V];
static struct gpr_packet_pool_info_v2_t pools[] = {
	{ GPR_HEAP_INDEX_DEFAULT, 0, 0, 100, 512 },
	{ GPR_HEAP_INDEX_DEFAULT, 0, 0, 4, 4096 },
	{ GPR_HEAP_INDEX_DEFAULT, 1, 0, 0, 65536 },
};

GPR_INTERNAL uint32_t gpr_drv_init(void)
{
	memset(table, 0, sizeof(table));
	table[0] = (struct ipc_dl_v2_t){ GPR_IDS_DOMAIN_ID_APPS_V, ipc_dl_local_init,
					 ipc_dl_local_deinit, TRUE };
	table[1] = (struct ipc_dl_v2_t){ GPR_IDS_DOMAIN_ID_ADSP_V, fake_dl_init,
					 fake_dl_deinit, TRUE };
	return gpr_drv_internal_init_v2(GPR_IDS_DOMAIN_ID_APPS_V, 2, table, 3, pools);
}

GPR_INTERNAL uint32_t gpr_drv_init_domain(uint32_t domain_id)
{
	(void)domain_id;
	return gpr_drv_init();
}
