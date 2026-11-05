/* SPDX-License-Identifier: GPL-2.0-only */
/* JIIOV ancapp64 worker behind the shared fingerprint helper: UFF1 and UFW1
 * records on the inherited stdout, commands on stdin. QTEE through the
 * upstream qcomtee driver (/dev/tee0), the sensor's power and reset through
 * /dev/tee_fp. Only metadata leaves this process; diagnostics go to stderr.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <linux/tee_fp.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include "qcomtee_object.h"
#include "qcomtee_object_types.h"
#include "jiiov-files.h"

#define JIIOV_CALIBRATION "/var/lib/phonewave-fingerprint/calibration"
#define JIIOV_TEMPLATES "/var/lib/phonewave-fingerprint/templates"
#define JIIOV_TEMPLATE_GROUP 941001
#define JIIOV_TEMPLATE_LIMIT 5
#define JIIOV_CHIP 0x46544301

static volatile sig_atomic_t worker_stopping;
static void stop_worker(int signal_number) { (void)signal_number; worker_stopping=1; }

static struct qcomtee_param in(void *p, size_t n)
{ return (struct qcomtee_param){.attr=QCOMTEE_UBUF_INPUT,.ubuf={p,n}}; }
static struct qcomtee_param out(void *p, size_t n)
{ return (struct qcomtee_param){.attr=QCOMTEE_UBUF_OUTPUT,.ubuf={p,n}}; }
static struct qcomtee_param obj(struct qcomtee_object *p)
{ return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_INPUT,.object=p}; }
static struct qcomtee_param newobj(void)
{ return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_OUTPUT}; }
/* The listener thread is cancelled inside its blocking wait. */
static int tee_call(int fd, unsigned long op, ...)
{
    va_list ap; va_start(ap,op); void *p=va_arg(ap,void *); va_end(ap);
    int old; pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS,&old);
    int ret=ioctl(fd,op,p),saved=errno;
    pthread_setcanceltype(old,NULL); errno=saved;
    return ret;
}
static int invoke(struct qcomtee_object *o, const char *name, uint32_t op,
                  struct qcomtee_param *p, int count)
{
    qcomtee_result_t result=UINT32_MAX;
    int ret=qcomtee_object_invoke(o,op,p,count,&result);
    fprintf(stderr,"%s: transport=%d service=%d\n",name,ret,(int32_t)result);
    return ret ? -1 : (int32_t)result;
}
static int64_t milliseconds(void)
{
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC,&now);
    return (int64_t)now.tv_sec*1000+now.tv_nsec/1000000;
}

/* The FS and GPFS listeners, each a callback object with its own buffer. */
struct listener {
    struct qcomtee_object callback;
    struct qcomtee_object *memory;
    size_t size;
    uint32_t id,is64;
};
static qcomtee_result_t listener_dispatch(struct qcomtee_object *object,qcomtee_op_t op,
                                          struct qcomtee_param *p,int n)
{
    if (op==1 && !n) return 0;
    if (op || n!=6 || p[0].attr!=QCOMTEE_UBUF_OUTPUT || p[1].attr!=QCOMTEE_UBUF_OUTPUT || p[1].ubuf.size<4) return 2;
    for (int i=2;i<6;i++) if (p[i].attr!=QCOMTEE_OBJREF_OUTPUT) return 2;
    struct listener *l=container_of(object,struct listener,callback);
    void *memory=qcomtee_memory_object_addr(l->memory);
    int ret=l->id==FILE_SERVICE_ID ? fs_dispatch(memory) : gp_dispatch(memory);
    /* Output buffers have a capacity but no address; keep the reply alive. */
    p[0].ubuf.addr=NULL;p[0].ubuf.size=0;
    l->is64=1;p[1].ubuf.addr=&l->is64;p[1].ubuf.size=4;
    for (int i=2;i<6;i++) p[i].object=NULL;
    return ret;
}
static int listener_supported(qcomtee_op_t op) {return op<=1 || op==QCOMTEE_OBJREF_OP_RELEASE;}
static void listener_release(struct qcomtee_object *o)
{
    struct listener *l=container_of(o,struct listener,callback);
    if (l->memory) {explicit_bzero(qcomtee_memory_object_addr(l->memory),l->size);qcomtee_memory_object_release(l->memory);}
    free(l);
}
static void listener_error(struct qcomtee_object *o,int e) {(void)o;if (e) fprintf(stderr,"listener callback error=%d\n",e);}
static struct qcomtee_object_ops listener_ops={.release=listener_release,.dispatch=listener_dispatch,
    .error=listener_error,.supported=listener_supported};
