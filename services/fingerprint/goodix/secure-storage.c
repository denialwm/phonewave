/* SPDX-License-Identifier: LGPL-2.1-or-later */
#define _GNU_SOURCE
#include "secure-storage.h"
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
#define FILE_LIMIT (4u*1024*1024)
#define COMPONENT 112
static uint32_t u32(const uint8_t *p)
{ return (uint32_t)p[0]|(uint32_t)p[1]<<8|(uint32_t)p[2]<<16|(uint32_t)p[3]<<24; }
static void put32(uint8_t *p,uint32_t n)
{for(unsigned i=0;i<4;i++)p[i]=n>>(8*i);}
static void erase(void *p,size_t n) {volatile uint8_t *b=p;while(n--)*b++=0;}
/* Opaque names emitted by SFS are exactly two bounded components. Android
 * absolute paths, dot components and arbitrary nested paths never resolve. */
static int names(const char *path,char dir[COMPONENT],char file[COMPONENT])
{
    size_t n=strnlen(path,TZ_CM_MAX_NAME_LEN);
    if(!n || n==TZ_CM_MAX_NAME_LEN)return -EINVAL;
    const char *slash=memchr(path,'/',n);
    if(!slash || slash==path || strchr(slash+1,'/'))return -EACCES;
    size_t a=slash-path,b=n-a-1;
    if(!b || a>=COMPONENT || b>=COMPONENT)return -EACCES;
    for(size_t i=0;i<n;i++){
        unsigned char c=path[i];
        if(c=='/' || (c>='A'&&c<='Z') || (c>='a'&&c<='z') || (c>='0'&&c<='9') || c=='_' || c=='-')continue;
        if(c=='.' && i>a && !strcmp(path+i,".bak"))break;
        return -EACCES;
    }
    memcpy(dir,path,a);dir[a]=0;memcpy(file,slash+1,b);file[b]=0;return 0;
}
static int private_dir(int root,const char *name,int create)
{
    if(create && mkdirat(root,name,0700) && errno!=EEXIST)return -1;
    int fd=openat(root,name,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);
    struct stat st;
    if(fd>=0 && (fstat(fd,&st) || st.st_uid!=geteuid() || (st.st_mode&077))){close(fd);errno=EACCES;return -1;}
    if(fd>=0 && create && fsync(root)){int e=errno;close(fd);errno=e;return -1;}
    return fd;
}
static int canonical_dir(int root,const char *name,int create)
{
    struct stat parent;
    if(fstat(root,&parent) || !S_ISDIR(parent.st_mode) || (parent.st_mode&0002)){errno=EACCES;return -1;}
    int created=0;
    if(create){if(!mkdirat(root,name,0700))created=1;else if(errno!=EEXIST)return -1;}
    int fd=openat(root,name,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);struct stat st;
    if(fd>=0 && (fstat(fd,&st) || (st.st_uid!=parent.st_uid && st.st_uid!=geteuid()) || (st.st_mode&0002))){close(fd);errno=EACCES;return -1;}
    if(fd>=0 && create){
        /* Newly created directories retain the canonical parent's owner,
         * allowing the stock secure-storage service to read them later. */
        if((created && fchown(fd,parent.st_uid,parent.st_gid)) || fsync(root)){
            int e=errno;close(fd);errno=e;return -1;}
    }
    return fd;
}
static int regular(int dir,const char *name,int private)
{
    int fd=openat(dir,name,O_RDONLY|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);struct stat st;
    if(fd>=0 && (fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_nlink!=1 || st.st_size<0 ||
        (uint64_t)st.st_size>FILE_LIMIT || (private && (st.st_uid!=geteuid() || (st.st_mode&077))))){close(fd);errno=EACCES;return -1;}
    return fd;
}
/* Only ENOENT falls through: a tampered overlay must not silently resurrect
 * older stock contents. No stock file is ever opened writable. */
