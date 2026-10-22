// SPDX-License-Identifier: MIT
/*
 * What callaudio needs from alsa-lib and libpipewire: the controls a UCM verb
 * names, reading and writing one control, and a PipeWire card's profile.
 * Each call opens what it uses and closes it again: calls start and end
 * rarely, and nothing stale survives an audio server restart.
 */
#include <errno.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <alsa/asoundlib.h>
#include <alsa/use-case.h>
#include <pipewire/pipewire.h>
#include <spa/param/profile.h>
#include <spa/pod/builder.h>
#include <spa/pod/parser.h>

static void copy(char *out, size_t len, const char *value)
{
	snprintf(out, len, "%s", value ? value : "");
}

/* UCM lists alternate names and comments. */
static bool ucm_has(snd_use_case_mgr_t *uc, const char *list_id, const char *name)
{
	const char **list;
	int n = snd_use_case_get_list(uc, list_id, &list);
	bool found = false;

	if (n < 0)
		return false;
	for (int i = 0; i < n; i += 2)
		if (!strcmp(list[i], name))
			found = true;
	snd_use_case_free_list(list, n);
	return found;
}

static void ucm_value(snd_use_case_mgr_t *uc, const char *name, const char *device,
		      const char *verb, char *out, size_t len)
{
	char id[256];
	const char *value = NULL;

	if (*out)
		return;
	snprintf(id, sizeof(id), "%s/%s/%s", name, device, verb);
	if (snd_use_case_get(uc, id, &value) < 0)
		return;
	copy(out, len, value);
	free((void *)value);
}

/*
 * The first card whose UCM configuration has `verb`, with the first
 * PlaybackVolume and CaptureSwitch its devices name. Reading values runs no
 * sequences: the card's state stays the audio server's.
 */
int callaudio_ucm_find(const char *verb, int *card, char *volume, size_t volume_len,
		       char *mute, size_t mute_len)
{
	int index = -1;

	while (snd_card_next(&index) == 0 && index >= 0) {
		char name[16], list_id[256];
		snd_use_case_mgr_t *uc;
		const char **devices;
		int n;

		snprintf(name, sizeof(name), "hw:%d", index);
		if (snd_use_case_mgr_open(&uc, name) < 0)
			continue;
		if (!ucm_has(uc, "_verbs", verb)) {
			snd_use_case_mgr_close(uc);
			continue;
		}
		copy(volume, volume_len, NULL);
		copy(mute, mute_len, NULL);
		snprintf(list_id, sizeof(list_id), "_devices/%s", verb);
		n = snd_use_case_get_list(uc, list_id, &devices);
		for (int i = 0; i < n; i += 2) {
			ucm_value(uc, "PlaybackVolume", devices[i], verb, volume, volume_len);
			ucm_value(uc, "CaptureSwitch", devices[i], verb, mute, mute_len);
		}
		if (n > 0)
			snd_use_case_free_list(devices, n);
		snd_use_case_mgr_close(uc);
		*card = index;
		return 0;
	}
	return -ENOENT;
}

/* Opens card's control `id` ("name='...'"), a boolean or integer one. */
static int ctl_open(int card, const char *id, snd_ctl_t **ctl, snd_ctl_elem_info_t *info,
		    snd_ctl_elem_value_t *value)
{
	snd_ctl_elem_id_t *elem;
	char name[16];
	int ret;

	snd_ctl_elem_id_alloca(&elem);
	ret = snd_ctl_ascii_elem_id_parse(elem, id);
	if (ret < 0)
		return ret;
	snprintf(name, sizeof(name), "hw:%d", card);
	ret = snd_ctl_open(ctl, name, 0);
	if (ret < 0)
		return ret;
	snd_ctl_elem_info_set_id(info, elem);
	ret = snd_ctl_elem_info(*ctl, info);
	if (ret >= 0 && snd_ctl_elem_info_get_type(info) != SND_CTL_ELEM_TYPE_BOOLEAN &&
	    snd_ctl_elem_info_get_type(info) != SND_CTL_ELEM_TYPE_INTEGER)
		ret = -EINVAL;
	if (ret < 0) {
		snd_ctl_close(*ctl);
		return ret;
	}
	snd_ctl_elem_info_get_id(info, elem);
	snd_ctl_elem_value_set_id(value, elem);
	return 0;
}

