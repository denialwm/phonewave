/* SPDX-License-Identifier: MIT */
/* ARGS reads Android properties; on the workstation each one is its default. */
#pragma once
#include <string.h>
#define PROPERTY_VALUE_MAX 92
static inline int property_get(const char *key, char *value, const char *def)
{
	(void)key;
	if (def) {
		strncpy(value, def, PROPERTY_VALUE_MAX - 1);
		value[PROPERTY_VALUE_MAX - 1] = 0;
		return (int)strlen(value);
	}
	value[0] = 0;
	return 0;
}
static inline int property_set(const char *key, const char *value)
{
	(void)key;
	(void)value;
	return 0;
}
