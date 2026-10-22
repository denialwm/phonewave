// SPDX-License-Identifier: MIT
/*
 * Drives the open AudioReach graph services (GSL) through a voice call the
 * way the stock audio HAL does, against the stand-in DSP in fakedsp.c, which
 * records every command.
 *
 *   voicegen ACDB list RX_KEYS TX_KEYS
 *     prints the calibration key vectors and tagged modules of both graphs
 *   voicegen ACDB call RX_KEYS TX_KEYS VSID TX_CHANNELS
 *     opens, configures and starts both graphs
 *
 * Keys are graph key vectors, "key=value,key=value", numbers in any C base.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "gsl_intf.h"

#define MAX_KEYS 16

#define VCPM_MIID 4
#define VCPM_PARAM_ID_VSID 0x080011BC
#define VCPM_PARAM_ID_TX_DEV_PP_CHANNEL_INFO 0x08001310
#define VCPM_PARAM_ID_CAL_KEYS 0x0800116B
#define VCPM_CAL_KEY_ID_VOLUME_LEVEL 0x08001166
#define VCPM_CAL_KEY_ID_VOL_BOOST 0x080011B6
#define VCPM_CAL_KEY_ID_BWE 0x08001167

struct keys {
	struct gsl_key_value_pair pairs[MAX_KEYS];
	struct gsl_key_vector vector;
};

static int parse_keys(const char *text, struct keys *keys)
{
	char *copy = strdup(text), *save = NULL;
	uint32_t n = 0;

	for (char *pair = strtok_r(copy, ",", &save); pair; pair = strtok_r(NULL, ",", &save)) {
		char *value = strchr(pair, '=');

		if (!value || n == MAX_KEYS) {
			free(copy);
			return -1;
		}
		*value++ = 0;
		keys->pairs[n].key = strtoul(pair, NULL, 0);
		keys->pairs[n].value = strtoul(value, NULL, 0);
		n++;
	}
	free(copy);
	keys->vector.num_kvps = n;
	keys->vector.kvp = keys->pairs;
	return 0;
}

static void list_ckvs(const char *name, struct gsl_key_vector *gkv)
{
	struct gsl_key_vector_list list = { 0 };
	int ret = gsl_get_graph_ckvs(gkv, &list);

	printf("%s ckvs: ret %d, %u vectors\n", name, ret, list.num_key_vectors);
	if (ret || !list.list_size)
		return;
	list.key_vector_list = calloc(1, list.list_size);
	gsl_get_graph_ckvs(gkv, &list);
	uint8_t *p = (uint8_t *)list.key_vector_list;
	for (uint32_t i = 0; i < list.num_key_vectors; i++) {
		struct gsl_key_vector_array *vector = (void *)p;

		printf("  [");
		for (uint32_t j = 0; j < vector->num_keys; j++)
			printf(" %08x=%08x", vector->kvp[j].key, vector->kvp[j].value);
		printf(" ]\n");
		p += 4 + vector->num_keys * 8;
	}
}

static void list_tags(const char *name, struct gsl_key_vector *gkv)
{
	size_t size = 0;

	gsl_get_tags_with_module_info(gkv, NULL, &size);
	struct gsl_tag_module_info *tags = calloc(1, size);
	int ret = gsl_get_tags_with_module_info(gkv, tags, &size);

	printf("%s tags: ret %d, %u tags\n", name, ret, ret ? 0 : tags->num_tags);
	if (ret)
		return;
	uint8_t *p = tags->tag_module_entry;
	for (uint32_t i = 0; i < tags->num_tags; i++) {
		struct gsl_tag_module_info_entry *entry = (void *)p;

		printf("  tag %08x:", entry->tag_id);
		for (uint32_t j = 0; j < entry->num_modules; j++)
			printf(" module %08x instance %08x;", entry->module_entry[j].module_id,
			       entry->module_entry[j].module_iid);
		printf("\n");
		p += sizeof(*entry) + entry->num_modules * sizeof(entry->module_entry[0]);
	}
}

/* One apm_module_param_data_t record header; returns where its body goes. */
static uint32_t *param(uint32_t *p, uint32_t miid, uint32_t pid, uint32_t size)
{
	p[0] = miid;
	p[1] = pid;
	p[2] = size;
	p[3] = 0;
	return p + 4;
}

