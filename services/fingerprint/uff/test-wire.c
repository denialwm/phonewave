/* SPDX-License-Identifier: LGPL-2.1-or-later */
#include <assert.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include "uff-wire.h"
int main(void)
{
    uint32_t packet[0x240/4]={2,2004};
    assert(uff_wire_valid(packet,0xb8));
    assert(!uff_wire_valid(packet,0xb4));
    assert(!uff_wire_valid(packet,0xbc));
    packet[1]=2010;assert(uff_wire_valid(packet,0x78));
    assert(!uff_wire_valid(packet,0x80));
    packet[1]=0xffffffff;assert(!uff_wire_valid(packet,sizeof(packet)));
    packet[0]=5;packet[1]=20;assert(!uff_wire_valid(packet,0x38));
    packet[1]=21;packet[8]=10;packet[9]=0;packet[12]=10;
    assert(uff_wire_valid(packet,0x42));
    packet[9]=1;assert(!uff_wire_valid(packet,0x42));
    packet[9]=0;packet[8]=4*1024*1024+1;assert(!uff_wire_valid(packet,0x42));
    packet[8]=10;assert(!uff_wire_valid(packet,0x43));
    puts("Fixed wire lengths, unknown commands, factory export denial and calibration bounds: PASS");
}