static int source(struct secure_store *s,int persist,const char *dir,const char *file)
{
    int root=persist?s->persist:s->data;
    int canonical=persist && s->canonical;
    int d=canonical?canonical_dir(root,dir,0):private_dir(root,dir,0),fd=-1,e=errno;
    if(d>=0){fd=regular(d,file,!canonical);e=errno;close(d);}
    if(fd>=0 || e!=ENOENT){errno=e;return fd;}
    if(canonical || !persist || s->base<0){errno=ENOENT;return -1;}
    d=openat(s->base,dir,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);
    if(d<0)return -1;
    fd=regular(d,file,0);e=errno;close(d);errno=e;return fd;
}
static int write_all(int fd,const void *data,size_t n,off_t offset)
{
    const uint8_t *p=data;
    while(n){ssize_t z=pwrite(fd,p,n,offset);if(z<0 && errno==EINTR)continue;
        if(z<=0){if(!z)errno=EIO;return -1;}p+=z;n-=z;offset+=z;}
    return 0;
}
static int copy_file(int from,int to)
{
    uint8_t data[4096];off_t off=0;int ret=0;
    if(from<0)return 0;
    for(;;){ssize_t n=pread(from,data,sizeof(data),off);if(n<0 && errno==EINTR)continue;
        if(n<0){ret=-1;break;}if(!n)break;
        if(off+n>FILE_LIMIT){errno=EFBIG;ret=-1;break;}
        if(write_all(to,data,n,off)){ret=-1;break;}off+=n;}
    erase(data,sizeof(data));return ret;
}
/* One serialized listener owns each overlay. A stale transaction is never a
 * committed file; validate it before recovering after an interrupted write. */