static int custom(const char *what, gsl_handle_t graph, uint32_t *start, uint32_t *end)
{
	int ret = gsl_set_custom_config(graph, (uint8_t *)start, (uint8_t *)end - (uint8_t *)start);

	printf("%s: %d\n", what, ret);
	return ret;
}

/* What the stock HAL's voice session sends, in its order. */
static int call(struct keys *rx, struct keys *tx, uint32_t vsid, uint32_t tx_channels)
{
	struct gsl_key_vector none = { 0, NULL };
	gsl_handle_t rx_graph = NULL, tx_graph = NULL;
	uint32_t buffer[64], *p;
	int ret;

	ret = gsl_open(&rx->vector, &none, &rx_graph);
	printf("open rx: %d\n", ret);
	if (!ret) {
		ret = gsl_open(&tx->vector, &none, &tx_graph);
		printf("open tx: %d\n", ret);
	}
	if (ret)
		return ret;

	p = param(buffer, VCPM_MIID, VCPM_PARAM_ID_VSID, 4);
	*p++ = vsid;
	*p++ = 0;
	ret |= custom("rx vsid", rx_graph, buffer, p);

	p = param(buffer, VCPM_MIID, VCPM_PARAM_ID_TX_DEV_PP_CHANNEL_INFO, 8);
	*p++ = vsid;
	*p++ = tx_channels;
	ret |= custom("tx channels", tx_graph, buffer, p);

	p = param(buffer, VCPM_MIID, VCPM_PARAM_ID_CAL_KEYS, 8 + 3 * 8);
	*p++ = vsid;
	*p++ = 3;
	*p++ = VCPM_CAL_KEY_ID_VOLUME_LEVEL;
	*p++ = 0;
	*p++ = VCPM_CAL_KEY_ID_VOL_BOOST;
	*p++ = 0;
	*p++ = VCPM_CAL_KEY_ID_BWE;
	*p++ = 0;
	ret |= custom("rx cal keys", rx_graph, buffer, p);

	ret |= gsl_ioctl(rx_graph, GSL_CMD_PREPARE, NULL, 0);
	ret |= gsl_ioctl(rx_graph, GSL_CMD_START, NULL, 0);
	ret |= gsl_ioctl(tx_graph, GSL_CMD_PREPARE, NULL, 0);
	ret |= gsl_ioctl(tx_graph, GSL_CMD_START, NULL, 0);
	printf("prepare and start: %d\n", ret);
	return ret;
}

int main(int argc, char **argv)
{
	struct gsl_acdb_data_files files = { .num_files = 1 };
	struct gsl_init_data init = {
		.acdb_files = &files,
		.max_num_ready_checks = 1,
		.ready_check_interval_ms = 10,
	};
	struct keys rx, tx;
	int ret;

	if (argc < 5 || strlen(argv[1]) >= sizeof(files.acdbFiles[0].fileName) ||
	    (strcmp(argv[2], "list") && strcmp(argv[2], "call")) ||
	    (!strcmp(argv[2], "call") && argc != 7) ||
	    parse_keys(argv[3], &rx) || parse_keys(argv[4], &tx)) {
		fprintf(stderr, "usage: voicegen ACDB list RX_KEYS TX_KEYS\n"
				"       voicegen ACDB call RX_KEYS TX_KEYS VSID TX_CHANNELS\n");
		return 2;
	}
	files.acdbFiles[0].fileNameLen = strlen(argv[1]);
	strcpy(files.acdbFiles[0].fileName, argv[1]);
	ret = gsl_init(&init);
	printf("gsl_init: %d\n", ret);
	if (ret)
		return 1;
	if (!strcmp(argv[2], "call"))
		return call(&rx, &tx, strtoul(argv[5], NULL, 0), strtoul(argv[6], NULL, 0)) ? 1 : 0;
	list_ckvs("rx", &rx.vector);
	list_ckvs("tx", &tx.vector);
	list_tags("rx", &rx.vector);
	list_tags("tx", &tx.vector);
	return 0;
}
