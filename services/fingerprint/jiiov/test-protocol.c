/* SPDX-License-Identifier: LGPL-2.1-or-later */
#include "anc-protocol.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main(void)
{
    uint8_t req[ANC_REQUEST_SIZE];
    /* Independent expected wire prefix for the verified product query. */
    const uint8_t expected[]={
        0x34,2,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0,
        2,0,0,0, 8,7,6,5, 4,3,2,1, 4,0,0,0,
        0,0,0,0, 0x3b,0,0,0,
    };
    memset(req,0xff,sizeof(req));
    anc_request_init(req,ANC_GROUP_SENSOR,4,0x3b,UINT64_C(0x0102030405060708));
    assert(!memcmp(req,expected,sizeof(expected)));
    for (size_t i=sizeof(expected);i<sizeof(req);i++) assert(req[i]==0);
    anc_request_init(req,ANC_GROUP_ALGORITHM,ANC_ENROLL_END,0,0);
    assert(anc_get_le32(req+0x1c)==5 && anc_get_le32(req+0x20)==0);
    anc_request_init(req,ANC_GROUP_ALGORITHM,ANC_TEMPLATE_SET_GROUP,0x12345678,0);
    assert(req[0x24]==0x78 && req[0x25]==0x56 && req[0x26]==0x34 && req[0x27]==0x12);
    assert(anc_get_le32(req+0x20)==0 && anc_get_le32(req+0x28)==0);
    anc_request_init(req,ANC_GROUP_ALGORITHM,ANC_TEMPLATE_DELETE,0x10203040,0);
    assert(anc_get_le32(req+0x28)==0x10203040 && anc_get_le32(req+0x20)==0);

    uint8_t rsp[ANC_RESPONSE_SIZE]={114};
    uint32_t status=0;
    assert(!anc_response_status(rsp,sizeof(rsp),&status) && status==114);
    assert(anc_response_status(rsp,ANC_RESPONSE_LOGICAL_SIZE-1,&status)<0);
    assert(anc_response_status(rsp,sizeof(rsp)+1,&status)<0);
    assert(anc_response_status(NULL,sizeof(rsp),&status)<0);

    uint8_t storage[ANC_FRAME_HEADER_SIZE+ANC_JV0307_PIXEL_BYTES+1]={0};
    /* Deliberately unaligned input, as the firmware header is packed. */
    uint8_t *buf=storage+1;
    anc_put_le32(buf,43808); anc_put_le32(buf+4,148); anc_put_le32(buf+8,148);
    struct anc_frame frame;
    const size_t size=sizeof(storage)-1;
    assert(!anc_jv0307_frame_parse(buf,size,&frame));
    assert(frame.pixels==buf+44 && frame.pixel_bytes==43808);
    for (size_t i=0;i<size;i++) {
        assert(anc_jv0307_frame_parse(buf,i,&frame)<0);
        assert(frame.pixels==NULL);
    }
    anc_put_le32(buf,UINT32_MAX);
    assert(anc_jv0307_frame_parse(buf,size,&frame)<0);
    anc_put_le32(buf,43808); anc_put_le32(buf+4,UINT32_MAX);
    assert(anc_jv0307_frame_parse(buf,size,&frame)<0);
    puts("ANC packet vectors and malformed/truncated frame tests passed");
    return 0;
}
