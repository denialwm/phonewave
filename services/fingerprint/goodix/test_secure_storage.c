/* SPDX-License-Identifier: LGPL-2.1-or-later */
#define _GNU_SOURCE
#include "secure-storage.h"
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#undef MSGV
#undef MSGD
#include "gpfs_msg.h"
static void put(uint8_t *p,uint32_t n){for(unsigned i=0;i<4;i++)p[i]=n>>(8*i);}
static uint32_t get(uint8_t *p){return (uint32_t)p[0]|(uint32_t)p[1]<<8|(uint32_t)p[2]<<16|(uint32_t)p[3]<<24;}
static int io_calls;
static int fake_io(void *ctx,unsigned op,void *buf,uint32_t n)
{
    (void)ctx;assert(op==(io_calls%2?0xa2:0xb5));
    if(op==0xb5){uint8_t *b=buf;assert(n==512 && b[510]==0 && (b[511]==2 || b[511]==4));}
    else{assert(n==512);memset(buf,0x5a,n);}
    io_calls++;return 0;
}
static int commit_calls,fail_commit;
static int commit_io(void *ctx,unsigned op,void *buf,uint32_t n)
{
    (void)ctx;uint8_t *b=buf;
    if(commit_calls%3==0){assert(op==0xb5 && (n==512 || n==1024));
        for(uint32_t i=0;i<n;i+=512)assert(b[i+510]==0 && b[i+511]==3);
    }else if(commit_calls%3==1){assert(op==0xb5 && n==512 && b[511]==5);}
    else{assert(op==0xa2 && n==512);memset(b,0,512);b[510]=3;}
    commit_calls++;return fail_commit?-EIO:0;
}
static void rpmb_request(uint8_t *b,unsigned cmd,unsigned type)
{memset(b,0,20040);put(b,cmd);put(b+4,1);put(b+8,512);put(b+12,24);b[24+507]=1;b[24+511]=type;}
static void content(int dir,const char *path,const char *expected)
{char b[32]={0};int fd=openat(dir,path,O_RDONLY|O_CLOEXEC);assert(fd>=0);assert(read(fd,b,sizeof(b))==(ssize_t)strlen(expected));assert(!strcmp(b,expected));close(fd);}
int main(int argc,char **argv)
{
    assert(argc==2 && !chdir(argv[1]));umask(077);
    assert(!mkdir("stock",0700) && !mkdir("persist",0700) && !mkdir("data",0700));
    struct secure_store s={.base=open("stock",O_RDONLY|O_DIRECTORY),.persist=open("persist",O_RDONLY|O_DIRECTORY),.data=open("data",O_RDONLY|O_DIRECTORY)};
    assert(s.base>=0 && s.persist>=0 && s.data>=0);
    assert(!mkdirat(s.base,"Namespace",0700));
    int fd=openat(s.base,"Namespace/File",O_WRONLY|O_CREAT|O_EXCL,0600);assert(fd>=0 && write(fd,"stock",5)==5);close(fd);
    uint8_t *memory=calloc(1,GPFILE_SERVICE_BUF_LEN);assert(memory);
    tz_gpfile_write_req_t *w=(void *)memory;tz_gpfile_read_rsp_t *r=(void *)memory;
    w->cmd_id=8;strcpy(w->pathname,"Namespace/File");w->count=5;
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err && r->num_bytes_read==5 && !memcmp(r->buf,"stock",5));
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,"Namespace/File");w->offset=1;w->count=2;memcpy(w->buf,"XX",2);w->backup=1;
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err && r->num_bytes_read==2);
    content(s.base,"Namespace/File","stock");content(s.persist,"Namespace/File","sXXck");content(s.persist,"Namespace/File.bak","stock");
    close(s.persist);s.persist=open("persist",O_RDONLY|O_DIRECTORY);assert(s.persist>=0);
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=8;strcpy(w->pathname,"Namespace/File");w->count=5;
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err && !memcmp(r->buf,"sXXck",5));
    puts("PASS: stock read-through, atomic private updates, backups and reopen persistence");
    const char *bad[]={"../stock/Namespace/File","Namespace/../File","/Namespace/File","Namespace/File/extra","Namespace/.file","Namespace/File.bak.bak"};
    for(unsigned i=0;i<sizeof(bad)/sizeof(*bad);i++){
        memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,bad[i]);w->count=1;w->buf[0]='!';
        assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err);
    }
    assert(!symlinkat("../../stock/Namespace/File",s.persist,"Namespace/Symlink"));
    assert(!linkat(s.base,"Namespace/File",s.persist,"Namespace/Hardlink",0));
    assert(!mkfifoat(s.persist,"Namespace/Fifo",0600));
    for(unsigned i=0;i<3;i++){
        const char *paths[]={"Namespace/Symlink","Namespace/Hardlink","Namespace/Fifo"};
        memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,paths[i]);w->count=1;
        assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err);
    }
    /* Hard link changes only the fixture's link count; remove before final check. */
    assert(!unlinkat(s.persist,"Namespace/Hardlink",0));
    int d=openat(s.persist,"Namespace",O_RDONLY|O_DIRECTORY);assert(d>=0);
    assert(!symlinkat("../../stock/Namespace/File",d,".pending"));
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,"Namespace/File");w->count=1;
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err);
    assert(!unlinkat(d,".pending",0));close(d);
    content(s.base,"Namespace/File","stock");content(s.persist,"Namespace/File","sXXck");
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,"Namespace/File");w->offset=-1;w->count=1;
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err==EINVAL);
    for(unsigned cmd=10;cmd<=11;cmd++){memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=cmd;assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err==ENOSYS);}
    puts("PASS: traversal, links, special files, transaction attacks and destructive operations denied");
    uint8_t *rpmb=calloc(1,20040);assert(rpmb);
    for(unsigned type=1;type<=5;type++){
        rpmb_request(rpmb,0x102,type);int before=io_calls;
        assert(!secure_rpmb(rpmb,20040,fake_io,NULL));
        assert(io_calls-before==((type==2||type==4)?2:0));
        assert((int32_t)get(rpmb+4)==((type==2||type==4)?0:-EPERM));
    }
    rpmb_request(rpmb,0x101,0);int init_before=io_calls;
    assert(!secure_rpmb(rpmb,20040,fake_io,NULL) && io_calls==init_before && (int32_t)get(rpmb+8)==-EPERM);
    rpmb_request(rpmb,0x103,3);int before=io_calls;assert(!secure_rpmb(rpmb,20040,fake_io,NULL) && io_calls==before && (int32_t)get(rpmb+4)==-EPERM);
    rpmb_request(rpmb,0x102,4);put(rpmb+12,UINT32_MAX);assert(!secure_rpmb(rpmb,20040,fake_io,NULL) && io_calls==before);
    rpmb_request(rpmb,0x102,4);put(rpmb+4,UINT32_MAX);assert(!secure_rpmb(rpmb,20040,fake_io,NULL) && io_calls==before);
    rpmb_request(rpmb,0x102,4);assert(!secure_rpmb(rpmb,24,fake_io,NULL) && io_calls==before);
    for(size_t n=0;n<24;n++)assert(secure_rpmb(rpmb,n,fake_io,NULL)==-EINVAL);
    puts("PASS: only RPMB reads reach transport; key programming, writes and malformed frames denied");
    rpmb_request(rpmb,0x103,3);put(rpmb+20,1);
    assert(!secure_rpmb_authenticated(rpmb,20040,commit_io,NULL) && commit_calls==3 && !get(rpmb+4) && get(rpmb+8)==512 && rpmb[20+510]==3);
    rpmb_request(rpmb,0x103,1);put(rpmb+20,1);
    assert(!secure_rpmb_authenticated(rpmb,20040,commit_io,NULL) && commit_calls==3 && (int32_t)get(rpmb+4)==-EPERM);
    rpmb_request(rpmb,0x103,3);put(rpmb+4,2);put(rpmb+8,1024);put(rpmb+20,1);rpmb[24+512+511]=1;
    assert(!secure_rpmb_authenticated(rpmb,20040,commit_io,NULL) && commit_calls==3 && (int32_t)get(rpmb+4)==-EPERM);
    rpmb_request(rpmb,0x103,3);put(rpmb+20,1);fail_commit=1;
    assert(!secure_rpmb_authenticated(rpmb,20040,commit_io,NULL) && commit_calls==4 && (int32_t)get(rpmb+4)==-EIO && !get(rpmb+8));
    puts("PASS: opt-in commits forward only trusted authenticated frames; no write retry or key programming");
    /* Canonical mode is an explicit alternative, never an overlay combined
     * with a hardware commit. Preserve existing access metadata. */
    s.canonical=1;close(s.persist);s.persist=dup(s.base);
    fd=openat(s.base,"Namespace/File",O_RDONLY);assert(fd>=0);struct stat old,new;
    assert(!fstat(fd,&old));close(fd);
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,"Namespace/File");w->offset=0;w->count=1;w->buf[0]='C';
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err);
    content(s.base,"Namespace/File","Ctock");
    fd=openat(s.base,"Namespace/File",O_RDONLY);assert(fd>=0 && !fstat(fd,&new));close(fd);
    assert(old.st_uid==new.st_uid && old.st_gid==new.st_gid && (old.st_mode&0777)==(new.st_mode&0777));
    puts("PASS: explicit canonical writes preserve file ownership/mode and use no diagnostic overlay");
    /* Canonical transaction cleanup must work without ever following links. */
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,"Namespace/Temporary");w->count=4;memcpy(w->buf,"temp",4);
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err);
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);tz_gpfile_rename_req_t *rename=(void *)memory;
    rename->cmd_id=11;strcpy(rename->from,"Namespace/Temporary");strcpy(rename->to,"Namespace/Renamed");
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err);content(s.base,"Namespace/Renamed","temp");
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);rename->cmd_id=11;strcpy(rename->from,"Namespace/Renamed");strcpy(rename->to,"OtherNamespace/Renamed");
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err==EXDEV);
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=10;strcpy(w->pathname,"Namespace/Renamed");
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err);
    assert(faccessat(s.base,"Namespace/Renamed",F_OK,0)<0 && errno==ENOENT);
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=10;strcpy(w->pathname,"Namespace/Renamed");
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err==ENOENT);
    assert(!symlinkat("../../persist/Namespace/File",s.base,"Namespace/Unsafe"));
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=10;strcpy(w->pathname,"Namespace/Unsafe");
    assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && r->err);
    puts("PASS: canonical remove/rename is durable, namespace-confined and rejects symlinks");
    if(!geteuid()){
        /* Match Android's canonical owner while the service runs as root. */
        assert(!fchown(s.base,1000,1000));
        d=openat(s.base,"Namespace",O_RDONLY|O_DIRECTORY);assert(d>=0 && !fchown(d,1000,1000));close(d);
        fd=openat(s.base,"Namespace/File",O_RDONLY);assert(fd>=0 && !fchown(fd,1000,1000) && !fchmod(fd,0640));close(fd);
        memset(memory,0,GPFILE_SERVICE_BUF_LEN);w->cmd_id=9;strcpy(w->pathname,"Namespace/File");w->offset=0;w->count=1;w->buf[0]='R';
        assert(!secure_gpfs(&s,memory,GPFILE_SERVICE_BUF_LEN) && !r->err);
        fd=openat(s.base,"Namespace/File",O_RDONLY);assert(fd>=0 && !fstat(fd,&new));close(fd);
        assert(new.st_uid==1000 && new.st_gid==1000 && (new.st_mode&0777)==0640);
        puts("PASS: root service preserves Android ownership and permissions");
    }
    free(rpmb);free(memory);close(s.base);close(s.persist);close(s.data);return 0;
}