/* The control's first value and its range; a switch reads 0 or 1. */
int callaudio_ctl_get(int card, const char *id, long *value, long *min, long *max)
{
	snd_ctl_elem_info_t *info;
	snd_ctl_elem_value_t *val;
	snd_ctl_t *ctl;
	int ret;

	snd_ctl_elem_info_alloca(&info);
	snd_ctl_elem_value_alloca(&val);
	ret = ctl_open(card, id, &ctl, info, val);
	if (ret < 0)
		return ret;
	if (snd_ctl_elem_info_get_type(info) == SND_CTL_ELEM_TYPE_BOOLEAN) {
		*min = 0;
		*max = 1;
	} else {
		*min = snd_ctl_elem_info_get_min(info);
		*max = snd_ctl_elem_info_get_max(info);
	}
	ret = snd_ctl_elem_read(ctl, val);
	if (ret >= 0)
		*value = snd_ctl_elem_value_get_integer(val, 0);
	snd_ctl_close(ctl);
	return ret < 0 ? ret : 0;
}

/* Sets every channel of the control to `value`. */
int callaudio_ctl_set(int card, const char *id, long value)
{
	snd_ctl_elem_info_t *info;
	snd_ctl_elem_value_t *val;
	snd_ctl_t *ctl;
	unsigned int count;
	int ret;

	snd_ctl_elem_info_alloca(&info);
	snd_ctl_elem_value_alloca(&val);
	ret = ctl_open(card, id, &ctl, info, val);
	if (ret < 0)
		return ret;
	count = snd_ctl_elem_info_get_count(info);
	for (unsigned int i = 0; i < count; i++)
		snd_ctl_elem_value_set_integer(val, i, value);
	ret = snd_ctl_elem_write(ctl, val);
	snd_ctl_close(ctl);
	return ret < 0 ? ret : 0;
}

#define MAX_DEVICES 8

struct pw_profile;

struct pw_card {
	struct pw_profile *state;
	struct pw_device *proxy;
	struct spa_hook listener;
	bool chosen;
};

struct pw_profile {
	struct pw_main_loop *loop;
	struct pw_core *core;
	struct pw_registry *registry;
	struct spa_hook core_listener, registry_listener;
	struct pw_card devices[MAX_DEVICES];
	unsigned int count;
	char card[16];
	const char *want;
	int want_index, current_index;
	char current[256];
	int sync, error;
};

static void device_info(void *data, const struct pw_device_info *info)
{
	struct pw_card *device = data;
	const char *card = info->props ? spa_dict_lookup(info->props, "api.alsa.card") : NULL;

	if (card && !strcmp(card, device->state->card))
		device->chosen = true;
}

static void device_param(void *data, int seq, uint32_t id, uint32_t index, uint32_t next,
			 const struct spa_pod *param)
{
	struct pw_card *device = data;
	struct pw_profile *state = device->state;
	const char *name = NULL;
	int32_t profile = -1;

	if (!device->chosen ||
	    spa_pod_parse_object(param, SPA_TYPE_OBJECT_ParamProfile, NULL,
				 SPA_PARAM_PROFILE_index, SPA_POD_Int(&profile),
				 SPA_PARAM_PROFILE_name, SPA_POD_String(&name)) < 0)
		return;
	if (id == SPA_PARAM_EnumProfile && !strcmp(name, state->want))
		state->want_index = profile;
	if (id == SPA_PARAM_Profile) {
		state->current_index = profile;
		copy(state->current, sizeof(state->current), name);
	}
}

static const struct pw_device_events device_events = {
	PW_VERSION_DEVICE_EVENTS,
	.info = device_info,
	.param = device_param,
};

static void registry_global(void *data, uint32_t id, uint32_t permissions, const char *type,
			    uint32_t version, const struct spa_dict *props)
{
	struct pw_profile *state = data;
	const char *class = props ? spa_dict_lookup(props, PW_KEY_MEDIA_CLASS) : NULL;
	struct pw_card *device;

	if (strcmp(type, PW_TYPE_INTERFACE_Device) || !class || strcmp(class, "Audio/Device") ||
	    state->count == MAX_DEVICES)
		return;
	device = &state->devices[state->count++];
	device->state = state;
	device->proxy = pw_registry_bind(state->registry, id, type, PW_VERSION_DEVICE, 0);
	if (!device->proxy) {
		state->count--;
		return;
	}
	pw_device_add_listener(device->proxy, &device->listener, &device_events, device);
}

