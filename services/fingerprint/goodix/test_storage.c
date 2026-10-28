/* SPDX-License-Identifier: LGPL-2.1-or-later */
#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include "qcomtee_object.h"
#include "qcomtee_object_types.h"
static struct qcomtee_param in(void *p,size_t n) {return (struct qcomtee_param){.attr=QCOMTEE_UBUF_INPUT,.ubuf={p,n}};}
static struct qcomtee_param obj(struct qcomtee_object *o) {return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_INPUT,.object=o};}
static struct qcomtee_param newobj(void) {return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_OUTPUT};}
static int invoke(struct qcomtee_object *o,const char *s,uint32_t op,struct qcomtee_param *p,int n)
{(void)o;(void)s;(void)op;(void)p;(void)n;assert(!"Unexpected TEE call in host storage test");return -1;}
#include "goodix-storage.h"

static char *path(char buffer[256],const char *name)
{assert(strlen(name)<256);memset(buffer,0,256);strcpy(buffer,name);return buffer;}
int main(int argc,char **argv)
{
    assert(argc==2);assert(!chdir(argv[1]));umask(077);
    assert(!mkdir("calibration",0700) && !mkdir("store",0700));
    struct goodix_storage s={.calibration=open("calibration",O_RDONLY|O_DIRECTORY),.store=open("store",O_RDONLY|O_DIRECTORY)};
    assert(s.calibration>=0 && s.store>=0);
    int fd=openat(s.calibration,"cali_data_0.so",O_WRONLY|O_CREAT|O_EXCL,0600);
    assert(fd>=0 && write(fd,"factory",7)==7);close(fd);
    char input[256],name[80];
    assert(storage_name(path(input,"/persist/goodix/cali_data_0.so"),name)==0);
    fd=storage_openat(&s,input,O_RDONLY);assert(fd>=0);close(fd);
    assert(storage_openat(&s,input,O_RDWR|O_TRUNC)<0 && errno==EROFS);
    assert(storage_unlink(&s,input)<0 && errno==EROFS);
    assert(storage_rename(&s,input,path((char[256]){0},"/data/vendor/goodix/stolen"))<0 && errno==EROFS);
    static const char *bad[]={"/etc/shadow","/data/vendor/goodix/../calibration/cali_data_0.so","/data/vendor/goodix/.hidden", "/persist/goodix/not-calibration", "/data/vendor/goodix/sub/file"};
    for (unsigned i=0;i<sizeof(bad)/sizeof(*bad);i++) assert(storage_name(path(input,bad[i]),name)<0);
    memset(input,'x',sizeof(input));assert(storage_name(input,name)<0);
    puts("PASS: calibration read-only and unrecognized/traversal paths denied");
    path(input,"/data/vendor/goodix/template.bin");
    fd=storage_openat(&s,input,O_RDWR|O_CREAT|O_EXCL);assert(fd>=0 && write(fd,"native",6)==6);close(fd);
    assert(storage_openat(&s,input,O_RDWR|O_CREAT|O_EXCL)<0 && errno==EEXIST);
    fd=storage_openat(&s,input,O_RDWR);assert(fd>=0 && !storage_backup(&s,fd,input));
    assert(pwrite(fd,"fresh!",6,0)==6);close(fd);
    fd=storage_openat(&s,path(input,"/data/vendor/goodix/template.bin.bak"),O_RDONLY);
    char previous[8]={0};assert(fd>=0 && read(fd,previous,6)==6 && !strcmp(previous,"native"));close(fd);
    puts("PASS: private GPFS backups preserve the previous blob before overwrite");
    assert(!symlinkat("../calibration/cali_data_0.so",s.store,"symlink"));
    assert(storage_openat(&s,path(input,"/data/vendor/goodix/symlink"),O_RDWR|O_TRUNC)<0);
    assert(!linkat(s.calibration,"cali_data_0.so",s.store,"hardlink",0));
    assert(storage_openat(&s,path(input,"/data/vendor/goodix/hardlink"),O_RDWR|O_TRUNC)<0);
    assert(!mkfifoat(s.store,"fifo",0600));
    assert(storage_openat(&s,path(input,"/data/vendor/goodix/fifo"),O_RDONLY)<0);
    char factory[8]={0};fd=openat(s.calibration,"cali_data_0.so",O_RDONLY);
    assert(fd>=0 && read(fd,factory,7)==7 && !strcmp(factory,"factory"));close(fd);
    puts("PASS: symlinks, hard links and nonregular files cannot escape the private store");
    fd=openat(s.store,"oversized",O_RDWR|O_CREAT|O_EXCL,0600);assert(fd>=0);
    assert(!ftruncate(fd,GOODIX_STORE_LIMIT+1));close(fd);
    assert(storage_openat(&s,path(input,"/data/vendor/goodix/oversized"),O_RDWR)<0);
    for (unsigned i=0;i<GOODIX_STORE_FILES;i++) {
        snprintf(name,sizeof(name),"limit-%u",i);fd=openat(s.store,name,O_WRONLY|O_CREAT|O_EXCL,0600);assert(fd>=0);close(fd);
    }
    assert(storage_openat(&s,path(input,"/data/vendor/goodix/excess"),O_RDWR|O_CREAT)<0 && errno==ENOSPC);
    puts("PASS: file size/count bounds enforced; factory data unchanged");
    close(s.store);close(s.calibration);return 0;
}
