/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Packet lengths resolved from this device's factory CA, factory-command-map.json. */
#ifndef UFF_WIRE_H
#define UFF_WIRE_H
static int uff_wire_valid(const uint32_t *p,size_t size)
{
    if (size<32) return 0;
    if (p[0]==2) {
        switch(p[1]) {
        case 2000:return size==0x1c0;
        case 2002:return size==0x100;
        case 2003:return size==0xa0;
        case 2004:return size==0xb8;
        case 2005:return size==0x80;
        case 2006:return size==0xc8;
        case 2007:return size==0x90;
        case 2008:return size==0xb0;
        case 2009:return size==0x80;
        case 2010:return size==0x78;
        case 2011:return size==0x80;
        case 2012:return size==0x88;
        case 2013:return size==0xa0;
        case 2014:return size==0xf0;
        case 2015:return size==0x80;
        case 2016:return size==0x78;
        case 2017:return size==0x80;
        case 2018:return size==0x98;
        case 2019:return size==0x30;
        case 2020:return size==0x130;
        case 2048:return size==0x2968;
        case 2100:return size==0xb0;
        case 2101:return size==0x240;
        case 2102:return size==0x30;
        case 2200:return size==0x188;
        case 2204:return size==0x188;
        case 2205:return size==0x188;
        case 2206:return size==0x188;
        case 5000:return size==0xa0;
        default:return 0;
        }
    }
    /* Protected calibration import is the only permitted factory-group command. */
    if (p[0]==5 && p[1]==21 && size>=0x38 && size<=1024*1024+0x38)
        return p[8]>0 && p[8]<=4*1024*1024 && p[9]<=p[8] &&
            p[12]<=p[8]-p[9] && size==(size_t)p[12]+0x38;
    return 0;
}
#endif
