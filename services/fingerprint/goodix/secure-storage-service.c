/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* One owner for GPFS and read-only RPMB. No credential requests or key bytes
 * enter this daemon's interface; callbacks carry opaque encrypted storage. */
#define _GNU_SOURCE
#include "qcomtee_object.h"
#include "qcomtee_object_types.h"
#include "secure-storage.h"
#include <errno.h>
#include <fcntl.h>
#include <linux/bsg.h>
#include <poll.h>
#include <pthread.h>
#include <scsi/sg.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/un.h>
#include <unistd.h>
static struct qcomtee_param in(void *p,size_t n)
{return (struct qcomtee_param){.attr=QCOMTEE_UBUF_INPUT,.ubuf={p,n}};}
static struct qcomtee_param obj(struct qcomtee_object *o)
{return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_INPUT,.object=o};}
static struct qcomtee_param newobj(void)
{return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_OUTPUT};}
static int invoke(struct qcomtee_object *o,const char *name,uint32_t op,struct qcomtee_param *p,int n)
{
    qcomtee_result_t service=UINT32_MAX;int ret=qcomtee_object_invoke(o,op,p,n,&service);
    if(ret || service)fprintf(stderr,"secure storage %s transport=%d service=%d\n",name,ret,(int32_t)service);
    return ret || service?-EIO:0;
}
/* Reuse the confined Goodix template path handling; it never resolves a
 * protected SFS path or writes factory calibration. Registration stays here. */