static const struct pw_registry_events registry_events = {
	PW_VERSION_REGISTRY_EVENTS,
	.global = registry_global,
};

static void core_done(void *data, uint32_t id, int seq)
{
	struct pw_profile *state = data;

	if (id == PW_ID_CORE && seq == state->sync)
		pw_main_loop_quit(state->loop);
}

static void core_error(void *data, uint32_t id, int seq, int res, const char *message)
{
	struct pw_profile *state = data;

	if (id == PW_ID_CORE) {
		state->error = res < 0 ? res : -EIO;
		pw_main_loop_quit(state->loop);
	}
}

static const struct pw_core_events core_events = {
	PW_VERSION_CORE_EVENTS,
	.done = core_done,
	.error = core_error,
};

static int roundtrip(struct pw_profile *state)
{
	state->sync = pw_core_sync(state->core, PW_ID_CORE, state->sync);
	pw_main_loop_run(state->loop);
	return state->error;
}

/*
 * Sets the profile named `want` on the PipeWire device of ALSA card `card`,
 * without WirePlumber storing it as the card's choice. `previous` gets the
 * profile the card had.
 */
int callaudio_pw_profile(int card, const char *want, char *previous, size_t previous_len)
{
	struct pw_profile state = { .want = want, .want_index = -1, .current_index = -1 };
	struct pw_context *context;
	struct pw_card *chosen = NULL;
	int ret;

	pw_init(NULL, NULL);
	snprintf(state.card, sizeof(state.card), "%d", card);
	state.loop = pw_main_loop_new(NULL);
	if (!state.loop)
		return -errno;
	context = pw_context_new(pw_main_loop_get_loop(state.loop), NULL, 0);
	if (!context) {
		ret = -errno;
		goto loop;
	}
	state.core = pw_context_connect(context, NULL, 0);
	if (!state.core) {
		ret = -errno;
		goto context;
	}
	pw_core_add_listener(state.core, &state.core_listener, &core_events, &state);
	state.registry = pw_core_get_registry(state.core, PW_VERSION_REGISTRY, 0);
	pw_registry_add_listener(state.registry, &state.registry_listener, &registry_events,
				 &state);
	/* Globals, then each device's info. */
	ret = roundtrip(&state);
	if (!ret)
		ret = roundtrip(&state);
	for (unsigned int i = 0; !ret && i < state.count; i++)
		if (state.devices[i].chosen)
			chosen = &state.devices[i];
	if (!ret && !chosen)
		ret = -ENODEV;
	if (!ret) {
		pw_device_enum_params(chosen->proxy, 0, SPA_PARAM_EnumProfile, 0, UINT32_MAX, NULL);
		pw_device_enum_params(chosen->proxy, 0, SPA_PARAM_Profile, 0, UINT32_MAX, NULL);
		ret = roundtrip(&state);
	}
	if (!ret && state.want_index < 0)
		ret = -ENOENT;
	if (!ret) {
		copy(previous, previous_len, state.current);
		if (state.current_index != state.want_index) {
			uint8_t buffer[256];
			struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
			struct spa_pod *pod = spa_pod_builder_add_object(&b,
				SPA_TYPE_OBJECT_ParamProfile, SPA_PARAM_Profile,
				SPA_PARAM_PROFILE_index, SPA_POD_Int(state.want_index),
				SPA_PARAM_PROFILE_save, SPA_POD_Bool(false));

			pw_device_set_param(chosen->proxy, SPA_PARAM_Profile, 0, pod);
			ret = roundtrip(&state);
		}
	}
	for (unsigned int i = 0; i < state.count; i++) {
		spa_hook_remove(&state.devices[i].listener);
		pw_proxy_destroy((struct pw_proxy *)state.devices[i].proxy);
	}
	spa_hook_remove(&state.registry_listener);
	pw_proxy_destroy((struct pw_proxy *)state.registry);
	spa_hook_remove(&state.core_listener);
	pw_core_disconnect(state.core);
context:
	pw_context_destroy(context);
loop:
	pw_main_loop_destroy(state.loop);
	return ret;
}
