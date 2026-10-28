/* SPDX-License-Identifier: LGPL-2.1-or-later */
#include "goodix-worker-ops.h"
#include <assert.h>
#include <errno.h>
#include <string.h>

struct fake {
    uint32_t ids[10],count;
    const char *controls[8];unsigned control_index;
    unsigned capture,add,save,cancel,post,hat,start,group,sleeps,wakes,postmatch;
    FILE *records;bool postmatch_after_sample;
    unsigned fail_target,fail_command;int failure;
    bool foreign,duplicate,bad_count,missing_hat,reject_hat,unknown_match,capture_retry,continuation;
    unsigned add_retry;
};
static int exchange(void *opaque,uint32_t *p,size_t size)
{
    struct fake *f=opaque;
    assert(goodix_wire_valid(p,size));
    if (p[1]==f->fail_target && p[2]==f->fail_command) return f->failure;
    if (p[1]==1002 && p[2]==1) {p[8]=5;p[10]=0x4c;p[11]=0x50;}
    if (p[1]==1001 && p[2]==0) ((uint8_t *)p)[0x34]=1;
    if (p[1]==1002 && p[2]==8) f->sleeps++;
    if (p[1]==1002 && p[2]==9) f->wakes++;
    if (p[1]==1000 && p[2]==0) {
        f->group++;
        assert(p[8]==GOODIX_NATIVE_GROUP);
        assert(!strcmp((char *)p+0x24,"/data/vendor/goodix"));
    }
    if (p[1]==1000 && p[2]==7) {
        p[8]=f->bad_count?11:f->count;
        for (unsigned i=0;i<f->count;i++) {
            p[0x24/4+i]=f->foreign?0:GOODIX_NATIVE_GROUP;
            p[0x4c/4+i]=f->duplicate?f->ids[0]:f->ids[i];
        }
    }
    if (p[1]==1000 && p[2]==6) {
        assert(p[8]==GOODIX_NATIVE_GROUP && p[9]!=0);
        bool found=false;
        for (unsigned i=0;i<f->count;i++) if (f->ids[i]==p[9]) {
            memmove(f->ids+i,f->ids+i+1,(f->count-i-1)*4);f->count--;found=true;break;
        }
        assert(found);
    }
    if (p[1]==1000 && p[2]==14) {uint64_t v=1234;memcpy((char *)p+32,&v,8);}
    if (p[1]==1000 && p[2]==3) {
        f->start++;assert(p[8]==GOODIX_NATIVE_GROUP && ((uint8_t *)p)[0x24]==0);
        for (unsigned i=0;i<69;i++) assert(((uint8_t *)p)[0x25+i]==0xaa);
        if (f->reject_hat) return 1059;
    }
    if (p[1]==1002 && p[2]==3) {
        f->capture++;assert(p[9]==1 || p[9]==2);
        if (f->capture_retry && f->capture==1) return 1112;
        ((uint8_t *)p)[0x2e]=f->continuation;
    }
    if (p[1]==1001 && p[2]==1) {
        f->add++;
        assert(((uint8_t *)p)[0x1c0]==(f->add==1));
        if (f->add_retry && f->add==1) return f->add_retry;
        p[8]=GOODIX_NATIVE_GROUP;p[9]=77;
        uint16_t remaining=0;memcpy((char *)p+0x2c,&remaining,2);
    }
    if (p[1]==1001 && p[2]==2) {
        p[0x30/4]=GOODIX_NATIVE_GROUP;p[0x34/4]=f->unknown_match?999:f->ids[0];
        memset((char *)p+0x40,0xbb,69); /* synthetic returned token-shaped bytes */
    }
    if (p[1]==1000 && p[2]==9) {f->save++;assert(p[8]==GOODIX_NATIVE_GROUP && p[9]==77);f->ids[f->count++]=77;}
    if (p[1]==1000 && p[2]==8) f->cancel++;
    if (p[1]==1000 && p[2]==4) f->post++;
    if (p[1]==1000 && p[2]==12) {
        f->postmatch++;
        assert(p[10]==GOODIX_NATIVE_GROUP && p[11]==f->ids[0]);
        if (f->records) {
            char line[256];long at=ftell(f->records);rewind(f->records);
            while (fgets(line,sizeof(line),f->records)) f->postmatch_after_sample|=!strncmp(line,"UFW1 sample 0 ",14);
            fseek(f->records,at,SEEK_SET);
        }
    }
    return 0;
}
static int control(void *opaque,char *line,size_t capacity,int timeout)
{
    struct fake *f=opaque;(void)timeout;
    assert(f->control_index<8);
    const char *command=f->controls[f->control_index++];
    if (!command) return -ECANCELED;
    assert(strlen(command)<capacity);strcpy(line,command);return 0;
}
static int hat(void *opaque,uint8_t token[69])
{
    struct fake *f=opaque;f->hat++;
    if (f->missing_hat) return -ENOKEY;
    memset(token,0xaa,69);return 0;
}
static int authorize(void *opaque,uint64_t challenge,uint8_t token[69])
{
    assert(challenge==1234);
    return hat(opaque,token);
}
static struct goodix_worker worker(struct fake *f,FILE *records)
{
    return (struct goodix_worker){.context=f,.exchange=exchange,.control=control,.hat=hat,.records=records,.slide_enroll=true};
}
int main(void)
{
    FILE *records=tmpfile();assert(records);
    struct fake f={0};struct goodix_worker w=worker(&f,records);
    assert(!goodix_worker_prepare(&w) && w.initialized && w.sleeping && w.calibration_state==1);
    f=(struct fake){.fail_target=1002,.fail_command=1,.failure=1004};w=worker(&f,records);
    assert(goodix_worker_prepare(&w)==1004 && !w.initialized);
    puts("PASS: calibrated startup and fatal detection failure");
    f=(struct fake){.ids={11,12},.count=2};w=worker(&f,records);
    assert(!goodix_worker_operate(&w,"list",0) && w.count==2);
    assert(!goodix_worker_operate(&w,"delete",11) && w.count==1 && w.ids[0]==12);
    assert(goodix_worker_operate(&w,"delete",0)==-ENOENT);
    assert(goodix_worker_operate(&w,"delete",999)==-ENOENT);
    for (unsigned bad=0;bad<3;bad++) {
        f=(struct fake){.ids={11,12},.count=2};f.foreign=bad==0;f.duplicate=bad==1;f.bad_count=bad==2;
        w=worker(&f,records);assert(goodix_worker_database(&w)==-EPROTO);
    }
    puts("PASS: private group, enumeration validation and single-template deletion");
    f=(struct fake){.missing_hat=true};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"enroll",0)==-ENOKEY && !f.start && !f.capture && !f.save && f.post==1);
    f=(struct fake){.reject_hat=true};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"enroll",0)==1059 && f.start==1 && !f.capture && !f.save && f.post==1);
    puts("PASS: absent or rejected HAT never reaches capture or save");
    f=(struct fake){0};w=worker(&f,records);
    assert(!goodix_worker_operate(&w,"check-enroll",0) && f.start==1 && !f.capture && !f.save && f.cancel==1 && f.post==1);
    f=(struct fake){.controls={"capture","finish"}};w=worker(&f,records);
    assert(!goodix_worker_operate(&w,"enroll",0) && f.save==1 && w.identifier==77 && f.cancel==1 && f.post==1);
    f=(struct fake){.capture_retry=true,.controls={"capture","finish","capture","finish"}};w=worker(&f,records);
    assert(!goodix_worker_operate(&w,"enroll",0) && f.capture==2 && f.add==1 && f.save==1);
    const unsigned retries[] = {1013, 1014, 1047};
    for (unsigned i=0;i<sizeof(retries)/sizeof(retries[0]);i++) {
        f=(struct fake){.add_retry=retries[i],.controls={"capture","finish","capture","finish"}};w=worker(&f,records);
        assert(!goodix_worker_operate(&w,"enroll",0) && f.capture==2 && f.add==2 && f.save==1);
    }
    f=(struct fake){.controls={"capture","cancel"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"enroll",0)==-ECANCELED && !f.save && !w.identifier && f.cancel==1 && f.post==1);
    FILE *broker_records=tmpfile();assert(broker_records);
    f=(struct fake){.controls={"capture","finish"}};w=worker(&f,broker_records);w.authorize=authorize;
    assert(!goodix_worker_operate(&w,"enroll",0) && f.hat==1 && f.save==1);
    rewind(broker_records);char line[256];
    while (fgets(line,sizeof(line),broker_records)) assert(!strstr(line,"UFW1 challenge"));
    fclose(broker_records);
    f=(struct fake){.missing_hat=true};w=worker(&f,records);w.authorize=authorize;
    assert(goodix_worker_operate(&w,"enroll",0)==-ENOKEY && !f.start && !f.capture && !f.save && f.post==1);
    puts("PASS: authorized session, first-image retry and finish-before-save barrier");
    f=(struct fake){.count=1,.ids={11},.controls={"capture","finish"}};w=worker(&f,records);
    assert(!goodix_worker_operate(&w,"verify",0) && w.identifier==11 && !w.code && f.cancel==1);
    f=(struct fake){.count=1,.ids={11},.controls={"capture","cancel"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"verify",0)==-ECANCELED && !w.identifier && f.cancel==1);
    f=(struct fake){.count=1,.ids={11},.unknown_match=true,.controls={"capture","finish"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"verify",0)==-EPROTO && !w.identifier);
    f=(struct fake){.count=1,.ids={11},.fail_target=1001,.fail_command=2,.failure=1008,.controls={"capture","finish"}};w=worker(&f,records);
    assert(!goodix_worker_operate(&w,"verify",0) && w.code==1 && !w.identifier);
    puts("PASS: matched-ID validation, rejection and cancellation never publish a false match");
    FILE *armed=tmpfile();assert(armed);
    f=(struct fake){.count=1,.ids={11},.controls={"capture","finish"}};w=worker(&f,armed);
    assert(!goodix_worker_database(&w) && w.loaded && f.group==1);
    assert(!goodix_worker_operate(&w,"verify",0) && f.group==1 && w.identifier==11 && !w.active);
    rewind(armed);char said[256];bool captured=false,sampled=false;
    while (fgets(said,sizeof(said),armed)) {
        if (!strcmp(said,"UFW1 captured\n")) {assert(!sampled);captured=true;}
        if (!strncmp(said,"UFW1 sample ",12)) sampled=true;
    }
    assert(captured && sampled);fclose(armed);
    f=(struct fake){.count=1,.ids={11},.controls={"capture","finish"}};w=worker(&f,records);
    assert(!goodix_worker_operate(&w,"enroll",0) && f.group==2);
    puts("PASS: armed verification keeps its loaded group and says when the image is taken");
    f=(struct fake){0};w=worker(&f,records);w.initialized=w.sleeping=true;
    assert(!goodix_worker_irq(&w) && w.sleeping && f.wakes==1 && f.sleeps==1);
    w.active=true;f.wakes=f.sleeps=0;
    assert(!goodix_worker_irq(&w) && !w.sleeping && f.wakes==1 && !f.sleeps);
    puts("PASS: an interrupt during an open session leaves the sensor awake for its capture");
    FILE *matched=tmpfile();assert(matched);
    f=(struct fake){.count=1,.ids={11},.records=matched,.controls={"capture","finish"}};w=worker(&f,matched);
    assert(!goodix_worker_operate(&w,"verify",0) && w.identifier==11 && f.postmatch==1 && f.postmatch_after_sample);
    fclose(matched);
    f=(struct fake){.count=1,.ids={11},.fail_target=1001,.fail_command=2,.failure=1008,.controls={"capture","rest"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"verify",0)==-ECANCELED && w.resting && f.cancel==1 && w.sleeping && !f.postmatch && !w.code);
    f=(struct fake){.count=1,.ids={11},.controls={"rest"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"verify",0)==-ECANCELED && w.resting && f.cancel==1 && !f.capture);
    f=(struct fake){.count=1,.ids={11},.controls={"capture","rest"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"verify",0)==-ECANCELED && !w.resting && !w.identifier);
    f=(struct fake){.count=1,.ids={11},.controls={"capture","rest"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"enroll",0)==-ECANCELED && !w.resting && !f.save);
    puts("PASS: the post-match step follows the sample; only a verification without a match rests");
    f=(struct fake){.count=1,.ids={11},.continuation=true,.controls={"capture"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"verify",0)==-ETIMEDOUT && f.capture==11 && f.cancel==1 && !w.identifier);
    f=(struct fake){.count=1,.ids={11},.fail_target=1000,.fail_command=8,.failure=-EIO,.controls={"capture","finish"}};w=worker(&f,records);
    assert(goodix_worker_operate(&w,"verify",0)==-EIO);
    puts("PASS: continuation bound and cleanup failure cannot become successful operations");
    uint32_t unsafe[8]={0,1010,1};assert(!goodix_wire_valid(unsafe,sizeof(unsafe)));
    fclose(records);return 0;
}
