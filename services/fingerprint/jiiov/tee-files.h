/* SPDX-License-Identifier: GPL-2.0-only */
/* Optional, read-only calibration listener for the bring-up probe.
 * Wire layouts: Qualcomm minkipc bb99356e15c34434d5bc521f1f68bb2f3273793b,
 * listeners/listenercbo/idl and listeners/libfsservice/fs/fs_msg.h.
 * Calibration copies remain read-only. Explicit storage sessions can also
 * access bounded template chunks in a separate private directory.
 */
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include "fs_msg.h"
#include "gpfs_msg.h"

static struct {
    int server, registration, dma, gp_dma, directory, storage_directory;
    int files[8], last_errno;
    void *mapping, *gp_mapping;
    pthread_t worker;
    int started;
    atomic_int stopping;
} fs = {.server=-1,.registration=-1,.dma=-1,.gp_dma=-1,.directory=-1,.storage_directory=-1,
        .mapping=MAP_FAILED,.gp_mapping=MAP_FAILED};

static void fs_log_path(const char *operation, const char *path, size_t capacity)
{
    printf("Firmware FS %s path=\"",operation);
    for (size_t i=0;i<capacity && path[i];i++) {
        unsigned char c=path[i];
        if (c>=32 && c<127 && c!='"' && c!='\\') putchar(c);
        else printf("\\x%02x",c);
    }
    puts("\"");
}

/* AncWriteClusterFileEncrypt splits at 1 MiB and prepends 52 bytes.
 * Only this diagnostic's database, never Android's existing enrollment data.
 * Five primary/backup chunks for the index and each of five feature slots.
 */
#define FS_TEMPLATE_CHUNK_SIZE (1024u*1024u+52u)
/* This firmware uses the entire registered 504 KiB callback buffer, including
 * 515824-byte writes observed live, beyond the public header's 500 KiB array.
 * Derive payload bounds from the actual mapped message, not that array.
 */
#define FS_GP_WRITE_CAPACITY (GPFILE_SERVICE_BUF_LEN-0x110u)
#define FS_GP_READ_CAPACITY (GPFILE_SERVICE_BUF_LEN-0x0cu)
_Static_assert(offsetof(tz_gpfile_write_req_t,buf)==0x110, "GP write payload ABI");
_Static_assert(offsetof(tz_gpfile_read_rsp_t,buf)==0x0c, "GP read payload ABI");
static const char *fs_template_name(const char *path, size_t capacity)
{
    static const char prefix[]="/droidloom-fingerprint-probe/";
    static const char database[]="template_941001";
    if (!memchr(path,0,capacity) || strncmp(path,prefix,sizeof(prefix)-1)) return NULL;
    const char *name=path+sizeof(prefix)-1, *suffix=name;
    if (strncmp(suffix,database,sizeof(database)-1)) return NULL;
    suffix+=sizeof(database)-1;
    if (!strncmp(suffix,".db",3)) suffix+=3;
    else if (*suffix=='_' && suffix[1]>='0' && suffix[1]<='4') suffix+=2;
    else return NULL;
    if (!strncmp(suffix,".bak",4)) suffix+=4;
    if (*suffix++!='.' || !*suffix) return NULL;
    if (suffix[0]=='0' && suffix[1]) return NULL;
    unsigned chunk=0;
    for (; *suffix; suffix++) {
        if (*suffix<'0' || *suffix>'9') return NULL;
        chunk=chunk*10+(*suffix-'0');
        if (chunk>=5) return NULL;
    }
    return name;
}

static int fs_storage_open(const char *name, int flags)
{
    if (fs.storage_directory<0 || !name) { errno=EACCES; return -1; }
    int fd=openat(fs.storage_directory,name,flags|O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK,0600);
    if (fd<0) return -1;
    struct stat st;
    if (fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_nlink!=1 ||
        st.st_uid!=geteuid() || (st.st_mode&077) || st.st_size>FS_TEMPLATE_CHUNK_SIZE) {
        close(fd); errno=EACCES; return -1;
    }
    return fd;
}

