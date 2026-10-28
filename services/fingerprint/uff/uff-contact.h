/* SPDX-License-Identifier: GPL-2.0-only */
/* Type-B touch contact gate over an under-display sensor's circle: the
 * Aston's S3910 and the NX809J's TCM. No input injection or event grabbing. */
#ifndef UFF_CONTACT_H
#define UFF_CONTACT_H
#include <linux/input.h>
#include <stdbool.h>
#include <string.h>
#define UFF_CONTACT_SLOTS 10
/* The touchscreen's exact live axes, ten units per panel pixel, and the
 * sensor's circle as the square around it, in panel pixels. */
struct uff_contact_area {int max_x,max_y,left,top,size;};
static const struct uff_contact_area UFF_CONTACT_ASTON={12639,27799,535,1928,195};
/* Stock's touch circle: centre (608,2024), radius 120. */
static const struct uff_contact_area UFF_CONTACT_NX809J={12159,26879,488,1904,240};
struct uff_touch_slot {int id,x,y;};
struct uff_contact {
    const struct uff_contact_area *area;
    struct uff_touch_slot slots[UFF_CONTACT_SLOTS];
    int slot,active_id;
    bool active,armed;
};
static void uff_contact_init(struct uff_contact *c,const struct uff_contact_area *area)
{
    memset(c,0,sizeof(*c));
    c->area=area;
    for (unsigned i=0;i<UFF_CONTACT_SLOTS;i++) c->slots[i].id=-1;
    c->active_id=-1;c->armed=true;
}
static bool uff_contact_inside(const struct uff_contact *c,const struct uff_touch_slot *s)
{
    const struct uff_contact_area *a=c->area;
    int x=s->x/10-a->left,y=s->y/10-a->top;
    if (x<0 || y<0 || x>=a->size || y>=a->size) return false;
    int dx=2*x+1-a->size,dy=2*y+1-a->size;
    return dx*dx+dy*dy<=a->size*a->size;
}
/* 1=new DOWN; 2=contact lost/UP; 0=no transition; -1=stream invalid.
 * An outside/multi contact must fully release before a new contact can arm.
 */
static int uff_contact_event(struct uff_contact *c,const struct input_event *e)
{
    if (e->type==EV_SYN && e->code==SYN_DROPPED) return -1;
    if (e->type==EV_ABS) {
        if (e->code==ABS_MT_SLOT) {
            if (e->value<0 || e->value>=UFF_CONTACT_SLOTS) return -1;
            c->slot=e->value;
        } else if (e->code==ABS_MT_TRACKING_ID) {
            if (e->value< -1) return -1;
            c->slots[c->slot].id=e->value;
        } else if (e->code==ABS_MT_POSITION_X) {
            if (e->value<0 || e->value>c->area->max_x) return -1;
            c->slots[c->slot].x=e->value;
        } else if (e->code==ABS_MT_POSITION_Y) {
            if (e->value<0 || e->value>c->area->max_y) return -1;
            c->slots[c->slot].y=e->value;
        }
    }
    if (e->type!=EV_SYN || e->code!=SYN_REPORT) return 0;
    int count=0,id=-1;bool inside=false;
    for (unsigned i=0;i<UFF_CONTACT_SLOTS;i++) if (c->slots[i].id>=0) {
        count++;id=c->slots[i].id;inside=uff_contact_inside(c,&c->slots[i]);
    }
    if (c->active) {
        if (count==1 && inside && id==c->active_id) return 0;
        c->active=false;c->armed=count==0;c->active_id=-1;return 2;
    }
    if (!count) {c->armed=true;return 0;}
    if (c->armed && count==1 && inside) {
        c->active=true;c->armed=false;c->active_id=id;return 1;
    }
    c->armed=false;return 0;
}
#endif
