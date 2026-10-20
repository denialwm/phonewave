/* SPDX-License-Identifier: LGPL-2.1-or-later */
#define main uff_probe_main
#include "uff-probe.c"
#undef main
#include <assert.h>

int main(void)
{
    assert(geteuid()==0);
    char directory[]="/var/lib/aston-fingerprint/storage-test-XXXXXX";
    assert(mkdtemp(directory));
    struct uff_storage s={.directory=open(directory,O_RDONLY|O_DIRECTORY|O_CLOEXEC)};
    assert(s.directory>=0);
    for (size_t i=0;i<8;i++) s.files[i]=-1;
    const char *name="gf_cali_persist.so.normal";
    tz_file_open_req_t r={.flags=O_RDWR|O_CREAT|O_EXCL};
    snprintf(r.pathname,sizeof(r.pathname),"/mnt/vendor/persist/data/%s",name);
    int handle=storage_open(&s,&r);assert(handle==100);
    int fd=storage_fd(&s,handle);assert(fd>=0);
    assert(write(fd,"sentinel",8)==8);
    struct stat st;assert(!storage_check_file(fd,&st) && st.st_size==8);
    assert(storage_fd(&s,99)<0 && errno==EBADF);
    assert(storage_fd(&s,108)<0 && errno==EBADF);
    assert(storage_fd(&s,101)<0 && errno==EBADF);
    assert(!close(fd));s.files[0]=-1;
    const char *denied[]={"/etc/shadow","/mnt/vendor/persist/data/../gf_cali_persist.so.normal",
      "/mnt/vendor/persist/data/gf_cali_persist.so.normal/extra",
      "/mnt/vendor/persist/data/other",
      "/mnt/vendor/persist/data/finger_0_0.so",
      "/mnt/vendor/persist/data/finger_941002_5.so",
      "/denial-fingerprint/uff/global_finger.db.bak.bak.bak","/denial-fingerprint/uff/../../etc/shadow"};
    for (size_t i=0;i<sizeof(denied)/sizeof(denied[0]);i++) assert(!storage_name(denied[i]) && errno==EACCES);
    assert(!strcmp(storage_name("/denial-fingerprint/uff/global_finger.db"),"uff-global_finger.db"));
    assert(!strcmp(storage_name("/mnt/vendor/persist/data/finger_941002_4.so"),"uff-finger_941002_4.so"));
    char unterminated[256];memset(unterminated,'x',sizeof(unterminated));
    assert(!storage_name(unterminated));
    r.flags=O_RDONLY|O_TRUNC;assert(storage_open(&s,&r)<0 && errno==EINVAL);
    r.flags=O_RDWR|O_DIRECTORY;assert(storage_open(&s,&r)<0 && errno==EINVAL);
    /* A rejected inode must never be truncated before validation. */
    assert(!fchmodat(s.directory,name,0644,0));r.flags=O_RDWR|O_TRUNC;
    assert(storage_open(&s,&r)<0 && errno==EACCES);
    assert(!fstatat(s.directory,name,&st,AT_SYMLINK_NOFOLLOW) && st.st_size==8);
    assert(!fchmodat(s.directory,name,0600,0));
    assert(!linkat(s.directory,name,s.directory,"witness",0));
    assert(storage_open(&s,&r)<0 && errno==EACCES);
    assert(!fstatat(s.directory,name,&st,AT_SYMLINK_NOFOLLOW) && st.st_size==8);
    assert(!unlinkat(s.directory,"witness",0));
    fd=openat(s.directory,name,O_RDWR|O_NOFOLLOW);assert(fd>=0);
    assert(!ftruncate(fd,UFF_STORAGE_LIMIT+1));close(fd);
    assert(storage_open(&s,&r)<0 && errno==EACCES);
    assert(!fstatat(s.directory,name,&st,AT_SYMLINK_NOFOLLOW) && st.st_size==UFF_STORAGE_LIMIT+1);
    assert(!unlinkat(s.directory,name,0));
    assert(!symlinkat("/etc/shadow",s.directory,name));
    assert(storage_open(&s,&r)<0 && errno==ELOOP);
    assert(!unlinkat(s.directory,name,0));
    assert(!mkfifoat(s.directory,name,0600));
    assert(storage_open(&s,&r)<0 && errno==EACCES);
    assert(!unlinkat(s.directory,name,0));
    void *gp=calloc(1,GPFILE_SERVICE_BUF_LEN);assert(gp);
    tz_gpfile_write_req_t *wr=gp;
    wr->cmd_id=1;strcpy(wr->pathname,"/denial-fingerprint/uff/123");wr->count=7;memcpy(wr->buf,"fixture",7);
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN));
    assert(!((tz_gpfile_write_rsp_t *)gp)->err && ((tz_gpfile_write_rsp_t *)gp)->num_bytes_written==7);
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);tz_gpfile_read_req_t *rd=gp;
    rd->cmd_id=0;strcpy(rd->pathname,"/denial-fingerprint/uff/123");rd->count=20;
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN));
    assert(!((tz_gpfile_read_rsp_t *)gp)->err && ((tz_gpfile_read_rsp_t *)gp)->num_bytes_read==7);
    assert(!memcmp(((tz_gpfile_read_rsp_t *)gp)->buf,"fixture",7));
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);wr=gp;
    wr->cmd_id=1;strcpy(wr->pathname,"/denial-fingerprint/uff/123");wr->offset=7;wr->count=1;wr->backup=1;wr->buf[0]='!';
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && !((tz_gpfile_write_rsp_t *)gp)->err);
    assert(!fstatat(s.directory,"uff-template-123.bak",&st,AT_SYMLINK_NOFOLLOW) && st.st_size==7);
    assert(!fstatat(s.directory,"uff-template-123",&st,AT_SYMLINK_NOFOLLOW) && st.st_size==8);
    for (size_t i=0;i<sizeof(denied)/sizeof(denied[0]);i++) {
        memset(gp,0,GPFILE_SERVICE_BUF_LEN);wr=gp;wr->cmd_id=1;strcpy(wr->pathname,denied[i]);wr->count=1;
        assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && ((tz_gpfile_write_rsp_t *)gp)->err==EACCES);
    }
    const char *badgp[]={"/denial-fingerprint/uff/0","/denial-fingerprint/uff/0123","/denial-fingerprint/uff/4294967296","/denial-fingerprint/uff/123.bak.bak.bak","/denial-fingerprint/uff/123/extra"};
    char mapped[80];
    assert(!gp_name("/denial-fingerprint/uff/global_finger.db",mapped) && !strcmp(mapped,"uff-global_finger.db"));
    assert(!gp_name("/denial-fingerprint/uff/global_finger.db.bak.bak",mapped));
    assert(gp_name("/denial-fingerprint/uff/global_finger.db.bak.bak.bak",mapped)<0);
    for(size_t i=0;i<sizeof(badgp)/sizeof(badgp[0]);i++) assert(gp_name(badgp[i],mapped)<0);
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);wr=gp;wr->cmd_id=1;strcpy(wr->pathname,"/denial-fingerprint/uff/123");wr->offset=-1;wr->count=1;
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && ((tz_gpfile_write_rsp_t *)gp)->err==EINVAL);
    assert(!fstatat(s.directory,"uff-template-123",&st,AT_SYMLINK_NOFOLLOW) && st.st_size==8);
    assert(!symlinkat("/etc/shadow",s.directory,"uff-template-124"));
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);tz_gpfile_rename_req_t *rename=gp;
    rename->cmd_id=3;strcpy(rename->from,"/denial-fingerprint/uff/123");strcpy(rename->to,"/denial-fingerprint/uff/124");
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && ((tz_gpfile_err_rsp_t *)gp)->err==ELOOP);
    assert(!unlinkat(s.directory,"uff-template-124",0));
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);rename=gp;
    rename->cmd_id=3;strcpy(rename->from,"/denial-fingerprint/uff/123");strcpy(rename->to,"/denial-fingerprint/uff/124");
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && !((tz_gpfile_err_rsp_t *)gp)->err);
    assert(!linkat(s.directory,"uff-template-124",s.directory,"witness",0));
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);tz_gpfile_remove_req_t *remove=gp;
    remove->cmd_id=2;strcpy(remove->pathname,"/denial-fingerprint/uff/124");
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && ((tz_gpfile_err_rsp_t *)gp)->err==EACCES);
    assert(!unlinkat(s.directory,"witness",0));
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);remove=gp;remove->cmd_id=2;strcpy(remove->pathname,"/denial-fingerprint/uff/124");
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && !((tz_gpfile_err_rsp_t *)gp)->err);
    assert(!unlinkat(s.directory,"uff-template-123.bak",0));
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);wr=gp;wr->cmd_id=1;strcpy(wr->pathname,"/denial-fingerprint/uff/125");
    wr->count=GPFILE_SERVICE_BUF_LEN-offsetof(tz_gpfile_write_req_t,buf);
    memset((char *)gp+offsetof(tz_gpfile_write_req_t,buf),0x5a,wr->count);
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && !((tz_gpfile_write_rsp_t *)gp)->err);
    assert(((tz_gpfile_write_rsp_t *)gp)->num_bytes_written==515824);
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);rd=gp;rd->cmd_id=0;strcpy(rd->pathname,"/denial-fingerprint/uff/125");rd->count=515824;
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && !((tz_gpfile_read_rsp_t *)gp)->err);
    assert(((tz_gpfile_read_rsp_t *)gp)->num_bytes_read==515824);
    for(size_t i=0;i<515824;i++) assert(*((unsigned char *)gp+offsetof(tz_gpfile_read_rsp_t,buf)+i)==0x5a);
    memset(gp,0,GPFILE_SERVICE_BUF_LEN);wr=gp;wr->cmd_id=1;strcpy(wr->pathname,"/denial-fingerprint/uff/125");wr->count=515825;
    assert(!storage_gp(&s,gp,GPFILE_SERVICE_BUF_LEN) && ((tz_gpfile_err_rsp_t *)gp)->err==EINVAL);
    assert(!unlinkat(s.directory,"uff-template-125",0));
    explicit_bzero(gp,GPFILE_SERVICE_BUF_LEN);free(gp);
    puts("GP storage read/write/backup/rename/remove, bounds and namespace confinement: PASS");
    close(s.directory);assert(!rmdir(directory));
    puts("Storage confinement: PASS (paths, handles, flags, modes, hardlinks, symlinks, size limit, FIFO, pre-truncation checks)");
    return 0;
}
