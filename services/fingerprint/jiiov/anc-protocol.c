/* SPDX-License-Identifier: LGPL-2.1-or-later */
#include "anc-protocol.h"
#include <string.h>

uint32_t anc_get_le32(const void *data)
{
    const uint8_t *p=data;
    return (uint32_t)p[0] | (uint32_t)p[1]<<8 | (uint32_t)p[2]<<16 | (uint32_t)p[3]<<24;
}

void anc_put_le32(void *data, uint32_t value)
{
    uint8_t *p=data;
    for (unsigned i=0;i<4;i++) p[i]=(uint8_t)(value>>(8*i));
}

void anc_request_init(uint8_t request[ANC_REQUEST_SIZE], enum anc_group group,
                      uint32_t command, uint32_t parameter, uint64_t time_ms)
{
    memset(request,0,ANC_REQUEST_SIZE);
    anc_put_le32(request,ANC_REQUEST_LOGICAL_SIZE);
    anc_put_le32(request+0x10,group);
    anc_put_le32(request+0x14,(uint32_t)time_ms);
    anc_put_le32(request+0x18,(uint32_t)(time_ms>>32));
    anc_put_le32(request+0x1c,command);
    size_t offset=0x20;
    if ((group==ANC_GROUP_SENSOR && command==4) ||
        (group==ANC_GROUP_ALGORITHM && command==ANC_TEMPLATE_SET_GROUP)) offset=0x24;
    if (group==ANC_GROUP_ALGORITHM && command==ANC_TEMPLATE_DELETE) offset=0x28;
    anc_put_le32(request+offset,parameter);
}

int anc_response_status(const void *response, size_t size, uint32_t *status)
{
    if (!response || !status || size<ANC_RESPONSE_LOGICAL_SIZE || size>ANC_RESPONSE_SIZE)
        return -1;
    *status=anc_get_le32(response);
    return 0;
}

int anc_jv0307_frame_parse(const void *buffer, size_t capacity, struct anc_frame *frame)
{
    if (!frame) return -1;
    memset(frame,0,sizeof(*frame));
    if (!buffer || capacity<ANC_FRAME_HEADER_SIZE) return -1;
    const uint8_t *p=buffer;
    uint32_t bytes=anc_get_le32(p), width=anc_get_le32(p+4), height=anc_get_le32(p+8);
    if (width!=ANC_JV0307_WIDTH || height!=ANC_JV0307_HEIGHT ||
        bytes!=ANC_JV0307_PIXEL_BYTES || bytes>capacity-ANC_FRAME_HEADER_SIZE)
        return -1;
    *frame=(struct anc_frame){p+ANC_FRAME_HEADER_SIZE,bytes,width,height};
    return 0;
}
