/* SPDX-License-Identifier: GPL-2.0-only */
/* Type-B touch contact gate over an under-display sensor's circle, on the
 * phone's touchscreen. No input injection or event grabbing. */
#ifndef UFF_CONTACT_H
#define UFF_CONTACT_H
#include <dirent.h>
#include <fcntl.h>
#include <linux/input.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#define UFF_CONTACT_SLOTS 10
/* The touchscreen's live axes, the panel in pixels, and the sensor's circle
 * as the square around it, in panel pixels. */
struct uff_contact_area {int max_x,max_y,panel_width,panel_height,left,top,size;};
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
    int x=(int)((long long)s->x*a->panel_width/(a->max_x+1))-a->left;
    int y=(int)((long long)s->y*a->panel_height/(a->max_y+1))-a->top;
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
/* The root helper says where the sensor sits, from Denial's profile:
 * UFF_CONTACT_AREA="panel_width panel_height left top size". */
static inline int uff_contact_area_from_env(struct uff_contact_area *a)
{
    const char *text=getenv("UFF_CONTACT_AREA");
    char extra;
    if (!text || sscanf(text,"%d %d %d %d %d %c",&a->panel_width,&a->panel_height,
                        &a->left,&a->top,&a->size,&extra)!=5) return -1;
    if (a->panel_width<1 || a->panel_height<1 || a->size<1 || a->left<0 || a->top<0 ||
        a->left+a->size>a->panel_width || a->top+a->size>a->panel_height) return -1;
    return 0;
}
/* The phone's touchscreen: the one direct multitouch device. Its current
 * slots are read in, so a finger already held when the child starts must
 * lift before it counts. Returns its descriptor, or -1. */
static inline int uff_contact_open(struct uff_contact *c,struct uff_contact_area *a)
{
    if (uff_contact_area_from_env(a)) return -1;
    DIR *directory=opendir("/dev/input");
    if (!directory) return -1;
    int found=-1;
    struct dirent *entry;
    while ((entry=readdir(directory))) {
        if (strncmp(entry->d_name,"event",5)) continue;
        int fd=openat(dirfd(directory),entry->d_name,O_RDONLY|O_NONBLOCK|O_CLOEXEC|O_NOFOLLOW);
        if (fd<0) continue;
        unsigned long props[(INPUT_PROP_CNT+63)/64]={0};
        struct input_absinfo slot,ax,ay;
        if (ioctl(fd,EVIOCGPROP(sizeof(props)),props)<0 ||
            !(props[INPUT_PROP_DIRECT/64]>>(INPUT_PROP_DIRECT%64)&1) ||
            ioctl(fd,EVIOCGABS(ABS_MT_SLOT),&slot) || ioctl(fd,EVIOCGABS(ABS_MT_POSITION_X),&ax) ||
            ioctl(fd,EVIOCGABS(ABS_MT_POSITION_Y),&ay) || slot.minimum!=0 ||
            slot.maximum>=UFF_CONTACT_SLOTS || slot.value<0 || slot.value>slot.maximum ||
            ax.minimum!=0 || ax.maximum<1 || ay.minimum!=0 || ay.maximum<1) {close(fd);continue;}
        if (found>=0) {close(fd);close(found);found=-1;break;} /* two touchscreens: ambiguous */
        a->max_x=ax.maximum;a->max_y=ay.maximum;
        uff_contact_init(c,a);c->slot=slot.value;
        int values[UFF_CONTACT_SLOTS+1];
        const int axes[]={ABS_MT_TRACKING_ID,ABS_MT_POSITION_X,ABS_MT_POSITION_Y};
        bool read_all=true;
        for (unsigned k=0;read_all && k<3;k++) {
            values[0]=axes[k];
            if (ioctl(fd,EVIOCGMTSLOTS(sizeof(values)),values)) {read_all=false;break;}
            for (unsigned i=0;i<UFF_CONTACT_SLOTS;i++) {
                if (k==0) {c->slots[i].id=i<=(unsigned)slot.maximum?values[i+1]:-1;if(c->slots[i].id>=0)c->armed=false;}
                else if (k==1) c->slots[i].x=values[i+1];
                else c->slots[i].y=values[i+1];
            }
        }
        if (!read_all) {close(fd);break;}
        found=fd;
    }
    closedir(directory);
    return found;
}
#endif