static void *listener_worker(void *root)
{
    for (;;) {pthread_testcancel();if (qcomtee_object_process_one(root)) break;}
    return NULL;
}

struct device {
    struct qcomtee_object *root,*env,*loader,*app,*registration,*group;
    struct listener *listeners[2];
    pthread_t thread;
    int control;
    bool thread_started,powered,initialized,algorithm,cleanup_ok;
    uint32_t ids[JIIOV_TEMPLATE_LIMIT],count;
};

static int private_directory(const char *path)
{
    if (mkdir(path,0700) && errno!=EEXIST) return -1;
    int fd=open(path,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);
    struct stat st;
    if (fd>=0 && (fstat(fd,&st) || st.st_uid || (st.st_mode&077))) {close(fd);errno=EACCES;return -1;}
    return fd;
}
static int files_start(struct device *d)
{
    fs.directory=open(JIIOV_CALIBRATION,O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOFOLLOW);
    fs.storage_directory=private_directory(JIIOV_TEMPLATES);
    if (fs.directory<0 || fs.storage_directory<0) {perror("fingerprint state");return -1;}
    uint32_t id=87;
    struct qcomtee_param args[]={in(&id,4),newobj()};
    if (invoke(d->env,"open IRegisterListenerCBO",0,args,2)) return -1;
    d->registration=args[1].object;
    int err=pthread_create(&d->thread,NULL,listener_worker,d->root);
    if (err) {errno=err;perror("listener thread");return -1;}
    d->thread_started=true;
    for (int i=0;i<2;i++) {
        struct listener *l=calloc(1,sizeof(*l));
        if (!l) return -1;
        l->id=i?GPFILE_SERVICE_ID:FILE_SERVICE_ID;
        l->size=i?GPFILE_SERVICE_BUF_LEN:FILE_SERVICE_BUF_LEN;
        if (qcomtee_object_cb_init(&l->callback,&listener_ops,d->root)) {free(l);return -1;}
        d->listeners[i]=l;
        if (qcomtee_memory_object_alloc(l->size,d->root,&l->memory)) return -1;
        memset(qcomtee_memory_object_addr(l->memory),0,l->size);
        /* A successful invoke hands one reference to QTEE, even on rejection. */
        qcomtee_object_refs_inc(&l->callback);
        struct qcomtee_param reg[]={in(&l->id,4),obj(&l->callback),obj(l->memory)};
        qcomtee_result_t status=UINT32_MAX;
        int r=qcomtee_object_invoke(d->registration,0,reg,3,&status);
        fprintf(stderr,"register listener %#x: transport=%d service=%d\n",l->id,r,(int32_t)status);
        if (r) qcomtee_object_refs_dec(&l->callback);
        if (r || status) return -1;
    }
    return 0;
}
/* After the app is unloaded: releasing the registration unregisters only
 * these listeners. */
static void files_stop(struct device *d)
{
    if (d->registration) {qcomtee_object_refs_dec(d->registration);d->registration=NULL;}
    if (d->thread_started) {pthread_cancel(d->thread);pthread_join(d->thread,NULL);d->thread_started=false;}
    for (int i=0;i<2;i++) if (d->listeners[i]) {qcomtee_object_refs_dec(&d->listeners[i]->callback);d->listeners[i]=NULL;}
    for (int i=0;i<8;i++) if (fs.files[i]>=0) {close(fs.files[i]);fs.files[i]=-1;}
    if (fs.directory>=0) {close(fs.directory);fs.directory=-1;}
    if (fs.storage_directory>=0) {close(fs.storage_directory);fs.storage_directory=-1;}
}

