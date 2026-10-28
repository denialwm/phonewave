/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Host file services for goodixfp64: the FS (0x0a) and GPFS (0x7000)
 * listeners. The three factory calibration names are served read-only from
 * this unit's persist copy. Everything under the template root lives in a
 * private Linux directory, bounded in names, count and size. No other path
 * is opened, and only path metadata is logged, never contents.
 * Wire definitions: Qualcomm minkipc libfsservice headers.
 */
#include <dirent.h>
#include <pthread.h>
#include <stddef.h>
#undef MSGV
#undef MSGD
#include "fs_msg.h"
#include "gpfs_msg.h"

#define GOODIX_TEMPLATE_ROOT "/data/vendor/goodix/"
#define GOODIX_STORE_LIMIT (4*1024*1024)
#define GOODIX_STORE_FILES 64

struct goodix_storage;
struct goodix_listener {
    struct qcomtee_object callback;
    struct qcomtee_object *memory;
    struct goodix_storage *storage;
    size_t size;
    uint32_t id,is64;
};
struct goodix_storage {
    struct qcomtee_object *registration;
    struct goodix_listener *listeners[2];
    pthread_t worker;
    int started,initialized,calibration,store,files[8],last_errno;
    unsigned char writable[8];
};

/* Which directory a TA path names: 0 calibration (read-only), 1 private
 * store, -1 denied. The basename is copied to name. */
static int storage_name(const char *path,char name[80])
{
    static const char *calibration[]={"cali_data_0.so","cali_data_0.so.bak","cali_data_extra_0.so.bak"};
    static const char *persist[]={"/mnt/vendor/persist/goodix/","/persist/goodix/"};
    if (!memchr(path,0,TZ_CM_MAX_NAME_LEN)) {errno=EINVAL;return -1;}
    for (size_t j=0;j<sizeof(persist)/sizeof(*persist);j++) {
        size_t n=strlen(persist[j]);
        if (strncmp(path,persist[j],n)) continue;
        for (size_t i=0;i<sizeof(calibration)/sizeof(*calibration);i++)
            if (!strcmp(path+n,calibration[i])) {snprintf(name,80,"%s",calibration[i]);return 0;}
    }
    size_t n=strlen(GOODIX_TEMPLATE_ROOT);
    if (strncmp(path,GOODIX_TEMPLATE_ROOT,n)) {errno=EACCES;return -1;}
    const char *base=path+n;size_t len=strlen(base);
    if (!len || len>=64 || base[0]=='.') {errno=EACCES;return -1;}
    for (size_t i=0;i<len;i++) {
        char c=base[i];
        if (!((c>='a'&&c<='z')||(c>='A'&&c<='Z')||(c>='0'&&c<='9')||c=='.'||c=='_'||c=='-')) {errno=EACCES;return -1;}
    }
    snprintf(name,80,"%s",base);return 1;
}
static void storage_log(uint32_t id,uint32_t cmd,const char *path)
{
    fprintf(stderr,"storage listener=%#x command=%#x path=\"",id,cmd);
    for (size_t i=0;i<TZ_CM_MAX_NAME_LEN && path[i];i++) {
        unsigned char c=path[i];
        if (c>=32 && c<127 && c!='"' && c!='\\') fputc(c,stderr);
        else fprintf(stderr,"\\x%02x",c);
    }
    fputs("\"\n",stderr);
}
static int storage_check(int fd,struct stat *st)
{
    if (fstat(fd,st)) return -1;
    if (!S_ISREG(st->st_mode) || st->st_size<0 || st->st_size>GOODIX_STORE_LIMIT) {errno=EACCES;return -1;}
    return 0;
}
static int storage_count(struct goodix_storage *s)
{
    int scan=openat(s->store,".",O_RDONLY|O_DIRECTORY|O_CLOEXEC);
    if (scan<0) return -1;
    DIR *dir=fdopendir(scan);
    if (!dir) {close(scan);return -1;}
    int count=0;struct dirent *e;
    while ((e=readdir(dir))) if (e->d_name[0]!='.') count++;
    closedir(dir);return count;
}
/* Opens a mapped name. Calibration never opens for writing; a new private
 * file is created only within the file-count bound. */
