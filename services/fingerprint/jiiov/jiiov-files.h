/* SPDX-License-Identifier: GPL-2.0-only */
/* Host file services for ancapp64: the FS and GPFS listeners' requests.
 * Wire layouts: Qualcomm minkipc listeners/libfsservice fs_msg.h and
 * gpfs_msg.h. The factory calibration copy is served read-only; template
 * chunks live in a separate private directory, bounded in names and size.
 * No other path is opened, and only path metadata is logged.
 */
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
/* libqcomtee and minkipc both define these logging macros. */
#undef MSGV
#undef MSGD
#include "fs_msg.h"
#include "gpfs_msg.h"
#include "anc-protocol.h"

static struct {
    int directory, storage_directory, files[8], last_errno;
} fs = {.directory=-1,.storage_directory=-1,.files={-1,-1,-1,-1,-1,-1,-1,-1}};

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
/* The group folder the worker gives the app; its files carry this name. */
#define JIIOV_TEMPLATE_ROOT "/denial-fingerprint/jiiov"
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
    static const char prefix[]=JIIOV_TEMPLATE_ROOT "/";
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
