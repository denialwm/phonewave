/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Native /dev/tee0 listener adapter. Four exact Goodix cache filenames are
 * redirected into a private Linux directory. No firmware path is opened.
 * Only bounded path metadata is logged, never contents.
 * Wire definitions: Qualcomm minkipc listenercbo and libfsservice headers.
 */
#include <pthread.h>
#include <stdarg.h>
#include <stddef.h>
/* Both SDK header families define these diagnostic macros. */
#undef MSGV
#undef MSGD
#include "fs_msg.h"
#include "gpfs_msg.h"

struct uff_storage;
struct uff_listener {
    struct qcomtee_object callback;
    struct qcomtee_object *memory;
    size_t size;
    uint32_t id,is64;
    struct uff_storage *storage;
};
struct uff_storage {
    struct qcomtee_object *registration;
    struct uff_listener *listeners[2];
    pthread_t worker;
    int started,initialized,directory,files[8],last_errno;
};
static int tee_call(int fd,unsigned long op,...)
{
    va_list ap;
    va_start(ap,op);void *arg=va_arg(ap,void *);va_end(ap);
    int oldtype;
    pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS,&oldtype);
    int ret=ioctl(fd,op,arg),saved=errno;
    pthread_setcanceltype(oldtype,NULL);
    errno=saved;
    return ret;
}
static void *storage_worker(void *root)
{
    for (;;) {
        pthread_testcancel();
        if (qcomtee_object_process_one(root)) break;
    }
    return NULL;
}
static void storage_log_path(uint32_t id,uint32_t cmd,const char *path)
{
    printf("Storage listener=%#x command=%#x path=\"",id,cmd);
    for (size_t i=0;i<256 && path[i];i++) {
        unsigned char c=path[i];
        if (c>=32 && c<127 && c!='"' && c!='\\') putchar(c);
        else printf("\\x%02x",c);
    }
    puts("\"");
}
#define UFF_STORAGE_LIMIT (4*1024*1024)
#include "uff-template-names.h"
static const char *storage_name(const char *path)
{
    /* Exact virtual names only: synthetic group 941002 and slots 0..4.
     * Android user groups, arbitrary basenames and host paths are denied.
     */
    static const struct {const char *virtual_path,*private_name;} names[]={
        {"/denial-fingerprint/uff/global_finger.db","uff-global_finger.db"},
        {"/denial-fingerprint/uff/global_finger.db.bak","uff-global_finger.db.bak"},
        {"/denial-fingerprint/uff/global_finger.db.bak.bak","uff-global_finger.db.bak.bak"},
        {"/mnt/vendor/persist/data/finger_941002_0.so","uff-finger_941002_0.so"},
        {"/mnt/vendor/persist/data/finger_941002_0_bak.so","uff-finger_941002_0_bak.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_0.so","uff-finger_extra_info_941002_0.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_0_bak.so","uff-finger_extra_info_941002_0_bak.so"},
        {"/mnt/vendor/persist/data/finger_941002_1.so","uff-finger_941002_1.so"},
        {"/mnt/vendor/persist/data/finger_941002_1_bak.so","uff-finger_941002_1_bak.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_1.so","uff-finger_extra_info_941002_1.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_1_bak.so","uff-finger_extra_info_941002_1_bak.so"},
        {"/mnt/vendor/persist/data/finger_941002_2.so","uff-finger_941002_2.so"},
        {"/mnt/vendor/persist/data/finger_941002_2_bak.so","uff-finger_941002_2_bak.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_2.so","uff-finger_extra_info_941002_2.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_2_bak.so","uff-finger_extra_info_941002_2_bak.so"},
        {"/mnt/vendor/persist/data/finger_941002_3.so","uff-finger_941002_3.so"},
        {"/mnt/vendor/persist/data/finger_941002_3_bak.so","uff-finger_941002_3_bak.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_3.so","uff-finger_extra_info_941002_3.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_3_bak.so","uff-finger_extra_info_941002_3_bak.so"},
        {"/mnt/vendor/persist/data/finger_941002_4.so","uff-finger_941002_4.so"},
        {"/mnt/vendor/persist/data/finger_941002_4_bak.so","uff-finger_941002_4_bak.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_4.so","uff-finger_extra_info_941002_4.so"},
        {"/mnt/vendor/persist/data/finger_extra_info_941002_4_bak.so","uff-finger_extra_info_941002_4_bak.so"},
        {"/mnt/vendor/persist/data/auth_token_941002.so","uff-auth_token_941002.so"},
        {"/mnt/vendor/persist/data/gf_dynamic_persist.so.normal","gf_dynamic_persist.so.normal"},
        {"/mnt/vendor/persist/data/gf_dynamic_persist.so_bak.normal","gf_dynamic_persist.so_bak.normal"},
        {"/mnt/vendor/persist/data/gf_cali_persist.so.normal","gf_cali_persist.so.normal"},
        {"/mnt/vendor/persist/data/gf_cali_persist.so_bak.normal","gf_cali_persist.so_bak.normal"},
    };
    if (memchr(path,0,256))
        for (size_t i=0;i<sizeof(names)/sizeof(names[0]);i++)
            if (!strcmp(path,names[i].virtual_path)) return names[i].private_name;
    static _Thread_local char native_name[80];
    if (!gp_name(path,native_name)) return native_name;
    errno=EACCES;return NULL;
}
static int storage_check_file(int fd,struct stat *st)
{
    if (fstat(fd,st)) return -1;
    if (!S_ISREG(st->st_mode) || st->st_uid!=0 || st->st_nlink!=1 ||
        (st->st_mode&077) || st->st_size<0 || st->st_size>UFF_STORAGE_LIMIT) {
        errno=EACCES;return -1;
    }
    return 0;
}
static int storage_fd(struct uff_storage *s,int handle)
{
    if (handle<100 || handle>=108 || s->files[handle-100]<0) {
        errno=EBADF;return -1;
    }
    return s->files[handle-100];
}
static int storage_open(struct uff_storage *s,const tz_file_open_req_t *r)
{
    const char *name=storage_name(r->pathname);
    if (name && !strncmp(name,"uff-template-",13) && r->flags!=O_RDONLY) {errno=EACCES;return -1;}
    if (!name) return -1;
    int flags=r->flags;
    printf("Private storage open %s flags=%#x\n",name,flags);
    if ((flags&~(O_ACCMODE|O_CREAT|O_TRUNC|O_APPEND|O_EXCL)) ||
        (flags&O_ACCMODE)==O_ACCMODE ||
        ((flags&(O_TRUNC|O_APPEND)) && (flags&O_ACCMODE)==O_RDONLY)) {
        errno=EINVAL;return -1;
    }
    int slot;
    for (slot=0;slot<8 && s->files[slot]>=0;slot++);
    if (slot==8) {errno=EMFILE;return -1;}
    /* Validate an existing inode before truncating it. Never follow links,
     * expose a real host fd, or let the firmware choose a host directory. */
    int fd=openat(s->directory,name,(flags&~O_TRUNC)|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK,0600);
    if (fd<0) return -1;
    struct stat st;
    if (storage_check_file(fd,&st) || ((flags&O_TRUNC) && ftruncate(fd,0))) {
        int saved=errno;close(fd);errno=saved;return -1;
    }
    s->files[slot]=fd;return slot+100;
}
static void storage_stat(tz_stat_t *out,const struct stat *st)
{
    memset(out,0,sizeof(*out));out->st_mode=st->st_mode;
    out->st_nlink=1;out->st_size=st->st_size;
    out->st_blksize=st->st_blksize;out->st_blocks=st->st_blocks;
}
static int storage_fs(struct uff_listener *l)
{
    struct uff_storage *s=l->storage;
    unsigned char request[FILE_SERVICE_BUF_LEN];
    void *memory=qcomtee_memory_object_addr(l->memory);
    memcpy(request,memory,sizeof(request));
    uint32_t command;memcpy(&command,request,4);
    memset(memory,0,l->size);memcpy(memory,&command,4);
    size_t result_offset=4;
    int ret=-1;
    switch (command) {
    case TZ_FS_MSG_CMD_FILE_OPEN: {
        const tz_file_open_req_t *r=(void *)request;
        storage_log_path(l->id,command,r->pathname);ret=storage_open(s,r);break;
    }
    case TZ_FS_MSG_CMD_FILE_LSTAT: {
        const tz_file_lstat_req_t *r=(void *)request;
        tz_file_lstat_rsp_t *out=memory;
        result_offset=offsetof(tz_file_lstat_rsp_t,ret);
        storage_log_path(l->id,command,r->path);
        const char *name=storage_name(r->path);
        if (!name) break;
        int fd=openat(s->directory,name,O_RDONLY|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
        if (fd<0) break;
        struct stat st;ret=storage_check_file(fd,&st);
        if (!ret) storage_stat(&out->buf,&st);
        int saved=errno;close(fd);errno=saved;break;
    }
    case TZ_FS_MSG_CMD_FILE_FSTAT: {
        const tz_file_fstat_req_t *r=(void *)request;
        tz_file_fstat_rsp_t *out=memory;
        result_offset=offsetof(tz_file_fstat_rsp_t,ret);
        int fd=storage_fd(s,r->filedes);struct stat st;
        if (fd<0) break;
        ret=storage_check_file(fd,&st);
        if (!ret) storage_stat(&out->buf,&st);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_READ: {
        const tz_file_read_req_t *r=(void *)request;
        tz_file_read_rsp_t *out=memory;
        result_offset=offsetof(tz_file_read_rsp_t,ret);
        int fd=storage_fd(s,r->fd);
        if (fd<0) break;
        if (r->count>sizeof(out->buf)) {errno=EINVAL;break;}
        do {ret=read(fd,out->buf,r->count);} while (ret<0 && errno==EINTR);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_WRITE: {
        const tz_file_write_req_t *r=(void *)request;
        int fd=storage_fd(s,r->fd);
        if (fd<0) break;
        off_t position=lseek(fd,0,SEEK_CUR);struct stat st;
        if (position<0 || storage_check_file(fd,&st)) break;
        if (fcntl(fd,F_GETFL)&O_APPEND) position=st.st_size;
        if (r->count>sizeof(r->buf) || position>UFF_STORAGE_LIMIT ||
            r->count>(uint64_t)(UFF_STORAGE_LIMIT-position)) {errno=EFBIG;break;}
        do {ret=write(fd,r->buf,r->count);} while (ret<0 && errno==EINTR);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_LSEEK: {
        const tz_file_lseek_req_t *r=(void *)request;
        int fd=storage_fd(s,r->fildes);
        if (fd<0) break;
        struct stat st;
        off_t base;
        if (r->whence==SEEK_SET) base=0;
        else if (r->whence==SEEK_CUR) base=lseek(fd,0,SEEK_CUR);
        else if (r->whence==SEEK_END && !storage_check_file(fd,&st)) base=st.st_size;
        else {errno=EINVAL;break;}
        int64_t position=(int64_t)base+r->offset;
        if (base<0 || position<0 || position>UFF_STORAGE_LIMIT) {errno=EINVAL;break;}
        ret=lseek(fd,position,SEEK_SET);break;
    }
    case TZ_FS_MSG_CMD_FILE_CLOSE: {
        const tz_file_close_req_t *r=(void *)request;
        int fd=storage_fd(s,r->fd);
        if (fd<0) break;
        ret=fsync(fd);int saved=errno;
        if (close(fd) && !ret) {ret=-1;saved=errno;}
        s->files[r->fd-100]=-1;
        if (!ret && fsync(s->directory)) {ret=-1;saved=errno;}
        errno=saved;break;
    }
    case TZ_FS_MSG_CMD_FILE_SYNC: {
        const tz_file_sync_req_t *r=(void *)request;
        int fd=storage_fd(s,r->fd);
        if (fd>=0) ret=fsync(fd);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_REMOVE:
    case TZ_FS_MSG_CMD_FILE_UNLINK: {
        const tz_file_unlink_req_t *r=(void *)request;
        const char *name=storage_name(r->pathname);
        if (!name) break;
        int fd=openat(s->directory,name,O_RDONLY|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
        struct stat st;
        if (fd<0) break;
        ret=storage_check_file(fd,&st);int saved=errno;close(fd);errno=saved;
        if (!ret) ret=unlinkat(s->directory,name,0);
        if (!ret) ret=fsync(s->directory);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_RENAME: {
        const tz_file_rename_req_t *r=(void *)request;
        char from[80],to[80];const char *mapped=storage_name(r->oldfilename);
        if (!mapped) break;
        snprintf(from,sizeof(from),"%s",mapped);mapped=storage_name(r->newfilename);
        if (!mapped) break;
        snprintf(to,sizeof(to),"%s",mapped);
        int fd=openat(s->directory,from,O_RDONLY|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
        struct stat st;
        if (fd<0) break;
        ret=storage_check_file(fd,&st);int saved=errno;close(fd);errno=saved;
        if (ret) break;
        fd=openat(s->directory,to,O_RDONLY|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
        if (fd>=0) {ret=storage_check_file(fd,&st);saved=errno;close(fd);errno=saved;}
        else if (errno!=ENOENT) ret=-1;
        if (!ret) ret=renameat(s->directory,from,s->directory,to);
        if (!ret) ret=fsync(s->directory);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_GET_ERRNO:ret=s->last_errno;break;
    default:
        fprintf(stderr,"Unsupported private FS command=%#x\n",command);
        explicit_bzero(request,sizeof(request));return 2;
    }
    if (ret<0) s->last_errno=errno;
    memcpy((char *)memory+result_offset,&ret,4);
    if (ret<0) fprintf(stderr,"Private FS command=%#x result=%d errno=%d\n",command,ret,s->last_errno);
    explicit_bzero(request,sizeof(request));return 0;
}
#include "uff-gp-storage.h"
static int storage_deny(struct uff_listener *l)
{
    if (l->id==GPFILE_SERVICE_ID)
        return storage_gp(l->storage,qcomtee_memory_object_addr(l->memory),l->size);
    return storage_fs(l);
}
static qcomtee_result_t storage_dispatch(struct qcomtee_object *object,
                                        qcomtee_op_t op,struct qcomtee_param *p,int n)
{
    if (op==1 && !n) return 0;
    if (op || n!=6 || p[0].attr!=QCOMTEE_UBUF_OUTPUT ||
        p[1].attr!=QCOMTEE_UBUF_OUTPUT || p[1].ubuf.size<4) return 2;
    for (int i=2;i<6;i++) if (p[i].attr!=QCOMTEE_OBJREF_OUTPUT) return 2;
    struct uff_listener *l=container_of(object,struct uff_listener,callback);
    int result=storage_deny(l);
    /* Native callback output buffers have a capacity, but no allocated
     * address. Keep response storage alive through SUPPL_SEND. */
    p[0].ubuf.addr=NULL;p[0].ubuf.size=0;
    l->is64=1;p[1].ubuf.addr=&l->is64;p[1].ubuf.size=4;
    for (int i=2;i<6;i++) p[i].object=NULL;
    return result;
}
static int storage_supported(qcomtee_op_t op) {return op<=1 || op==QCOMTEE_OBJREF_OP_RELEASE;}
static void storage_release(struct qcomtee_object *o)
{
    struct uff_listener *l=container_of(o,struct uff_listener,callback);
    if (l->memory) qcomtee_memory_object_release(l->memory);
    free(l);
}
static void storage_error(struct qcomtee_object *o,int error)
{
    (void)o;
    if (error) fprintf(stderr,"Storage callback transport error=%d\n",error);
}
static struct qcomtee_object_ops storage_ops={.release=storage_release,
    .dispatch=storage_dispatch,.error=storage_error,.supported=storage_supported};
static void storage_stop(struct uff_storage *s)
{
    /* Releasing this object unregisters only our listeners, while callbacks
     * are still serviced. The app must already have been unloaded.
     */
    if (!s->initialized) return;
    if (s->registration) {qcomtee_object_refs_dec(s->registration);s->registration=NULL;}
    if (s->started) {pthread_cancel(s->worker);pthread_join(s->worker,NULL);s->started=0;}
    for (size_t i=0;i<2;i++) if (s->listeners[i]) {
        qcomtee_object_refs_dec(&s->listeners[i]->callback);s->listeners[i]=NULL;
    }
    for (size_t i=0;i<8;i++) if (s->files[i]>=0) {close(s->files[i]);s->files[i]=-1;}
    if (s->directory>=0) {close(s->directory);s->directory=-1;}
    s->initialized=0;
}
static int storage_start(struct uff_storage *s,struct qcomtee_object *root,
                         struct qcomtee_object *env)
{
    s->initialized=1;s->directory=-1;
    for (size_t i=0;i<8;i++) s->files[i]=-1;
    const char *path="/var/lib/aston-fingerprint/secure-cache";
    if (mkdir(path,0700) && errno!=EEXIST) goto fail;
    s->directory=open(path,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);
    struct stat st;
    if (s->directory<0 || fstat(s->directory,&st) || st.st_uid!=0 || (st.st_mode&077)) goto fail;
    uint32_t uid=87;
    struct qcomtee_param args[]={input(&uid,4),object_out()};
    if (invoke(env,"open IRegisterListenerCBO",0,args,2)) goto fail;
    s->registration=args[1].object;
    int error=pthread_create(&s->worker,NULL,storage_worker,root);
    if (error) {errno=error;perror("storage thread");goto fail;}
    s->started=1;
    for (size_t i=0;i<2;i++) {
        struct uff_listener *l=calloc(1,sizeof(*l));
        if (!l) goto fail;
        l->storage=s;
        l->id=i?GPFILE_SERVICE_ID:FILE_SERVICE_ID;
        l->size=i?GPFILE_SERVICE_BUF_LEN:FILE_SERVICE_BUF_LEN;
        if (qcomtee_object_cb_init(&l->callback,&storage_ops,root)) {free(l);goto fail;}
        s->listeners[i]=l;
        if (qcomtee_memory_object_alloc(l->size,root,&l->memory)) goto fail;
        memset(qcomtee_memory_object_addr(l->memory),0,l->size);
        /* Keep one local reference; a successful invoke transfers the other
         * reference to QTEE, including when the service rejects registration.
         */
        qcomtee_object_refs_inc(&l->callback);
        struct qcomtee_param reg[]={input(&l->id,4),object_in(&l->callback),object_in(l->memory)};
        qcomtee_result_t result=-1;
        int transport=qcomtee_object_invoke(s->registration,0,reg,3,&result);
        fprintf(stderr,"register storage %#x: transport=%d service=%d\n",l->id,transport,result);
        if (transport) qcomtee_object_refs_dec(&l->callback);
        if (transport || result) goto fail;
    }
    return 0;
 fail:
    storage_stop(s);return -1;
}
