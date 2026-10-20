/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Native JIIOV ancapp64 wire format, recovered from Roadstr firmware.
 * Keep transport and libfprint types out of this interface.
 */
#pragma once
#include <stddef.h>
#include <stdint.h>

enum {
    ANC_REQUEST_SIZE = 576,
    ANC_RESPONSE_SIZE = 320,
    ANC_REQUEST_LOGICAL_SIZE = 564,
    ANC_RESPONSE_LOGICAL_SIZE = 296,
    ANC_FRAME_HEADER_SIZE = 44,
    ANC_JV0307_WIDTH = 148,
    ANC_JV0307_HEIGHT = 148,
    ANC_JV0307_PIXEL_BYTES = 43808,
};

enum anc_group {
    ANC_GROUP_SENSOR = 2,
    ANC_GROUP_ALGORITHM = 3,
    ANC_GROUP_EXTENSION = 5,
};

enum anc_algorithm_command {
    ANC_ALGORITHM_INIT = 1,
    ANC_ALGORITHM_DEINIT = 2,
    ANC_ENROLL_BEGIN = 3,
    ANC_ENROLL_END = 5, /* parameter 0 aborts; positive values finalize */
    ANC_VERIFY_BEGIN = 6,
    ANC_EXTRACT = 8,
    ANC_ENROLL_ADD = 9,
    ANC_COMPARE = 10,
    ANC_VERIFY_END = 12,
    ANC_TEMPLATE_LOAD = 13,
    ANC_TEMPLATE_SET_GROUP = 14,
    ANC_TEMPLATE_DELETE = 16,
    ANC_TEMPLATE_LIST = 17,
    ANC_ENROLL_TOTAL = 18,
    ANC_ALGORITHM_VERSION = 19,
};

struct anc_frame {
    const uint8_t *pixels;
    uint32_t pixel_bytes, width, height;
};

uint32_t anc_get_le32(const void *data);
void anc_put_le32(void *data, uint32_t value);
void anc_request_init(uint8_t request[ANC_REQUEST_SIZE], enum anc_group group,
                      uint32_t command, uint32_t parameter, uint64_t time_ms);
int anc_response_status(const void *response, size_t size, uint32_t *status);
int anc_jv0307_frame_parse(const void *buffer, size_t capacity, struct anc_frame *frame);