static struct qcomtee_object *load(struct qcomtee_object *root,struct qcomtee_object *loader,int fd,size_t size)
{
    struct qcomtee_object *memory=NULL,*app=NULL;
    size_t offset=0;
    if (qcomtee_memory_object_alloc(size,root,&memory)) return NULL;
    unsigned char *bytes=qcomtee_memory_object_addr(memory);
    while (offset<size) {
        ssize_t n=pread(fd,bytes+offset,size-offset,(off_t)offset);
        if (n<0 && errno==EINTR) continue;
        if (n<=0) goto done;
        offset+=(size_t)n;
    }
    if (memcmp(bytes,"\177ELF\002\001",6)) goto done;
    char name[]="ancapp64";
    struct qcomtee_param p[]={in(name,sizeof(name)-1),obj(memory),newobj()};
    if (!invoke(loader,"load ancapp64",0,p,3)) app=p[2].object;
 done:
    qcomtee_memory_object_release(memory);
    return app;
}

/* IQSEEComCompatApp sendRequest, as stock libancapp calls it: a request and
 * response pair, and optionally one shared buffer whose address the
 * firmware reads at request offset 4. */
static int command_with(struct device *d,uint32_t group,uint32_t command,uint32_t parameter,
                        uint32_t response[ANC_RESPONSE_SIZE/4],struct qcomtee_object *buffer,uint32_t size)
{
    uint32_t request[ANC_REQUEST_SIZE/4]={0},blank[ANC_RESPONSE_SIZE/4]={0},request_out[ANC_REQUEST_SIZE/4]={0};
    uint32_t offsets[4]={4,0,0,0},is64=1,status;
    memset(response,0,ANC_RESPONSE_SIZE);
    anc_request_init((void *)request,group,command,parameter,(uint64_t)milliseconds());
    /* Stock AlgoVerifyExtractFeature sets sub-payload 0x14 for verification. */
    if (group==ANC_GROUP_ALGORITHM && command==ANC_EXTRACT && parameter==1) request[0x30/4]=1;
    if (buffer) request[3]=size;
    struct qcomtee_param p[]={in(request,sizeof(request)),in(blank,sizeof(blank)),
        buffer?in(offsets,sizeof(offsets)):in(NULL,0),in(&is64,4),
        out(request_out,sizeof(request_out)),out(response,ANC_RESPONSE_SIZE),
        obj(buffer),obj(NULL),obj(NULL),obj(NULL)};
    fprintf(stderr,"ancapp64 group=%u command=%u parameter=%u\n",group,command,parameter);
    int ret=invoke(d->app,"ancapp64 sendRequest",0,p,10);
    if (ret) return ret<0 ? -EIO : -ret;
    if (anc_response_status(response,p[5].ubuf.size,&status)) return -EPROTO;
    return (int)status;
}
static int command(struct device *d,uint32_t group,uint32_t command,uint32_t parameter,
                   uint32_t response[ANC_RESPONSE_SIZE/4])
{return command_with(d,group,command,parameter,response,NULL,0);}

/* The app, its listeners and the control node, with the sensor unpowered.
 * A cancelled operation must still run device_close. */