static int storage_openat(struct goodix_storage *s,const char *path,int flags)
{
    char name[80];int where=storage_name(path,name);
    if (where<0) return -1;
    int writing=(flags&O_ACCMODE)!=O_RDONLY || (flags&(O_CREAT|O_TRUNC));
    if (!where && writing) {errno=EROFS;return -1;}
    int dir=where?s->store:s->calibration;
    int fd=openat(dir,name,(flags&~(O_TRUNC|O_CREAT|O_EXCL))|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
    if (fd>=0 && (flags&(O_CREAT|O_EXCL))==(O_CREAT|O_EXCL)) {close(fd);errno=EEXIST;return -1;}
    if (fd<0 && errno==ENOENT && where && (flags&O_CREAT)) {
        int count=storage_count(s);
        if (count<0) return -1;
        if (count>=GOODIX_STORE_FILES) {errno=ENOSPC;return -1;}
        fd=openat(dir,name,(flags&(O_ACCMODE|O_APPEND))|O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW,0600);
    }
    if (fd<0) return -1;
    struct stat st;
    if (storage_check(fd,&st)) {int e=errno;close(fd);errno=e;return -1;}
    if (where && st.st_nlink!=1) {close(fd);errno=EACCES;return -1;}
    if ((flags&O_TRUNC) && ftruncate(fd,0)) {int e=errno;close(fd);errno=e;return -1;}
    return fd;
}
static int storage_fd(struct goodix_storage *s,int handle)
{
    if (handle<100 || handle>=108 || s->files[handle-100]<0) {errno=EBADF;return -1;}
    return s->files[handle-100];
}
static void storage_stat(tz_stat_t *o,const struct stat *st)
{
    memset(o,0,sizeof(*o));o->st_mode=st->st_mode;o->st_nlink=1;
    o->st_size=st->st_size;o->st_blksize=st->st_blksize;o->st_blocks=st->st_blocks;
}
/* Private-store removal and rename; calibration is never changed. */
static int storage_unlink(struct goodix_storage *s,const char *path)
{
    char name[80];int where=storage_name(path,name);
    if (where<0) return -1;
    if (!where) {errno=EROFS;return -1;}
    if (unlinkat(s->store,name,0)) return -1;
    return fsync(s->store);
}
static int storage_rename(struct goodix_storage *s,const char *from,const char *to)
{
    char a[80],b[80];int wa=storage_name(from,a),wb=storage_name(to,b);
    if (wa<0 || wb<0) return -1;
    if (!wa || !wb) {errno=EROFS;return -1;}
    if (renameat(s->store,a,s->store,b)) return -1;
    return fsync(s->store);
}
static int storage_fs(struct goodix_listener *l)
{
    struct goodix_storage *s=l->storage;
    unsigned char req[FILE_SERVICE_BUF_LEN];
    void *mem=qcomtee_memory_object_addr(l->memory);
    memcpy(req,mem,sizeof(req));
    uint32_t cmd;memcpy(&cmd,req,4);memset(mem,0,l->size);memcpy(mem,&cmd,4);
    int ret=-1;size_t result_offset=4;errno=EACCES;
    switch (cmd) {
    case TZ_FS_MSG_CMD_FILE_OPEN: {
        const tz_file_open_req_t *r=(void *)req;
        storage_log(l->id,cmd,r->pathname);
        if (r->flags&~(O_ACCMODE|O_CREAT|O_TRUNC|O_APPEND|O_EXCL) || (r->flags&O_ACCMODE)==O_ACCMODE) {errno=EINVAL;break;}
        int slot=0;while (slot<8 && s->files[slot]>=0) slot++;
        if (slot==8) {errno=EMFILE;break;}
        int fd=storage_openat(s,r->pathname,r->flags);
        if (fd>=0) {s->files[slot]=fd;s->writable[slot]=(r->flags&O_ACCMODE)!=O_RDONLY;ret=slot+100;}
        break;
    }
    case TZ_FS_MSG_CMD_FILE_CLOSE: {
        const tz_file_close_req_t *r=(void *)req;int fd=storage_fd(s,r->fd);
        if (fd<0) break;
        ret=s->writable[r->fd-100]?fsync(fd):0;int e=errno;
        if (close(fd) && !ret) {ret=-1;e=errno;}
        s->files[r->fd-100]=-1;errno=e;break;
    }
    case TZ_FS_MSG_CMD_FILE_LSEEK: {
        const tz_file_lseek_req_t *r=(void *)req;int fd=storage_fd(s,r->fildes);
        if (fd<0) break;
        if (r->whence!=SEEK_SET && r->whence!=SEEK_CUR && r->whence!=SEEK_END) {errno=EINVAL;break;}
        off_t pos=lseek(fd,r->offset,r->whence);
        if (pos<0) break;
        if (pos>GOODIX_STORE_LIMIT) {errno=EINVAL;break;}
        ret=(int)pos;break;
    }
    case TZ_FS_MSG_CMD_FILE_READ: {
        const tz_file_read_req_t *r=(void *)req;tz_file_read_rsp_t *o=mem;
        result_offset=offsetof(tz_file_read_rsp_t,ret);int fd=storage_fd(s,r->fd);
        if (fd<0) break;
        if (r->count>sizeof(o->buf)) {errno=EINVAL;break;}
        do {ret=read(fd,o->buf,r->count);} while (ret<0 && errno==EINTR);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_WRITE: {
        const tz_file_write_req_t *r=(void *)req;int fd=storage_fd(s,r->fd);
        if (fd<0) break;
        if (!s->writable[r->fd-100]) {errno=EBADF;break;}
        off_t pos=lseek(fd,0,SEEK_CUR);
        if (pos<0) break;
        if (r->count>sizeof(r->buf) || pos+(off_t)r->count>GOODIX_STORE_LIMIT) {errno=EFBIG;break;}
        do {ret=write(fd,r->buf,r->count);} while (ret<0 && errno==EINTR);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_SYNC: {
        const tz_file_sync_req_t *r=(void *)req;int fd=storage_fd(s,r->fd);
        if (fd>=0) ret=fsync(fd);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_FSTAT: {
        const tz_file_fstat_req_t *r=(void *)req;tz_file_fstat_rsp_t *o=mem;
        result_offset=offsetof(tz_file_fstat_rsp_t,ret);int fd=storage_fd(s,r->filedes);struct stat st;
        if (fd>=0 && !(ret=storage_check(fd,&st))) storage_stat(&o->buf,&st);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_LSTAT: {
        const tz_file_lstat_req_t *r=(void *)req;tz_file_lstat_rsp_t *o=mem;
        result_offset=offsetof(tz_file_lstat_rsp_t,ret);storage_log(l->id,cmd,r->path);
        int fd=storage_openat(s,r->path,O_RDONLY);struct stat st;
        if (fd>=0) {ret=storage_check(fd,&st);if (!ret) storage_stat(&o->buf,&st);int e=errno;close(fd);errno=e;}
        break;
    }
    case TZ_FS_MSG_CMD_FILE_REMOVE:
    case TZ_FS_MSG_CMD_FILE_UNLINK: {
        const tz_file_unlink_req_t *r=(void *)req;
        storage_log(l->id,cmd,r->pathname);ret=storage_unlink(s,r->pathname);break;
    }
    case TZ_FS_MSG_CMD_FILE_RENAME: {
        const tz_file_rename_req_t *r=(void *)req;
        storage_log(l->id,cmd,r->oldfilename);ret=storage_rename(s,r->oldfilename,r->newfilename);break;
    }
    case TZ_FS_MSG_CMD_FILE_MKDIR: {
        /* Only the template root itself exists; it is the private store. */
        const tz_file_mkdir_req_t *r=(void *)req;storage_log(l->id,cmd,r->pathname);
        size_t n=strlen(GOODIX_TEMPLATE_ROOT);
        if (!strncmp(r->pathname,GOODIX_TEMPLATE_ROOT,n-1) && (!r->pathname[n-1] || (r->pathname[n-1]=='/' && !r->pathname[n]))) ret=0;
        else errno=EACCES;
        break;
    }
    case TZ_FS_MSG_CMD_FILE_GET_ERRNO:ret=s->last_errno;break;
    default:fprintf(stderr,"storage: unsupported FS command=%#x\n",cmd);errno=ENOSYS;break;
    }
    if (ret<0) s->last_errno=errno;
    memcpy((char *)mem+result_offset,&ret,4);
    if (ret<0) fprintf(stderr,"storage FS command=%#x return=%d errno=%d\n",cmd,ret,s->last_errno);
    explicit_bzero(req,sizeof(req));return 0;
}
/* GPFS backup requests preserve the previous private blob before writing.
 * Calibration paths and unbounded backup chains are never writable. */
static int storage_backup(struct goodix_storage *s,int source,const char *path)
{
    char name[80];
    if (storage_name(path,name)!=1) {errno=EROFS;return -1;}
    if (strstr(name,".bak.bak")) {errno=EINVAL;return -1;}
    char backup[TZ_CM_MAX_NAME_LEN];
    int length=snprintf(backup,sizeof(backup),"%s.bak",path);
    if (length<0 || length>=(int)sizeof(backup)) {errno=ENAMETOOLONG;return -1;}
    int target=storage_openat(s,backup,O_RDWR|O_CREAT);
    if (target<0) return -1;
    uint8_t buffer[4096];off_t offset=0;int ret=0;
    for (;;) {
        ssize_t count=pread(source,buffer,sizeof(buffer),offset);
        if (count<0 && errno==EINTR) continue;
        if (count<0) {ret=-1;break;}
        if (!count) break;
        ssize_t written=0;
        while (written<count) {
            ssize_t n=pwrite(target,buffer+written,count-written,offset+written);
            if (n<0 && errno==EINTR) continue;
            if (n<=0) {ret=-1;break;}
            written+=n;
        }
        if (ret) break;
        offset+=count;
    }
    explicit_bzero(buffer,sizeof(buffer));
    if (!ret && (ftruncate(target,offset) || fsync(target) || fsync(s->store))) ret=-1;
    int saved=errno;close(target);errno=saved;return ret;
}
/* GPFS: legacy operations 0..3 (read, write, remove, rename) and the newer
 * data/persist variants 4..11 map onto the same four by cmd % 4. */
static int storage_gp(struct goodix_listener *l)
{
    struct goodix_storage *s=l->storage;
    void *mem=qcomtee_memory_object_addr(l->memory);size_t size=l->size;
    uint32_t cmd;memcpy(&cmd,mem,4);
    if (cmd==TZ_GPFS_MSG_CMD_GPFS_VERSION) {
        memset(mem,0,size);tz_gpfile_version_rsp_t *o=mem;o->cmd_id=cmd;o->version=2;return 0;
    }
    if (cmd>11 || size<sizeof(tz_gpfile_write_req_t)) return 2;
    unsigned char *req=malloc(size);
    if (!req) return 2;
    memcpy(req,mem,size);memset(mem,0,size);memcpy(mem,&cmd,4);
    uint32_t op=cmd%4;int fd=-1,ret=-1;errno=0;
    storage_log(l->id,cmd,(char *)req+4);
    if (op==0 || op==1) {
        const tz_gpfile_read_req_t *r=(void *)req;
        size_t cap=op?sizeof(((tz_gpfile_write_req_t *)0)->buf):sizeof(((tz_gpfile_read_rsp_t *)0)->buf);
        if (r->offset<0 || r->count>cap || (uint64_t)r->offset+r->count>GOODIX_STORE_LIMIT) {errno=EINVAL;goto done;}
        fd=storage_openat(s,r->pathname,op?O_RDWR|O_CREAT:O_RDONLY);
        if (fd<0) goto done;
        uint32_t moved=0;
        if (op) {
            const tz_gpfile_write_req_t *w=(void *)req;
            if (w->backup && storage_backup(s,fd,r->pathname)) goto done;
            while (moved<r->count) {
                ssize_t n=pwrite(fd,w->buf+moved,r->count-moved,(off_t)r->offset+moved);
                if (n<0 && errno==EINTR) continue;
                if (n<=0) goto done;
                moved+=(uint32_t)n;
            }
            if (fsync(fd) || fsync(s->store)) goto done;
            ((tz_gpfile_write_rsp_t *)mem)->num_bytes_written=moved;
        } else {
            tz_gpfile_read_rsp_t *o=mem;
            while (moved<r->count) {
                ssize_t n=pread(fd,o->buf+moved,r->count-moved,(off_t)r->offset+moved);
                if (n<0 && errno==EINTR) continue;
                if (n<0) goto done;
                if (!n) break;
                moved+=(uint32_t)n;
            }
            o->num_bytes_read=moved;
        }
        ret=0;
    } else if (op==2) {
        ret=storage_unlink(s,(char *)req+4);
    } else {
        const tz_gpfile_rename_req_t *r=(void *)req;
        ret=storage_rename(s,r->from,r->to);
    }
 done:
    if (fd>=0) close(fd);
    int error=ret?(errno?errno:EIO):0;
    ((tz_gpfile_err_rsp_t *)mem)->err=error;
    if (error) fprintf(stderr,"storage GPFS command=%u error=%d\n",cmd,error);
    explicit_bzero(req,size);free(req);return 0;
}
static qcomtee_result_t storage_dispatch(struct qcomtee_object *object,qcomtee_op_t op,
                                         struct qcomtee_param *p,int n)
{
    if (op==1 && !n) return 0;
    if (op || n!=6 || p[0].attr!=QCOMTEE_UBUF_OUTPUT || p[1].attr!=QCOMTEE_UBUF_OUTPUT || p[1].ubuf.size<4) return 2;
    for (int i=2;i<6;i++) if (p[i].attr!=QCOMTEE_OBJREF_OUTPUT) return 2;
    struct goodix_listener *l=container_of(object,struct goodix_listener,callback);
    int ret=l->id==FILE_SERVICE_ID?storage_fs(l):storage_gp(l);
    /* Output buffers have a capacity but no address; keep the reply alive. */
    p[0].ubuf.addr=NULL;p[0].ubuf.size=0;
    l->is64=1;p[1].ubuf.addr=&l->is64;p[1].ubuf.size=4;
    for (int i=2;i<6;i++) p[i].object=NULL;
    return ret;
}
static int storage_supported(qcomtee_op_t op) {return op<=1 || op==QCOMTEE_OBJREF_OP_RELEASE;}
static void storage_release(struct qcomtee_object *o)
{
    struct goodix_listener *l=container_of(o,struct goodix_listener,callback);
    if (l->memory) {explicit_bzero(qcomtee_memory_object_addr(l->memory),l->size);qcomtee_memory_object_release(l->memory);}
    free(l);
}
static void storage_error(struct qcomtee_object *o,int e) {(void)o;if (e) fprintf(stderr,"storage callback error=%d\n",e);}
static struct qcomtee_object_ops storage_ops={.release=storage_release,.dispatch=storage_dispatch,
    .error=storage_error,.supported=storage_supported};
static void *storage_worker(void *root)
{
    for (;;) {pthread_testcancel();if (qcomtee_object_process_one(root)) break;}
    return NULL;
}
/* Call after the app is unloaded: releasing the registration unregisters
 * only our listeners. */
static void storage_stop(struct goodix_storage *s)
{
    if (!s->initialized) return;
    if (s->registration) {qcomtee_object_refs_dec(s->registration);s->registration=NULL;}
    if (s->started) {pthread_cancel(s->worker);pthread_join(s->worker,NULL);s->started=0;}
    for (int i=0;i<2;i++) if (s->listeners[i]) {qcomtee_object_refs_dec(&s->listeners[i]->callback);s->listeners[i]=NULL;}
    for (int i=0;i<8;i++) if (s->files[i]>=0) {close(s->files[i]);s->files[i]=-1;}
    if (s->calibration>=0) close(s->calibration);
    if (s->store>=0) close(s->store);
    s->calibration=s->store=-1;s->initialized=0;
}
static int storage_open_dir(const char *path,int create)
{
    if (create && mkdir(path,0700) && errno!=EEXIST) return -1;
    int fd=open(path,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);
    struct stat st;
    if (fd>=0 && create && (fstat(fd,&st) || st.st_uid!=0 || (st.st_mode&077))) {close(fd);errno=EACCES;return -1;}
    return fd;
}
static int storage_start(struct goodix_storage *s,struct qcomtee_object *root,struct qcomtee_object *env,
                         const char *calibration,const char *store)
{
    s->initialized=1;s->calibration=s->store=-1;
    for (int i=0;i<8;i++) s->files[i]=-1;
    s->calibration=storage_open_dir(calibration,0);
    if (s->calibration<0) {perror(calibration);goto fail;}
    s->store=storage_open_dir(store,1);
    if (s->store<0) {perror(store);goto fail;}
    struct stat calibration_stat,store_stat;
    if (fstat(s->calibration,&calibration_stat) || fstat(s->store,&store_stat) ||
        (calibration_stat.st_dev==store_stat.st_dev && calibration_stat.st_ino==store_stat.st_ino)) {
        errno=EINVAL;perror("calibration and template directories must differ");goto fail;
    }
    uint32_t id=87;struct qcomtee_param args[]={in(&id,4),newobj()};
    if (invoke(env,"open IRegisterListenerCBO",0,args,2)) goto fail;
    s->registration=args[1].object;
    int err=pthread_create(&s->worker,NULL,storage_worker,root);
    if (err) {errno=err;perror("storage thread");goto fail;}
    s->started=1;
    for (int i=0;i<2;i++) {
        /* Device deployment delegates GPFS to the shared credential storage
         * service so a Gatekeeper call can run during Goodix enrollment. */
        const char *external=getenv("GOODIX_EXTERNAL_GPFS");
        if(i==1 && external && !strcmp(external,"1")) continue;
        struct goodix_listener *l=calloc(1,sizeof(*l));
        if (!l) goto fail;
        l->storage=s;l->id=i?GPFILE_SERVICE_ID:FILE_SERVICE_ID;l->size=i?GPFILE_SERVICE_BUF_LEN:FILE_SERVICE_BUF_LEN;
        if (qcomtee_object_cb_init(&l->callback,&storage_ops,root)) {free(l);goto fail;}
        s->listeners[i]=l;
        if (qcomtee_memory_object_alloc(l->size,root,&l->memory)) goto fail;
        memset(qcomtee_memory_object_addr(l->memory),0,l->size);
        /* A successful invoke hands one reference to QTEE, even on rejection. */
        qcomtee_object_refs_inc(&l->callback);
        struct qcomtee_param reg[]={in(&l->id,4),obj(&l->callback),obj(l->memory)};
        qcomtee_result_t status=UINT32_MAX;
        int r=qcomtee_object_invoke(s->registration,0,reg,3,&status);
        fprintf(stderr,"register listener %#x: transport=%d service=%d\n",l->id,r,(int32_t)status);
        if (r) qcomtee_object_refs_dec(&l->callback);
        if (r || status) goto fail;
    }
    return 0;
 fail:
    storage_stop(s);return -1;
}