static int fs_fd(int handle)
{
    if (handle < 100 || handle > 107 || fs.files[handle-100] < 0) {
        errno = EBADF;
        return -1;
    }
    return fs.files[handle-100];
}
static int fs_open(const tz_file_open_req_t *request)
{
    static const char prefix[] = "/mnt/vendor/persist/jiiov/";
    /* Record bounded, escaped path metadata to identify missing services.
     * Never treat the firmware's pathname as a format string or log contents.
     */
    fputs("Firmware FS open path=\"",stdout);
    for (size_t i=0; i<sizeof(request->pathname) && request->pathname[i]; i++) {
        unsigned char c=request->pathname[i];
        if (c>=32 && c<127 && c!='"' && c!='\\') putchar(c);
        else printf("\\x%02x",c);
    }
    printf("\" flags=0x%x\n",request->flags);
    const char *template_name=fs_template_name(request->pathname,sizeof(request->pathname));
    if (template_name && fs.storage_directory>=0) {
        if (request->flags!=O_RDONLY) { errno=EROFS; return -1; }
        for (int i=0;i<8;i++) {
            if (fs.files[i]>=0) continue;
            int fd=fs_storage_open(template_name,O_RDONLY);
            if (fd<0) return -1;
            fs.files[i]=fd;
            return i+100;
        }
        errno=EMFILE; return -1;
    }
    if (!memchr(request->pathname,0,sizeof(request->pathname)) ||
        strncmp(request->pathname,prefix,sizeof(prefix)-1)) {
        errno = EACCES; return -1;
    }
    const char *name = request->pathname+sizeof(prefix)-1;
    if (strcmp(name,"jv_opt_data.bin") && strcmp(name,"jv_normalized_calibration.bin") &&
        strcmp(name,"jv_normalized_calibration.bin.bak")) {
        errno = EACCES; return -1;
    }
    printf("Calibration open %s flags=0x%x\n",name,request->flags);
    if (request->flags != O_RDONLY) { errno = EROFS; return -1; }
    for (int i=0; i<8; i++) {
        if (fs.files[i] >= 0) continue;
        int fd = openat(fs.directory,name,O_RDONLY|O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK);
        if (fd < 0) return -1;
        struct stat st;
        if (fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_size > 1024*1024) {
            close(fd); errno=EINVAL; return -1;
        }
        fs.files[i]=fd;
        return i+100;
    }
    errno=EMFILE; return -1;
}
static int fs_dispatch(void *memory)
{
    unsigned char request[FILE_SERVICE_BUF_LEN];
    memcpy(request,memory,sizeof(request));
    uint32_t command;
    memcpy(&command,request,4);
    memset(memory,0,FILE_SERVICE_BUF_LEN);
    memcpy(memory,&command,4);
    int ret = -1;
    size_t result_offset=4;
    switch (command) {
    case TZ_FS_MSG_CMD_FILE_OPEN:
        ret=fs_open((void *)request);
        break;
    case TZ_FS_MSG_CMD_FILE_READ: {
        const tz_file_read_req_t *r=(void *)request;
        tz_file_read_rsp_t *out=memory;
        int fd=fs_fd(r->fd);
        result_offset=offsetof(tz_file_read_rsp_t,ret);
        if (fd < 0) break;
        if (r->count > sizeof(out->buf)) { errno=EINVAL; break; }
        ret=read(fd,out->buf,r->count);
        break;
    }
    case TZ_FS_MSG_CMD_FILE_FSTAT: {
        const tz_file_fstat_req_t *r=(void *)request;
        tz_file_fstat_rsp_t *out=memory;
        result_offset=offsetof(tz_file_fstat_rsp_t,ret);
        int fd=fs_fd(r->filedes);
        struct stat st;
        if (fd < 0) break;
        ret=fstat(fd,&st);
        if (!ret) {
            out->buf.st_mode=st.st_mode;
            out->buf.st_nlink=1;
            out->buf.st_size=st.st_size;
            out->buf.st_blksize=st.st_blksize;
            out->buf.st_blocks=st.st_blocks;
            printf("Calibration fstat size=%lld\n",(long long)st.st_size);
        }
        break;
    }
    case TZ_FS_MSG_CMD_FILE_CLOSE: {
        const tz_file_close_req_t *r=(void *)request;
        int fd=fs_fd(r->fd);
        if (fd < 0) break;
        ret=close(fd);
        fs.files[r->fd-100]=-1;
        break;
    }
    case TZ_FS_MSG_CMD_FILE_GET_ERRNO:
        ret=fs.last_errno;
        break;
    default:
        fprintf(stderr,"Unsupported calibration FS command 0x%x\n",command);
        return 2; /* Object_ERROR_INVALID: no invented response layout. */
    }
    if (ret < 0) fs.last_errno=errno;
    memcpy((char *)memory+result_offset,&ret,4);
    printf("Calibration FS command=0x%x result=%d errno=%d\n",command,ret,
           ret < 0 ? fs.last_errno : 0);
    return 0;
}
static void fs_wake(int signal_number) { (void)signal_number; }
static int gp_storage_dispatch(void *memory, uint32_t command)
{
    /* Stock libdrmfs uses legacy absolute-path commands 1/2/3 for
     * write/remove/rename (0xef14, 0xefa4 and the GP dispatcher table).
     */
    if (fs.storage_directory<0) return 2;
    int error=EACCES;
    uint32_t written=0;
    if (command==1) {
        const tz_gpfile_write_req_t *r=memory;
        fs_log_path("bulk-write",r->pathname,sizeof(r->pathname));
        printf("Bulk write offset=%d count=%u backup=%u\n",r->offset,r->count,r->backup);
        const char *name=fs_template_name(r->pathname,sizeof(r->pathname));
        if (name && r->offset>=0 && r->count<=FS_GP_WRITE_CAPACITY &&
            (uint32_t)r->offset<=FS_TEMPLATE_CHUNK_SIZE-r->count && !r->backup) {
            int fd=fs_storage_open(name,O_WRONLY|O_CREAT);
            if (fd>=0) {
                error=0;
                while (written<r->count) {
                    const unsigned char *payload=(const unsigned char *)memory+0x110;
                    ssize_t n=pwrite(fd,payload+written,r->count-written,r->offset+written);
                    if (n<0 && errno==EINTR) continue;
                    if (n<=0) { error=n<0 ? errno : EIO; break; }
                    written+=n;
                }
                if (!error && fsync(fd)) error=errno;
                close(fd);
                if (!error && fsync(fs.storage_directory)) error=errno;
            } else error=errno;
        } else if (name) error=EINVAL;
    } else if (command==2) {
        const tz_gpfile_remove_req_t *r=memory;
        fs_log_path("bulk-remove",r->pathname,sizeof(r->pathname));
        const char *name=fs_template_name(r->pathname,sizeof(r->pathname));
        if (name) {
            error=unlinkat(fs.storage_directory,name,0) ? errno : 0;
            if (!error && fsync(fs.storage_directory)) error=errno;
        }
    } else if (command==3) {
        const tz_gpfile_rename_req_t *r=memory;
        fs_log_path("bulk-rename-from",r->from,sizeof(r->from));
        fs_log_path("bulk-rename-to",r->to,sizeof(r->to));
        const char *from=fs_template_name(r->from,sizeof(r->from));
        const char *to=fs_template_name(r->to,sizeof(r->to));
        if (from && to) {
            error=renameat(fs.storage_directory,from,fs.storage_directory,to) ? errno : 0;
            if (!error && fsync(fs.storage_directory)) error=errno;
        }
    } else return 2;
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);
    anc_put_le32(memory,command);
    anc_put_le32((char *)memory+4,error);
    if (command==1) anc_put_le32((char *)memory+8,written);
    printf("Template GP command=%u bytes=%u error=%d\n",command,written,error);
    return 0;
}
static int gp_dispatch(void *memory)
{
    tz_gpfile_read_req_t req;
    memcpy(&req,memory,sizeof(req));
    if (req.cmd_id>=1 && req.cmd_id<=3) return gp_storage_dispatch(memory,req.cmd_id);
    memset(memory,0,GPFILE_SERVICE_BUF_LEN);
    tz_gpfile_read_rsp_t *out=memory;
    out->cmd_id=req.cmd_id;
    if (req.cmd_id==TZ_GPFS_MSG_CMD_GPFS_VERSION) {
        tz_gpfile_version_rsp_t *version=memory;
        version->version=2; version->err=0;
        return 0;
    }
    /* Stock libdrmfs smci_gpdispatch: legacy absolute-path read is command 0
     * (0xee28 -> 0xf09c -> 0xf0e0), with the same read request/response layout.
     */
    if (req.cmd_id!=0 && req.cmd_id!=TZ_GPFS_MSG_CMD_PERSIST_FILE_READ &&
        req.cmd_id!=TZ_GPFS_MSG_CMD_DATA_FILE_READ) {
        fprintf(stderr,"Unsupported GP calibration command=0x%x\n",req.cmd_id);
        return 2;
    }
    if (req.count>FS_GP_READ_CAPACITY || req.offset<0 ||
        !memchr(req.pathname,0,sizeof(req.pathname))) {
        out->err=EINVAL; return 0;
    }
    tz_file_open_req_t open_req={.flags=O_RDONLY};
    /* Qualcomm's bulk interface may strip the persist mount prefix. */
    if (!strncmp(req.pathname,"jiiov/",6)) {
        int n=snprintf(open_req.pathname,sizeof(open_req.pathname),
                       "/mnt/vendor/persist/%s",req.pathname);
        if (n<0 || n>=(int)sizeof(open_req.pathname)) { out->err=EINVAL; return 0; }
    } else memcpy(open_req.pathname,req.pathname,sizeof(req.pathname));
    int handle=fs_open(&open_req);
    if (handle<0) { out->err=errno; return 0; }
    int fd=fs_fd(handle);
    ssize_t n=pread(fd,(unsigned char *)memory+0x0c,req.count,req.offset);
    out->err=n<0 ? errno : 0;
    out->num_bytes_read=n<0 ? 0 : n;
    close(fd); fs.files[handle-100]=-1;
    printf("GP calibration command=0x%x bytes=%u error=%d\n",
           req.cmd_id,out->num_bytes_read,out->err);
    return 0;
}
static void *fs_worker(void *unused)
{
    (void)unused;
    union { union smcinvoke_arg align; unsigned char bytes[4096]; } storage={0};
    struct smcinvoke_accept accept={.argsize=sizeof(union smcinvoke_arg),
        .buf_len=sizeof(storage),.buf_addr=(uintptr_t)&storage};
    while (!atomic_load(&fs.stopping)) {
        int ret=ioctl(fs.server,SMCINVOKE_IOCTL_ACCEPT_REQ,&accept);
        if (ret < 0) {
            if (errno==EINTR) continue;
            perror("accept calibration callback"); break;
        }
        printf("Calibration callback op=%u counts=0x%x\n",accept.op,accept.counts);
        union smcinvoke_arg *a=(void *)&storage;
        accept.result=2;
        if (accept.op==0 && accept.counts==counts(0,2,0,4)) {
            uintptr_t base=(uintptr_t)&storage;
            int valid=1;
            for (int i=0; i<2; i++) {
                if (a[i].b.addr < base+6*sizeof(*a) ||
                    a[i].b.addr > base+sizeof(storage) ||
                    a[i].b.size > base+sizeof(storage)-a[i].b.addr) valid=0;
            }
            if (valid && a[1].b.size==4) {
                int gp=(int32_t)accept.cbobj_id==-3;
                int dma=gp ? fs.gp_dma : fs.dma;
                void *mapping=gp ? fs.gp_mapping : fs.mapping;
                struct dma_buf_sync sync={.flags=DMA_BUF_SYNC_START|DMA_BUF_SYNC_RW};
                if ((int32_t)accept.cbobj_id!=-2 && !gp) accept.result=2;
                else if (!ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) {
                    accept.result=gp ? gp_dispatch(mapping) : fs_dispatch(mapping);
                    sync.flags=DMA_BUF_SYNC_END|DMA_BUF_SYNC_RW;
                    if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) accept.result=1;
                } else accept.result=1;
                a[0].b.size=0;
                uint32_t is64=1;
                memcpy((void *)(uintptr_t)a[1].b.addr,&is64,4);
                for (int i=2; i<6; i++) a[i]=null_object();
            }
        } else if (!accept.counts && (accept.op==1 || accept.op==0xffff || accept.op==0xfffe)) {
            accept.result=0;
        }
        accept.has_resp=1;
    }
    return NULL;
}
static void fs_stop(void)
{
    if (fs.registration>=0) { close(fs.registration); fs.registration=-1; }
    if (fs.started) {
        atomic_store(&fs.stopping,1);
        /* A signal can arrive just before ACCEPT starts. Timed join and a
         * repeated wake avoid losing that wakeup and hanging shutdown.
         */
        int joined;
        do {
            struct timespec deadline;
            clock_gettime(CLOCK_REALTIME,&deadline);
            deadline.tv_sec++;
            pthread_kill(fs.worker,SIGUSR1);
            joined=pthread_timedjoin_np(fs.worker,NULL,&deadline);
        } while (joined==ETIMEDOUT);
        fs.started=0;
    }
    if (fs.server>=0) { close(fs.server); fs.server=-1; }
    if (fs.mapping!=MAP_FAILED) { munmap(fs.mapping,FILE_SERVICE_BUF_LEN); fs.mapping=MAP_FAILED; }
    if (fs.gp_mapping!=MAP_FAILED) { munmap(fs.gp_mapping,GPFILE_SERVICE_BUF_LEN); fs.gp_mapping=MAP_FAILED; }
    if (fs.dma>=0) { close(fs.dma); fs.dma=-1; }
    if (fs.gp_dma>=0) { close(fs.gp_dma); fs.gp_dma=-1; }
    if (fs.directory>=0) { close(fs.directory); fs.directory=-1; }
    if (fs.storage_directory>=0) { close(fs.storage_directory); fs.storage_directory=-1; }
    for (int i=0; i<8; i++) if (fs.files[i]>=0) { close(fs.files[i]); fs.files[i]=-1; }
}
static int fs_start(int env, const char *directory, const char *storage_directory)
{
    for (int i=0; i<8; i++) fs.files[i]=-1;
    if (storage_directory) {
        fs.storage_directory=open(storage_directory,O_DIRECTORY|O_NOFOLLOW|O_RDONLY|O_CLOEXEC);
        struct stat st;
        if (fs.storage_directory<0 || fstat(fs.storage_directory,&st) ||
            st.st_uid!=geteuid() || (st.st_mode&077)) goto fail;
    }
    fs.directory=open(directory,O_DIRECTORY|O_NOFOLLOW|O_RDONLY|O_CLOEXEC);
    if (fs.directory<0) { perror(directory); return -1; }
    uint32_t uid=87;
    union smcinvoke_arg open_args[]={buffer(&uid,4),null_object()};
    if (invoke(env,"open listener registration",0,counts(1,0,0,1),open_args)) goto fail;
    fs.registration=open_args[1].o.fd;
    if (fs.registration<0) goto fail;
    struct smcinvoke_server server={.cb_buf_size=4096};
    fs.server=ioctl(env,SMCINVOKE_IOCTL_SERVER_REQ,&server);
    if (fs.server<0) { perror("create callback server"); goto fail; }
    int heap=open("/dev/dma_heap/qcom,qseecom",O_RDWR|O_CLOEXEC);
    if (heap<0) { perror("listener DMA heap"); goto fail; }
    struct dma_heap_allocation_data alloc={.len=FILE_SERVICE_BUF_LEN,.fd_flags=O_RDWR|O_CLOEXEC};
    int ret=ioctl(heap,DMA_HEAP_IOCTL_ALLOC,&alloc);
    if (ret) { close(heap); perror("allocate listener buffer"); goto fail; }
    fs.dma=alloc.fd;
    fs.mapping=mmap(NULL,alloc.len,PROT_READ|PROT_WRITE,MAP_SHARED,fs.dma,0);
    if (fs.mapping==MAP_FAILED) { close(heap); perror("map listener buffer"); goto fail; }
    alloc.len=GPFILE_SERVICE_BUF_LEN;
    alloc.fd=0;
    ret=ioctl(heap,DMA_HEAP_IOCTL_ALLOC,&alloc);
    close(heap);
    if (ret) { perror("allocate GP listener buffer"); goto fail; }
    fs.gp_dma=alloc.fd;
    fs.gp_mapping=mmap(NULL,alloc.len,PROT_READ|PROT_WRITE,MAP_SHARED,fs.gp_dma,0);
    if (fs.gp_mapping==MAP_FAILED) { perror("map GP listener buffer"); goto fail; }
    struct sigaction action={.sa_handler=fs_wake};
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR1,&action,NULL)) goto fail;
    sigset_t blocked, previous;
    sigemptyset(&blocked);
    sigaddset(&blocked,SIGINT); sigaddset(&blocked,SIGTERM);
    ret=pthread_sigmask(SIG_BLOCK,&blocked,&previous);
    if (ret) { errno=ret; perror("listener signal mask"); goto fail; }
    ret=pthread_create(&fs.worker,NULL,fs_worker,NULL);
    pthread_sigmask(SIG_SETMASK,&previous,NULL);
    if (ret) { errno=ret; perror("listener thread"); goto fail; }
    fs.started=1;
    uint32_t listener=FILE_SERVICE_ID;
    union smcinvoke_arg register_args[]={buffer(&listener,4),
        {.o={.fd=-2,.cb_server_fd=fs.server}}, {.o={.fd=fs.dma,.cb_server_fd=-1}}};
    if (invoke(fs.registration,"register bounded filesystem listener",0,
               counts(1,0,2,0),register_args)) goto fail;
    listener=GPFILE_SERVICE_ID;
    register_args[1].o.fd=-3;
    register_args[2].o.fd=fs.gp_dma;
    if (invoke(fs.registration,"register bounded bulk filesystem listener",0,
               counts(1,0,2,0),register_args)) goto fail;
    return 0;
fail:
    fs_stop();
    return -1;
}
