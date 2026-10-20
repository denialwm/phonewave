/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Bounded encrypted template blobs only. The namespace is synthetic Linux UFF,
 * never an Android group or a firmware-provided host path. */
#include <dirent.h>
static int gp_open(struct uff_storage *s,const char *name,int writing)
{
    int fd=openat(s->directory,name,(writing?O_RDWR:O_RDONLY)|O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC);
    if (fd<0 && writing && errno==ENOENT) {
        /* A compromised app cannot fill the disk with arbitrary identifiers. */
        int scan=openat(s->directory,".",O_RDONLY|O_DIRECTORY|O_CLOEXEC);
        if (scan<0) return -1;
        DIR *dir=fdopendir(scan);
        if (!dir) {close(scan);return -1;}
        unsigned count=0;struct dirent *entry;
        while ((entry=readdir(dir))) if (!strncmp(entry->d_name,"uff-template-",13)) count++;
        closedir(dir);
        if (!strncmp(name,"uff-template-",13) && count>=15) {errno=ENOSPC;return -1;}
        fd=openat(s->directory,name,O_CREAT|O_EXCL|O_RDWR|O_CLOEXEC|O_NOFOLLOW,0600);
    }
    if (fd>=0) {
        struct stat st;
        if (storage_check_file(fd,&st)) {int saved=errno;close(fd);errno=saved;return -1;}
    }
    return fd;
}
static int gp_copy_backup(struct uff_storage *s,int source,const char *name)
{
    char backup[80];
    if (strlen(name)+4>=sizeof(backup)) {errno=EINVAL;return -1;}
    snprintf(backup,sizeof(backup),"%s.bak",name);
    /* Bound backup depth to the two explicitly supported recovery copies. */
    if (strstr(name,".bak.bak")) {errno=EINVAL;return -1;}
    int target=gp_open(s,backup,1);
    if (target<0) return -1;
    unsigned char chunk[4096];off_t offset=0;int ret=0;
    for (;;) {
        ssize_t n=pread(source,chunk,sizeof(chunk),offset);
        if (n<0) {if(errno==EINTR)continue;ret=-1;break;}
        if (!n) break;
        ssize_t written=0;
        while (written<n) {
            ssize_t w=pwrite(target,chunk+written,n-written,offset+written);
            if (w<0 && errno==EINTR) continue;
            if (w<=0) {ret=-1;break;}
            written+=w;
        }
        if (ret) break;
        offset+=n;
    }
    explicit_bzero(chunk,sizeof(chunk));
    if (!ret && (ftruncate(target,offset) || fsync(target))) ret=-1;
    int saved=errno;close(target);errno=saved;return ret;
}
static int storage_gp(struct uff_storage *s,void *memory,size_t size)
{
    if (size<sizeof(tz_gpfile_write_req_t) || size>GPFILE_SERVICE_BUF_LEN) return 2;
    uint32_t cmd;memcpy(&cmd,memory,4);
    if (cmd==TZ_GPFS_MSG_CMD_GPFS_VERSION) {
        memset(memory,0,size);tz_gpfile_version_rsp_t *out=memory;
        out->cmd_id=cmd;out->version=2;return 0;
    }
    if (cmd>11) return 2;
    unsigned char *request=malloc(size);
    if (!request) return 2;
    memcpy(request,memory,size);memset(memory,0,size);memcpy(memory,&cmd,4);
    uint32_t operation=cmd%4;
    char name[80];int ret=-1,fd=-1,error=0;uint32_t count=0;
    const char *path=(char *)request+4;
    storage_log_path(GPFILE_SERVICE_ID,cmd,path);
    if (gp_name(path,name)) goto done;
    if (operation<2) {
        const tz_gpfile_read_req_t *read_request=(void *)request;
        int32_t offset=read_request->offset;count=read_request->count;
        fprintf(stderr,"Private GP operation=%u offset=%d count=%u\n",operation,offset,count);
        size_t capacity=size-(operation==1?offsetof(tz_gpfile_write_req_t,buf):offsetof(tz_gpfile_read_rsp_t,buf));
        if (offset<0 || count>capacity || offset>UFF_STORAGE_LIMIT || count>(uint32_t)(UFF_STORAGE_LIMIT-offset)) {errno=EINVAL;goto done;}
        fd=gp_open(s,name,operation==1);
        if (fd<0) goto done;
        if (operation==1) {
            const tz_gpfile_write_req_t *r=(void *)request;
            fprintf(stderr,"Private GP backup=%u\n",r->backup);
            if (r->backup>1) {errno=EINVAL;goto done;}
            if (r->backup && gp_copy_backup(s,fd,name)) goto done;
            uint32_t written=0;
            while (written<count) {
                ssize_t n=pwrite(fd,request+offsetof(tz_gpfile_write_req_t,buf)+written,count-written,(off_t)offset+written);
                if (n<0 && errno==EINTR) continue;
                if (n<=0) goto done;
                written+=n;
            }
            if (fsync(fd) || fsync(s->directory)) goto done;
            ((tz_gpfile_write_rsp_t *)memory)->num_bytes_written=written;
        } else {
            tz_gpfile_read_rsp_t *out=memory;uint32_t bytes=0;
            while (bytes<count) {
                ssize_t n=pread(fd,(unsigned char *)memory+offsetof(tz_gpfile_read_rsp_t,buf)+bytes,count-bytes,(off_t)offset+bytes);
                if (n<0 && errno==EINTR) continue;
                if (n<0) goto done;
                if (!n) break;
                bytes+=n;
            }
            out->num_bytes_read=bytes;
        }
        ret=0;
    } else {
        fd=gp_open(s,name,0);
        if (fd<0) goto done;
        if (operation==2) ret=unlinkat(s->directory,name,0);
        else {
            const tz_gpfile_rename_req_t *r=(void *)request;char destination[80];
            if (gp_name(r->to,destination)) goto done;
            int existing=gp_open(s,destination,0);
            if (existing>=0) close(existing);
            else if (errno!=ENOENT) goto done;
            ret=renameat(s->directory,name,s->directory,destination);
        }
        if (!ret) ret=fsync(s->directory);
    }
 done:
    if (ret) error=errno?errno:EIO;
    if (fd>=0) close(fd);
    ((tz_gpfile_err_rsp_t *)memory)->err=error;
    if (error) fprintf(stderr,"Private GP command=%u error=%d\n",cmd,error);
    explicit_bzero(request,size);free(request);return 0;
}