static int transaction(int dir,int canonical)
{
    struct stat st;
    if(!fstatat(dir,".pending",&st,AT_SYMLINK_NOFOLLOW)){
        struct stat parent;
        if(fstat(dir,&parent) || !S_ISREG(st.st_mode) || st.st_nlink!=1 ||
            (st.st_uid!=geteuid() && (!canonical || st.st_uid!=parent.st_uid)) ||
            (st.st_mode&(canonical?0002:077))){errno=EACCES;return -1;}
        if(unlinkat(dir,".pending",0))return -1;
    }else if(errno!=ENOENT)return -1;
    return openat(dir,".pending",O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW,0600);
}
static int replace(int dir,const char *file,int previous,const void *data,size_t count,off_t off,int canonical)
{
    /* Refuse linked or nonregular destinations before rename as well. */
    int existing=regular(dir,file,!canonical);
    if(existing<0 && errno!=ENOENT)return -1;
    if(existing>=0)close(existing);
    int fd=transaction(dir,canonical);if(fd<0)return -1;
    int ret=copy_file(previous,fd);
    if(!ret && count)ret=write_all(fd,data,count,off);
    if(!ret && canonical){
        struct stat owner;
        int inherited=previous>=0?previous:dir;
        if(fstat(inherited,&owner) || fchown(fd,owner.st_uid,owner.st_gid) ||
            fchmod(fd,previous>=0?(owner.st_mode&0777):0600))ret=-1;
    }
    if(!ret)ret=fsync(fd);
    int e=errno;
    if(close(fd) && !ret){ret=-1;e=errno;}
    if(!ret && renameat(dir,".pending",dir,file)){ret=-1;e=errno;}
    if(!ret && fsync(dir)){ret=-1;e=errno;}
    if(ret)unlinkat(dir,".pending",0);
    errno=e;return ret;
}
int secure_gpfs(struct secure_store *s,void *memory,size_t size)
{
    if(size<sizeof(tz_gpfile_write_req_t) || size<sizeof(tz_gpfile_read_rsp_t))return -EINVAL;
    uint8_t *b=memory;uint32_t cmd=u32(b);
    if(cmd==TZ_GPFS_MSG_CMD_GPFS_VERSION){memset(b,0,size);put32(b,cmd);((tz_gpfile_version_rsp_t *)b)->version=2;return 0;}
    /* Diagnostic persist overlays cannot hide a deleted stock file. Only
     * canonical persist or private data directories support remove/rename. */
    int error=ENOSYS;uint32_t moved=0;
    uint8_t *request=malloc(sizeof(tz_gpfile_write_req_t));if(!request)return -ENOMEM;
    memcpy(request,b,sizeof(tz_gpfile_write_req_t));memset(b,0,size);put32(b,cmd);
    const tz_gpfile_write_req_t *w=(void *)request;
    const tz_gpfile_read_req_t *r=(void *)request;
    char dir[COMPONENT],file[COMPONENT];int src=-1,d=-1;
    int write=cmd==5 || cmd==9,persist=cmd>=8;
    if(cmd==6 || cmd==7 || (s->canonical && (cmd==10 || cmd==11))){
        int ret=names(r->pathname,dir,file);if(ret){error=-ret;goto done;}
        int canonical=persist && s->canonical;
        d=canonical?canonical_dir(s->persist,dir,0):private_dir(s->data,dir,0);
        if(d<0){error=errno;goto done;}
        src=regular(d,file,!canonical);if(src<0){error=errno;goto done;}
        if(cmd==6 || cmd==10){
            if(unlinkat(d,file,0) || fsync(d)){error=errno;goto done;}
        }else{
            const tz_gpfile_rename_req_t *rename=(void *)request;
            char to_dir[COMPONENT],to_file[COMPONENT];
            ret=names(rename->to,to_dir,to_file);if(ret){error=-ret;goto done;}
            if(strcmp(dir,to_dir)){error=EXDEV;goto done;}
            int existing=regular(d,to_file,!canonical);
            if(existing<0 && errno!=ENOENT){error=errno;goto done;}
            if(existing>=0)close(existing);
            if(renameat(d,file,d,to_file) || fsync(d)){error=errno;goto done;}
        }
        error=0;goto done;
    }
    if(cmd!=4 && cmd!=5 && cmd!=8 && cmd!=9)goto done;
    int ret=names(r->pathname,dir,file);if(ret){error=-ret;goto done;}
    if(r->offset<0 || r->count>TZ_GPFS_FILE_SIZE || (uint64_t)r->offset+r->count>FILE_LIMIT){error=EINVAL;goto done;}
    src=source(s,persist,dir,file);
    if(src<0 && (!write || errno!=ENOENT)){error=errno;goto done;}
    if(write){
        d=persist && s->canonical?canonical_dir(s->persist,dir,1):private_dir(persist?s->persist:s->data,dir,1);if(d<0){error=errno;goto done;}
        if(w->backup && src>=0){
            size_t n=strlen(file);char backup[COMPONENT];
            if(n+4>=sizeof(backup) || strstr(file,".bak")){error=EINVAL;goto done;}
            memcpy(backup,file,n);memcpy(backup+n,".bak",5);
            if(replace(d,backup,src,NULL,0,0,persist && s->canonical)){error=errno;goto done;}
        }
        if(replace(d,file,src,w->buf,w->count,w->offset,persist && s->canonical)){error=errno;goto done;}
        moved=r->count;
    }else{
        tz_gpfile_read_rsp_t *o=memory;
        while(moved<r->count){ssize_t n=pread(src,o->buf+moved,r->count-moved,(off_t)r->offset+moved);
            if(n<0 && errno==EINTR)continue;
            if(n<0){error=errno;goto done;}if(!n)break;moved+=(uint32_t)n;}
    }
    error=0;
 done:
    if(src>=0)close(src);
    if(d>=0)close(d);
    if(error)memset(b,0,size);
    put32(b,cmd);((tz_gpfile_err_rsp_t *)b)->err=error;
    if(!error)((tz_gpfile_read_rsp_t *)b)->num_bytes_read=moved;
    fprintf(stderr,"secure GPFS command=%u status=%d bytes=%u\n",cmd,error,error?0:moved);
    erase(request,sizeof(tz_gpfile_write_req_t));free(request);return 0;
}
int secure_rpmb(void *memory,size_t size,secure_rpmb_io io,void *context)
{
    if(size<24)return -EINVAL;
    uint8_t *b=memory;uint32_t cmd=u32(b),count=u32(b+4),len=u32(b+8),off=u32(b+12),version=u32(b+16);
    /* INIT uses a different reply header. Explicitly reject it at its
     * status field; never accidentally report success with an empty geometry. */
    if(cmd==0x101){uint32_t requested=u32(b+4);memset(b,0,size);put32(b,cmd);
        put32(b+4,requested>=2?2:1);put32(b+8,(uint32_t)-EPERM);
        fprintf(stderr,"secure RPMB initialization unsupported; no device I/O\n");return 0;}
    uint8_t request[512]={0};int status=-EPERM;
    if(size>=536 && cmd==0x102 && count && count<=32 && len==512 && off>=24 && off<=size-512 && 20+(uint64_t)count*512<=size){
        memcpy(request,b+off,512);unsigned type=(unsigned)request[510]<<8|request[511];
        unsigned blocks=(unsigned)request[506]<<8|request[507];
        if((type==4 && blocks==count) || (type==2 && count==1)){
            memset(b,0,size);status=io(context,0xb5,request,512);
            if(!status)status=io(context,0xa2,b+20,count*512);
        }
    }
    if(status)memset(b,0,size);
    put32(b,cmd);put32(b+4,status);put32(b+8,status?0:count*512);put32(b+12,20);put32(b+16,version);
    fprintf(stderr,"secure RPMB command=%#x status=%d\n",cmd,status);
    erase(request,sizeof(request));return 0;
}