static int device_open(struct device *d,const char *firmware)
{
    int fd=open(firmware,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);
    struct stat st;
    if (fd<0 || fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_uid || (st.st_mode&022) ||
        st.st_size<64 || st.st_size>32*1024*1024) {if (fd>=0) close(fd);return -EACCES;}
    int ret=-EIO;
    d->root=qcomtee_object_root_init("/dev/tee0",tee_call,NULL,NULL);
    if (!d->root) {ret=-ENODEV;goto done;}
    uint32_t uid=geteuid();
    unsigned char credentials[]={0xa2,1,0x1a,uid>>24,uid>>16,uid>>8,uid,3,0x6c,
        'j','i','i','o','v','-','w','o','r','k','e','r'};
    struct qcomtee_param reg[]={in(credentials,sizeof(credentials)),newobj()};
    if (invoke(d->root,"registerLegacy",1,reg,2)) {ret=-EACCES;goto done;}
    d->env=reg[1].object;
    uint32_t loader_id=122;
    struct qcomtee_param open_args[]={in(&loader_id,4),newobj()};
    if (invoke(d->env,"open IQSEEComCompatAppLoader",0,open_args,2)) goto done;
    d->loader=open_args[1].object;
    /* The previous worker may still be unloading the app. */
    for (int tries=0;;tries++) {
        char name[]="ancapp64";uint32_t arch=0;
        struct qcomtee_param find[]={in(name,sizeof(name)-1),out(&arch,4),newobj()};
        int present=invoke(d->loader,"lookup ancapp64",2,find,3);
        if (present==23) break;
        if (present) goto done;
        qcomtee_object_refs_dec(find[2].object);
        if (tries==20 || worker_stopping) {ret=-EBUSY;goto done;}
        usleep(50000);
    }
    if (files_start(d)) goto done;
    d->app=load(d->root,d->loader,fd,(size_t)st.st_size);
    if (!d->app) goto done;
    d->control=open("/dev/tee_fp",O_RDWR|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
    struct stat node;struct tee_fp_state state;
    if (d->control<0 || fstat(d->control,&node) || !S_ISCHR(node.st_mode) || node.st_uid ||
        (node.st_mode&077) || ioctl(d->control,TEE_FP_IOC_GET_STATE,&state) ||
        state.version!=1 || state.powered) {ret=-EACCES;goto done;}
    ret=0;
 done:
    close(fd);
    return ret;
}
static int list_templates(struct device *d)
{
    uint32_t r[ANC_RESPONSE_SIZE/4];
    if (command(d,ANC_GROUP_ALGORITHM,ANC_TEMPLATE_LIST,0,r)) return -1;
    uint32_t count=anc_get_le32((char *)r+0x28);
    if (count>JIIOV_TEMPLATE_LIMIT) return -1;
    for (uint32_t i=0;i<count;i++) {
        uint32_t id=anc_get_le32((char *)r+0x14+4*i);
        if (!id) return -1;
        for (uint32_t j=0;j<i;j++) if (d->ids[j]==id) return -1;
        d->ids[i]=id;
    }
    d->count=count;
    return 0;
}
static bool listed(struct device *d,uint32_t id)
{
    for (uint32_t i=0;i<d->count;i++) if (d->ids[i]==id) return true;
    return false;
}
/* Power, chip check, algorithm, optical calibration and this phone's
 * template group, as stock fingerprint startup does them. */
static int sensor_up(struct device *d)
{
    uint32_t r[ANC_RESPONSE_SIZE/4];
    int64_t start=milliseconds();
    if (ioctl(d->control,TEE_FP_IOC_ENABLE_POWER)) return -1;
    d->powered=true;
    if (ioctl(d->control,TEE_FP_IOC_RESET) || worker_stopping) return -1;
    if (command(d,ANC_GROUP_SENSOR,1,0,r)) return -1;
    d->initialized=true;
    if (command(d,ANC_GROUP_SENSOR,6,0,r) || r[1]!=JIIOV_CHIP) {
        fprintf(stderr,"Unexpected JIIOV chip 0x%08x\n",r[1]);return -1;
    }
    if (command(d,ANC_GROUP_SENSOR,4,0x33,r) || r[2]!=0x33 || worker_stopping) return -1;
    if (command(d,ANC_GROUP_ALGORITHM,ANC_ALGORITHM_INIT,0,r)) return -1;
    d->algorithm=true;
    if (command(d,ANC_GROUP_EXTENSION,25,0,r) || worker_stopping) return -1;
    if (qcomtee_memory_object_alloc(4096,d->root,&d->group)) return -1;
    char *path=qcomtee_memory_object_addr(d->group);
    memset(path,0,4096);memcpy(path,JIIOV_TEMPLATE_ROOT,sizeof(JIIOV_TEMPLATE_ROOT));
    if (command_with(d,ANC_GROUP_ALGORITHM,ANC_TEMPLATE_SET_GROUP,JIIOV_TEMPLATE_GROUP,r,d->group,4096) ||
        command(d,ANC_GROUP_ALGORITHM,ANC_TEMPLATE_LOAD,0,r) || list_templates(d)) return -1;
    fprintf(stderr,"fingerprint timing sensor_ready_ms=%" PRId64 " templates=%u\n",milliseconds()-start,d->count);
    return 0;
}
static int device_close(struct device *d)
{
    uint32_t r[ANC_RESPONSE_SIZE/4];
    int status=0;
    if (d->algorithm && command(d,ANC_GROUP_ALGORITHM,ANC_ALGORITHM_DEINIT,0,r)) status=1;
    if (d->initialized && command(d,ANC_GROUP_SENSOR,2,0,r)) status=1;
    d->algorithm=d->initialized=false;
    if (d->control>=0) {
        struct tee_fp_state state;
        if (d->powered && ioctl(d->control,TEE_FP_IOC_DISABLE_POWER)) status=1;
        if (ioctl(d->control,TEE_FP_IOC_GET_STATE,&state) || state.version!=1 || state.powered) status=1;
        close(d->control);d->control=-1;d->powered=false;
    }
    if (d->app) {
        if (invoke(d->app,"unload ancapp64",2,NULL,0)) status=1;
        qcomtee_object_refs_dec(d->app);d->app=NULL;
    }
    files_stop(d);
    if (d->group) {qcomtee_memory_object_release(d->group);d->group=NULL;}
    if (d->loader) {qcomtee_object_refs_dec(d->loader);d->loader=NULL;}
    if (d->env) {qcomtee_object_refs_dec(d->env);d->env=NULL;}
    if (d->root) {qcomtee_object_refs_dec(d->root);d->root=NULL;}
    d->cleanup_ok=!status;
    return status;
}

static int worker_control(char line[16],int timeout_ms)
{
    size_t n=0;
    while (!worker_stopping && n<15) {
        struct pollfd fd={.fd=STDIN_FILENO,.events=POLLIN};
        int rc=poll(&fd,1,timeout_ms>1000?1000:timeout_ms);
        if (!rc && timeout_ms>1000) {timeout_ms-=1000;continue;}
        if (rc<0 && errno==EINTR) continue;
        if (rc<=0 || fd.revents&(POLLERR|POLLNVAL)) return -1;
        char c;
        ssize_t bytes=read(STDIN_FILENO,&c,1);
        if (bytes<0 && errno==EINTR) continue;
        if (bytes!=1 || (c<' ' && c!='\n')) return -1;
        if (c=='\n') {line[n]=0;return 0;}
        line[n++]=c;
    }
    return -1;
}
static int expect(const char *word,int timeout_ms)
{
    char line[16];
    return worker_control(line,timeout_ms) || strcmp(line,word) ? -1:0;
}
static int record(FILE *records,const char *format,...)
{
    va_list ap;va_start(ap,format);
    int n=vfprintf(records,format,ap);va_end(ap);
    return n<0 || fflush(records) ? -1:0;
}

struct result {uint32_t code,identifier;};

static int enroll(struct device *d,FILE *records,struct result *result)
{
    uint32_t r[ANC_RESPONSE_SIZE/4],old[JIIOV_TEMPLATE_LIMIT];
    uint32_t old_count=d->count;memcpy(old,d->ids,sizeof(old));
    if (old_count>=JIIOV_TEMPLATE_LIMIT) return -1;
    if (command(d,ANC_GROUP_ALGORITHM,ANC_ENROLL_TOTAL,0,r)) return -1;
    uint32_t total=anc_get_le32((char *)r+0x30),remaining=total;
    if (!total || total>100) return -1;
    if (command(d,ANC_GROUP_ALGORITHM,ANC_ENROLL_BEGIN,0,r)) return -1;
    int ret=-1;bool committed=false;
    if (record(records,"UFW1 prepared\n")) goto cleanup;
    for (unsigned attempts=0;attempts<100 && !worker_stopping;attempts++) {
        if (expect("capture",120000)) goto cleanup;
        uint32_t code=3;
        int capture=command(d,ANC_GROUP_SENSOR,5,0,r);
        if (capture<0 || record(records,"UFW1 captured\n")) goto cleanup;
        if (!capture) {
            int extract=command(d,ANC_GROUP_ALGORITHM,ANC_EXTRACT,0,r);
            if (extract<0) goto cleanup;
            if (extract==300) {
                int add=command(d,ANC_GROUP_ALGORITHM,ANC_ENROLL_ADD,0,r);
                if (add<0) goto cleanup;
                uint32_t next=anc_get_le32((char *)r+4);
                if (add==400 || add==401) {
                    if (next>remaining) goto cleanup;
                    remaining=next;code=0;
                }
            }
        }
        if (record(records,"UFW1 sample %u %u 0 %u\n",code,remaining,total) || expect("finish",120000)) goto cleanup;
        /* The helper sends finish only once the contact withdrew. */
        if (!code && !remaining) {
            if (worker_stopping || command(d,ANC_GROUP_ALGORITHM,ANC_ENROLL_END,1,r)) goto cleanup;
            committed=true;
            uint32_t id=anc_get_le32((char *)r+0x10);
            if (!id || listed(d,id) || list_templates(d) || d->count!=old_count+1 || !listed(d,id)) goto cleanup;
            for (uint32_t i=0;i<old_count;i++) if (old[i]==id) goto cleanup;
            result->code=0;result->identifier=id;ret=0;
            break;
        }
        if (record(records,"UFW1 idle\n")) goto cleanup;
    }
 cleanup:
    if (!committed && command(d,ANC_GROUP_ALGORITHM,ANC_ENROLL_END,0,r)) ret=-1;
    return worker_stopping?-1:ret;
}
static int verify(struct device *d,FILE *records,struct result *result)
{
    uint32_t r[ANC_RESPONSE_SIZE/4];
    if (!d->count || command(d,ANC_GROUP_ALGORITHM,ANC_VERIFY_BEGIN,0,r)) return -1;
    int ret=-1;
    if (record(records,"UFW1 prepared\n") || expect("capture",15000)) goto cleanup;
    int64_t start=milliseconds();
    int capture=command(d,ANC_GROUP_SENSOR,5,1,r);
    if (capture<0 || record(records,"UFW1 captured\n")) goto cleanup;
    int64_t captured=milliseconds();
    uint32_t code=3,id=0;
    if (!capture) {
        int extract=command(d,ANC_GROUP_ALGORITHM,ANC_EXTRACT,1,r);
        if (extract<0) goto cleanup;
        if (extract==300) {
            int compare=command(d,ANC_GROUP_ALGORITHM,ANC_COMPARE,0,r);
            if (compare<0) goto cleanup;
            code=1;
            if (compare==500) {
                id=anc_get_le32((char *)r+0x10);
                if (!listed(d,id)) goto cleanup;
                code=0;
            }
        }
    }
    fprintf(stderr,"fingerprint timing capture_ms=%" PRId64 " result_ms=%" PRId64 "\n",
            captured-start,milliseconds()-start);
    if (record(records,"UFW1 sample %u 0 %u 0\n",code,id) || expect("finish",15000)) goto cleanup;
    result->code=code;result->identifier=id;ret=0;
 cleanup:
    if (command(d,ANC_GROUP_ALGORITHM,ANC_VERIFY_END,0,r)) ret=-1;
    return worker_stopping?-1:ret;
}
/* An enrollment session opens and aborts; no image, no template. */
static int check_enroll(struct device *d)
{
    uint32_t r[ANC_RESPONSE_SIZE/4];
    if (d->count>=JIIOV_TEMPLATE_LIMIT || command(d,ANC_GROUP_ALGORITHM,ANC_ENROLL_BEGIN,0,r)) return -1;
    return command(d,ANC_GROUP_ALGORITHM,ANC_ENROLL_END,0,r) ? -1:0;
}
static int delete(struct device *d,uint32_t id)
{
    uint32_t r[ANC_RESPONSE_SIZE/4];
    uint32_t before=d->count;
    if (!listed(d,id) || command(d,ANC_GROUP_ALGORITHM,ANC_TEMPLATE_DELETE,id,r) ||
        list_templates(d) || d->count+1!=before || listed(d,id)) return -1;
    return 0;
}

static int present(void)
{
    struct stat driver,node;
    int fd=open("/dev/tee_fp",O_RDWR|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
    int status=stat("/sys/bus/platform/drivers/tee-fp",&driver) || !S_ISDIR(driver.st_mode) || fd<0 ||
        fstat(fd,&node) || !S_ISCHR(node.st_mode) || node.st_uid || (node.st_mode&077);
    if (fd>=0) close(fd);
    return status;
}

int main(int argc,char **argv)
{
    bool deleting=argc==4 && !strcmp(argv[1],"delete");
    if (geteuid() || (argc!=3 && !deleting)) return 2;
    const char *operation=argv[1];
    if (strcmp(operation,"probe") && strcmp(operation,"open") && strcmp(operation,"list") &&
        strcmp(operation,"check-enroll") && strcmp(operation,"enroll") && strcmp(operation,"verify") &&
        !deleting) return 2;
    uint32_t identifier=0;
    if (deleting) {
        char *end;errno=0;unsigned long value=strtoul(argv[3],&end,10);
        if (errno || *end || !value || value>UINT32_MAX || argv[3][0]=='0') return 2;
        identifier=(uint32_t)value;
    }
    if (prctl(PR_SET_DUMPABLE,0)) return 2;
    int fd=fcntl(STDOUT_FILENO,F_DUPFD_CLOEXEC,4);
    if (fd<0 || dup2(STDERR_FILENO,STDOUT_FILENO)<0) return 2;
    FILE *records=fdopen(fd,"w");
    if (!records) {close(fd);return 2;}
    signal(SIGPIPE,SIG_IGN);
    struct sigaction stop={.sa_handler=stop_worker};sigemptyset(&stop.sa_mask);
    sigaction(SIGTERM,&stop,NULL);sigaction(SIGINT,&stop,NULL);sigaction(SIGHUP,&stop,NULL);
    int status=1;
    struct result result={0};
    /* Discovery needs the bound driver and its node, no power or app. */
    if (!strcmp(operation,"probe") || !strcmp(operation,"open")) {
        status=present();
        if (!status && record(records,"UFF1 result 0 0\n")) status=1;
        if (fclose(records)) status=1;
        return status;
    }
    bool optical=!strcmp(operation,"enroll") || !strcmp(operation,"verify");
    struct device d={.control=-1};
    int ret=device_open(&d,argv[2]);
    /* The app waits loaded and the sensor unpowered until the touch. */
    if (!ret && !strcmp(operation,"verify"))
        ret=record(records,"UFW1 standby\n") || expect("activate",86400000) ? -1:0;
    if (!ret) ret=sensor_up(&d);
    if (!ret) {
        if (!strcmp(operation,"enroll")) ret=enroll(&d,records,&result);
        else if (!strcmp(operation,"verify")) ret=verify(&d,records,&result);
        else if (!strcmp(operation,"check-enroll")) ret=check_enroll(&d);
        else if (deleting) ret=delete(&d,identifier);
    }
    uint32_t ids[JIIOV_TEMPLATE_LIMIT],count=d.count;
    memcpy(ids,d.ids,sizeof(ids));
    if (device_close(&d)) ret=-1;
    if (optical && d.cleanup_ok && record(records,"UFW1 cleaned\n")) ret=-1;
    if (worker_stopping) ret=-1;
    /* Metadata only after the app is unloaded and the sensor is off. */
    if (!ret) {
        if (!strcmp(operation,"list"))
            for (uint32_t i=0;i<count;i++) if (record(records,"UFF1 item %u\n",ids[i])) ret=-1;
        if (!ret && record(records,"UFF1 result %u %u\n",result.code,result.identifier)) ret=-1;
    }
    status=ret?1:0;
    if (fclose(records)) status=1;
    return status;
}
