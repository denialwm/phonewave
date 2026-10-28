/* SPDX-License-Identifier: GPL-2.0-only */
#include "uff-contact.h"
#include <assert.h>
#include <stdio.h>
static int event(struct uff_contact *c,int type,int code,int value)
{struct input_event e={.type=type,.code=code,.value=value};return uff_contact_event(c,&e);}
static int frame(struct uff_contact *c) {return event(c,EV_SYN,SYN_REPORT,0);}
static void finger(struct uff_contact *c,int slot,int id,int x,int y)
{
    assert(!event(c,EV_ABS,ABS_MT_SLOT,slot));
    assert(!event(c,EV_ABS,ABS_MT_TRACKING_ID,id));
    assert(!event(c,EV_ABS,ABS_MT_POSITION_X,x));
    assert(!event(c,EV_ABS,ABS_MT_POSITION_Y,y));
}
int main(void)
{
    struct uff_contact c;uff_contact_init(&c,&UFF_CONTACT_ASTON);
    finger(&c,0,1,6320,20250);assert(frame(&c)==1);assert(frame(&c)==0);
    assert(!event(&c,EV_ABS,ABS_MT_POSITION_X,0));assert(frame(&c)==2);
    assert(!event(&c,EV_ABS,ABS_MT_POSITION_X,6320));assert(frame(&c)==0);
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,-1));assert(frame(&c)==0);
    finger(&c,0,2,6320,20250);assert(frame(&c)==1);
    finger(&c,1,3,100,100);assert(frame(&c)==2);
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,-1));assert(frame(&c)==0);
    assert(!event(&c,EV_ABS,ABS_MT_SLOT,0));
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,-1));assert(frame(&c)==0);
    finger(&c,0,4,6320,20250);assert(frame(&c)==1);
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,5));assert(frame(&c)==2);
    assert(event(&c,EV_ABS,ABS_MT_SLOT,10)==-1);
    assert(event(&c,EV_ABS,ABS_MT_POSITION_X,12640)==-1);
    assert(event(&c,EV_ABS,ABS_MT_POSITION_Y,27800)==-1);
    assert(event(&c,EV_SYN,SYN_DROPPED,0)==-1);
    uff_contact_init(&c,&UFF_CONTACT_ASTON);c.armed=false;finger(&c,0,6,6320,20250);assert(frame(&c)==0);
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,-1));assert(frame(&c)==0);
    finger(&c,0,7,5350,19280);assert(frame(&c)==0); /* outside circular corner */
    /* NX809J: stock's circle at (608,2024) px, radius 120, on 12159x26879. */
    uff_contact_init(&c,&UFF_CONTACT_NX809J);
    finger(&c,0,8,6080,20240);assert(frame(&c)==1);
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,-1));assert(frame(&c)==2);
    finger(&c,0,9,7270,20240);assert(frame(&c)==1); /* inside the right edge */
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,-1));assert(frame(&c)==2);
    finger(&c,0,10,4890,19050);assert(frame(&c)==0); /* outside circular corner */
    assert(!event(&c,EV_ABS,ABS_MT_TRACKING_ID,-1));assert(frame(&c)==0);
    assert(event(&c,EV_ABS,ABS_MT_POSITION_X,12160)==-1);
    assert(event(&c,EV_ABS,ABS_MT_POSITION_Y,26880)==-1);
    puts("Contact gate: PASS (frames, coalescing, early lift, motion, multitouch, ID replacement, stale startup, bounds, SYN_DROPPED)");
}