/* The trusted app supplies the complete authenticated frames and controls
 * commit ordering. Host code never constructs a MAC, key or write counter. */
int secure_rpmb_authenticated(void *memory,size_t size,secure_rpmb_io io,void *context)
{
    if(size<24)return -EINVAL;
    uint8_t *b=memory;
    if(u32(b)!=0x103)return secure_rpmb(memory,size,io,context);
    uint32_t count=u32(b+4),len=u32(b+8),off=u32(b+12),version=u32(b+16),batch=u32(b+20);
    int status=-EINVAL;uint8_t *frames=NULL;uint8_t result[512]={0},read_result[512]={0};
    if(!count || count>32 || !batch || batch>count || count%batch || len!=count*512 ||
        off<24 || off>size || len>size-off || size<532)goto reply;
    /* Validate every frame before the first I/O, including later batches. */
    for(uint32_t i=0;i<count;i++){
        const uint8_t *f=b+off+i*512;
        if(f[510]!=0 || f[511]!=3){status=-EPERM;goto reply;}
    }
    frames=malloc(len);if(!frames){status=-ENOMEM;goto reply;}
    memcpy(frames,b+off,len);read_result[511]=5;
    status=0;
    for(uint32_t i=0;i<count;i+=batch){
        status=io(context,0xb5,frames+i*512,batch*512);if(status)break;
        status=io(context,0xb5,read_result,512);if(status)break;
        status=io(context,0xa2,result,512);if(status)break;
        /* Preserve the result for the trusted verifier, but stop before
         * another batch if hardware reports a failure or unexpected reply. */
        if(result[510]!=3 || result[511]!=0 || result[508] || result[509])break;
    }
 reply:
    memset(b,0,size);put32(b,0x103);put32(b+4,status);put32(b+8,status?0:512);put32(b+12,20);put32(b+16,version);
    if(!status)memcpy(b+20,result,512);
    fprintf(stderr,"secure RPMB authenticated commit transport=%d frames=%u batch=%u reply_type=%u result=%u\n",
        status,count,batch,(unsigned)result[510]<<8|result[511],(unsigned)result[508]<<8|result[509]);
    if(frames){erase(frames,len);free(frames);}
    erase(result,sizeof(result));erase(read_result,sizeof(read_result));
    return 0;
}