#include "goodix-storage.h"
static volatile sig_atomic_t stopping;
static void stop(int sig){(void)sig;stopping=1;}
struct service;
struct listener {
    struct qcomtee_object callback;
    struct qcomtee_object *memory;
    struct service *service;
    uint32_t id,is64;
    size_t size;
};
struct service {
    struct qcomtee_object *root,*env,*registration;
    struct listener *listeners[2];
    struct secure_store store;
    struct goodix_storage goodix;
    int rpmb,lock;
    bool authenticated;
};
static int rpmb_io(void *opaque,unsigned opcode,void *data,uint32_t n)
{
    struct service *s=opaque;
    /* This function is private to secure_rpmb, whose request-type validation
     * is tested independently. No raw SCSI or writable RPMB API is exported. */
    if(opcode!=0xb5 && opcode!=0xa2)return -EPERM;
    for(unsigned attempt=0;attempt<2;attempt++){
        uint8_t cdb[12]={opcode,0xec,0,1},sense[96]={0};
        cdb[6]=n>>24;cdb[7]=n>>16;cdb[8]=n>>8;cdb[9]=n;
        struct sg_io_v4 io={.guard='Q',.protocol=BSG_PROTOCOL_SCSI,.subprotocol=BSG_SUB_PROTOCOL_SCSI_CMD,
            .request_len=12,.request=(uintptr_t)cdb,.response=(uintptr_t)sense,.max_response_len=sizeof(sense),.timeout=5000};
        if(opcode==0xb5){io.dout_xfer_len=n;io.dout_xferp=(uintptr_t)data;}
        else{io.din_xfer_len=n;io.din_xferp=(uintptr_t)data;}
        int ret=ioctl(s->rpmb,SG_IO,&io);
        if(!ret && !io.device_status && !io.transport_status && !io.driver_status && !io.info &&
            (opcode==0xb5?!io.dout_resid:!io.din_resid))return 0;
        /* UFS reports UNIT ATTENTION once after boot. Retry that rejected
         * read request once; never retry an authenticated storage write. */
        unsigned key=(sense[0]&0x7f)==0x72?sense[1]&15:sense[2]&15;
        bool read_only=opcode==0xa2 || (n==512 && ((uint8_t *)data)[510]==0 &&
            (((uint8_t *)data)[511]==2 || ((uint8_t *)data)[511]==4));
        if(ret || !read_only || key!=6 || io.transport_status || attempt)return ret?-errno:-EIO;
    }
    return -EIO;
}
static qcomtee_result_t dispatch(struct qcomtee_object *o,qcomtee_op_t op,struct qcomtee_param *p,int n)
{
    if(op==1 && !n)return 0;
    if(op || n!=6 || p[0].attr!=QCOMTEE_UBUF_OUTPUT || p[1].attr!=QCOMTEE_UBUF_OUTPUT || p[1].ubuf.size<4)return 2;
    for(int i=2;i<6;i++)if(p[i].attr!=QCOMTEE_OBJREF_OUTPUT)return 2;
    struct listener *l=container_of(o,struct listener,callback);struct service *s=l->service;
    uint8_t *memory=qcomtee_memory_object_addr(l->memory);int ret;char name[80];
    if(l->id==0x2000)ret=s->authenticated?secure_rpmb_authenticated(memory,l->size,rpmb_io,s):secure_rpmb(memory,l->size,rpmb_io,s);
    else if(storage_name((char *)memory+4,name)>=0){
        struct goodix_listener compatible={.memory=l->memory,.storage=&s->goodix,.size=l->size,.id=l->id};
        ret=storage_gp(&compatible);
    }else ret=secure_gpfs(&s->store,memory,l->size);
    p[0].ubuf.addr=NULL;p[0].ubuf.size=0;l->is64=1;
    p[1].ubuf.addr=&l->is64;p[1].ubuf.size=4;
    for(int i=2;i<6;i++)p[i].object=NULL;
    return ret?2:0;
}
static void release(struct qcomtee_object *o)
{
    struct listener *l=container_of(o,struct listener,callback);
    if(l->memory){explicit_bzero(qcomtee_memory_object_addr(l->memory),l->size);qcomtee_memory_object_release(l->memory);}
    free(l);
}
static int supported(qcomtee_op_t op){return op<=1 || op==QCOMTEE_OBJREF_OP_RELEASE;}
static struct qcomtee_object_ops ops={.release=release,.dispatch=dispatch,.supported=supported};
static int private_directory(const char *path)
{
    if(mkdir(path,0700) && errno!=EEXIST)return -1;
    int fd=open(path,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);struct stat st;
    if(fd>=0 && (fstat(fd,&st) || st.st_uid || (st.st_mode&077))){close(fd);errno=EACCES;return -1;}
    return fd;
}
static int board(void)
{
    char b[256];const char expected[]="nubia,nx809j";
    int fd=open("/sys/firmware/devicetree/base/compatible",O_RDONLY|O_CLOEXEC|O_NOFOLLOW);if(fd<0)return -1;
    ssize_t n=read(fd,b,sizeof(b));close(fd);return n>=(ssize_t)sizeof(expected) && !memcmp(b,expected,sizeof(expected))?0:-1;
}
static int ready(void)
{
    const char *path=getenv("NOTIFY_SOCKET");if(!path)return 0;
    struct sockaddr_un a={.sun_family=AF_UNIX};size_t n=strlen(path);
    if(!n || n>=sizeof(a.sun_path))return -1;
    memcpy(a.sun_path,path,n+1);if(a.sun_path[0]=='@')a.sun_path[0]=0;
    int fd=socket(AF_UNIX,SOCK_DGRAM|SOCK_CLOEXEC,0);if(fd<0)return -1;
    ssize_t sent=sendto(fd,"READY=1",7,MSG_NOSIGNAL,(void *)&a,offsetof(struct sockaddr_un,sun_path)+n+(path[0]!='@'));
    close(fd);return sent==7?0:-1;
}
int main(int argc,char **argv)
{
    (void)argv;
    if(argc!=1 || geteuid() || board() || prctl(PR_SET_DUMPABLE,0))return 2;
    umask(077);struct sigaction action={.sa_handler=stop};
    sigemptyset(&action.sa_mask);sigaction(SIGTERM,&action,NULL);sigaction(SIGINT,&action,NULL);signal(SIGPIPE,SIG_IGN);
    struct service s={.store={.base=-1,.persist=-1,.data=-1},.goodix={.calibration=-1,.store=-1},.rpmb=-1,.lock=-1};int ret=1;
    /* Fixed deployment paths: the daemon cannot be directed at arbitrary
     * partitions or a different private credential store through argv/env. */
    s.lock=open("/var/lib/nx809j-secure-storage/listener.lock",O_RDWR|O_CREAT|O_CLOEXEC|O_NOFOLLOW,0600);
    const char *commit=getenv("NX809J_CANONICAL_AUTHENTICATED_STORAGE");
    s.authenticated=commit && !strcmp(commit,"1");
    struct stat st;
    if(s.lock<0 || fstat(s.lock,&st) || !S_ISREG(st.st_mode) || st.st_uid || st.st_nlink!=1 || (st.st_mode&077) || flock(s.lock,LOCK_EX|LOCK_NB))goto done;
    s.store.base=open("/run/nx809j/securestorage/data",O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);
    struct statvfs vfs;
    if(s.store.base<0 || fstatvfs(s.store.base,&vfs) ||
        (s.authenticated?!!(vfs.f_flag&ST_RDONLY):!(vfs.f_flag&ST_RDONLY)))goto done;
    if(s.authenticated){s.store.canonical=1;s.store.persist=fcntl(s.store.base,F_DUPFD_CLOEXEC,3);}
    else s.store.persist=private_directory("/var/lib/nx809j-secure-storage/persist");
    s.store.data=private_directory("/var/lib/nx809j-secure-storage/data");
    s.goodix.store=private_directory("/var/lib/nx809j-fingerprint/templates");
    s.goodix.calibration=storage_open_dir("/var/lib/nx809j-fingerprint/calibration",0);
    if(s.store.persist<0 || s.store.data<0 || s.goodix.store<0 || s.goodix.calibration<0)goto done;
    s.rpmb=open("/dev/bsg/0:0:0:49476",O_RDWR|O_CLOEXEC|O_NOFOLLOW);
    if(s.rpmb<0 || fstat(s.rpmb,&st) || !S_ISCHR(st.st_mode))goto done;
    s.root=qcomtee_object_root_init("/dev/tee0",ioctl,NULL,NULL);if(!s.root)goto done;
    uint8_t identity[]={0xa2,1,0,3,0x6e,'s','e','c','u','r','e','-','s','t','o','r','a','g','e'};
    struct qcomtee_param reg[]={in(identity,sizeof(identity)),newobj()};
    if(invoke(s.root,"register actual root identity",1,reg,2))goto done;
    s.env=reg[1].object;uint32_t id=87;
    struct qcomtee_param open[]={in(&id,4),newobj()};if(invoke(s.env,"open listener registration",0,open,2))goto done;
    s.registration=open[1].object;
    for(unsigned i=0;i<2;i++){
        struct listener *l=calloc(1,sizeof(*l));if(!l)goto done;
        l->id=i?0x2000:0x7000;l->size=i?20040:GPFILE_SERVICE_BUF_LEN;l->service=&s;
        if(qcomtee_object_cb_init(&l->callback,&ops,s.root)){free(l);goto done;}
        s.listeners[i]=l;if(qcomtee_memory_object_alloc(l->size,s.root,&l->memory))goto done;
        memset(qcomtee_memory_object_addr(l->memory),0,l->size);qcomtee_object_refs_inc(&l->callback);
        struct qcomtee_param p[]={in(&l->id,4),obj(&l->callback),obj(l->memory)};
        qcomtee_result_t status=UINT32_MAX;int r=qcomtee_object_invoke(s.registration,0,p,3,&status);
        if(r)qcomtee_object_refs_dec(&l->callback);
        fprintf(stderr,"secure storage listener=%#x transport=%d service=%d\n",l->id,r,(int32_t)status);
        if(r || status)goto done;
    }
    if(ready())goto done;
    fprintf(stderr,"Secure storage ready: %s\n",s.authenticated?
        "canonical protected-file commit and authenticated RPMB; key programming denied":
        "private Linux file overlay; stock persist and RPMB read-only");
    ret=0;while(!stopping)if(qcomtee_object_process_one(s.root)){if(!stopping)ret=1;break;}
 done:
    if(ret)fprintf(stderr,"secure storage startup/transport failure errno=%d\n",errno);
    if(s.registration)qcomtee_object_refs_dec(s.registration);
    for(unsigned i=0;i<2;i++)if(s.listeners[i])qcomtee_object_refs_dec(&s.listeners[i]->callback);
    if(s.env)qcomtee_object_refs_dec(s.env);
    if(s.root)qcomtee_object_refs_dec(s.root);
    int fds[]={s.lock,s.rpmb,s.store.base,s.store.persist,s.store.data,s.goodix.store,s.goodix.calibration};
    for(unsigned i=0;i<sizeof(fds)/sizeof(*fds);i++)if(fds[i]>=0)close(fds[i]);
    return ret;
}
