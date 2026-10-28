/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* UFF1/UFW1 child interface; all actual TA packets use NX809J's Goodix ABI.
 * stdout carries records only. No images, HATs or key bytes are logged. */
#define _GNU_SOURCE
#include <pthread.h>
#define GOODIX_PROBE_LIBRARY
#include "goodix-probe.c"
#include "goodix-storage.h"
#include "goodix-worker-ops.h"
#include <signal.h>
#include <poll.h>
#include <stdbool.h>
#include <limits.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/un.h>

/* A verification without a match leaves its worker warm for the next
 * attempt's bridge, which connects here; root-only directory. */
#define GOODIX_WARM_DIRECTORY "/run/nx809j-fingerprint"
#define GOODIX_WARM_SOCKET GOODIX_WARM_DIRECTORY "/verify.sock"
#define GOODIX_WARM_MS 5000

static volatile sig_atomic_t worker_stopping;
static int worker_control_fd=STDIN_FILENO;
static bool resident_active;
static void resident_notify(const char *message);
static void worker_heartbeat(void) {if (resident_active) resident_notify("WATCHDOG=1");}
static void stop_worker(int signal_number) { (void)signal_number; worker_stopping=1; }

struct worker_device {
    struct qcomtee_object *root,*env,*loader,*app,*primary;
    struct goodix_storage storage;
    struct goodix_worker operation;
    int control,hat_fd;
    uint32_t sequence;
    bool powered,cleanup_ok;
};

static int64_t milliseconds(void)
{
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC,&now)) return -1;
    return (int64_t)now.tv_sec*1000+now.tv_nsec/1000000;
}

static int worker_exchange(void *opaque,uint32_t *packet,size_t size)
{
    struct worker_device *d=opaque;
    int64_t started=milliseconds();
    worker_heartbeat();
    if (!goodix_wire_valid(packet,size) || size>2u*1024*1024) return -EPROTO;
    uint32_t request[16]={0},request_out[16]={0},response[16]={0};
    uint32_t offsets[4]={0},is64=1;
    void *mapping=qcomtee_memory_object_addr(d->primary);
    memset(mapping,0,2u*1024*1024);memcpy(mapping,packet,size);
    request[2]=size;request[3]=++d->sequence;
    time_t now=time(NULL);struct tm date;
    if (!localtime_r(&now,&date)) {explicit_bzero(mapping,2u*1024*1024);return -EIO;}
    uint64_t ms=(uint64_t)(now+date.tm_gmtoff/3600*3600)*1000;
    memcpy(request+4,&ms,8);
    struct qcomtee_param p[]={in(request,64),in(response,64),in(offsets,16),in(&is64,4),
        out(request_out,64),out(response,64),obj(d->primary),obj(NULL),obj(NULL),obj(NULL)};
    int ret=invoke(d->app,"Goodix worker command",0,p,10);
    if (!ret && p[5].ubuf.size!=64) ret=-EPROTO;
    if (!ret) {
        memcpy(packet,mapping,size);
        ret=response[7]?(int32_t)response[7]:(int32_t)packet[3];
    }
    int64_t finished=milliseconds();
    fprintf(stderr,"Goodix worker target=%u command=%u status=%d duration_ms=%lld\n",packet[1],packet[2],ret,
        (long long)(started>=0 && finished>=started ? finished-started : -1));
    explicit_bzero(mapping,2u*1024*1024);
    explicit_bzero(request_out,sizeof(request_out));
    return ret;
}

static int worker_control(void *opaque,char *line,size_t capacity,int timeout_ms)
{
    struct worker_device *d=opaque;
    int64_t now=milliseconds(),deadline=now+timeout_ms;
    size_t used=0;
    if (now<0) return -EIO;
    while (!worker_stopping && used+1<capacity) {
        now=milliseconds();if (now<0) return -EIO;
        if (now>=deadline) return -ETIMEDOUT;
        int interval=(int)(deadline-now);if (interval>1000) interval=1000;
        struct pollfd p[2]={{.fd=worker_control_fd,.events=POLLIN},
            {.fd=d && d->operation.initialized?d->control:-1,.events=POLLIN}};
        worker_heartbeat();
        int ready=poll(p,2,interval);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0) return -errno;
        if (p[0].revents&(POLLERR|POLLNVAL)) return -EIO;
        if (p[0].revents&(POLLIN|POLLHUP)) {
            char c;ssize_t n=read(worker_control_fd,&c,1);
            if (n<0 && errno==EINTR) continue;
            if (n!=1) return -ECANCELED;
            if (c=='\n') {line[used]=0;return 0;}
            if (c<' ' || c>'~') return -EPROTO;
            line[used++]=c;
        }
        if (p[1].revents&(POLLERR|POLLNVAL)) return -EIO;
        if (p[1].revents&POLLIN) {
            uint32_t sequence;
            ssize_t n=read(d->control,&sequence,sizeof(sequence));
            if (n<0 && (errno==EAGAIN || errno==EINTR)) continue;
            if (n!=sizeof(sequence)) return -EIO;
            int ret=goodix_worker_irq(&d->operation);
            if (ret) return ret;
        }
    }
    return worker_stopping?-ECANCELED:-EMSGSIZE;
}

static int worker_hat(void *opaque,uint8_t token[GOODIX_HAT_SIZE])
{
    struct worker_device *d=opaque;
    int fd=d->hat_fd;
    if (fd<0) {
        char line[16];int ret=worker_control(opaque,line,sizeof(line),30000);
        if (ret) return ret;
        if (strcmp(line,"hat")) return -ENOKEY;
        fd=worker_control_fd;
    }
    int64_t deadline=milliseconds()+30000;
    size_t used=0;
    while (used<GOODIX_HAT_SIZE && !worker_stopping) {
        int64_t remaining=deadline-milliseconds();
        if (remaining<=0) return -ETIMEDOUT;
        struct pollfd p={.fd=fd,.events=POLLIN};
        worker_heartbeat();
        int ready=poll(&p,1,remaining>1000?1000:(int)remaining);
        if (ready<0 && errno==EINTR) continue;
        if (ready<0 || p.revents&(POLLERR|POLLNVAL)) return -EIO;
        if (!ready) continue;
        ssize_t n=read(fd,token+used,GOODIX_HAT_SIZE-used);
        if (n<0 && errno==EINTR) continue;
        if (n<=0) return -ENOKEY;
        used+=(size_t)n;
    }
    return worker_stopping?-ECANCELED:0;
}

#include "goodix-authorization.h"

static int worker_cleanup(struct worker_device *d)
{
    int status=0;
    if (d->app) {
        if (d->powered && d->operation.initialized && goodix_worker_sleep(&d->operation)) status=1;
        if (invoke(d->app,"unload owned goodixfp64",2,NULL,0)) status=1;
        qcomtee_object_refs_dec(d->app);d->app=NULL;
    }
    if (d->control>=0) {
        if (d->powered && ioctl(d->control,GOODIX_FP_IOC_DISABLE_POWER)) status=1;
        struct goodix_fp_state state;
        if (ioctl(d->control,GOODIX_FP_IOC_GET_STATE,&state) || state.version!=1 || state.powered) status=1;
        close(d->control);d->control=-1;d->powered=false;
    }
    /* Unload the app before unregistering the file services it owns. */
    storage_stop(&d->storage);
    if (d->primary) {
        explicit_bzero(qcomtee_memory_object_addr(d->primary),2u*1024*1024);
        qcomtee_memory_object_release(d->primary);d->primary=NULL;
    }
    if (d->loader) {qcomtee_object_refs_dec(d->loader);d->loader=NULL;}
    if (d->env) {qcomtee_object_refs_dec(d->env);d->env=NULL;}
    if (d->root) {qcomtee_object_refs_dec(d->root);d->root=NULL;}
    d->cleanup_ok=!status;
    return status;
}

/* Load the trusted app and its listeners before a touch, while the sensor
 * remains powered off. A cancelled standby must still run worker_cleanup. */
static int worker_preload(struct worker_device *d,const char *firmware)
{
    int64_t start=milliseconds(), storage_ready, app_ready;
    d->root=qcomtee_object_root_init("/dev/tee0",tee_call,NULL,NULL);
    if (!d->root) return -ENODEV;
    uint32_t uid=geteuid();
    unsigned char credentials[]={0xa2,1,0x1a,uid>>24,uid>>16,uid>>8,uid,3,0x6d,
        'g','o','o','d','i','x','-','w','o','r','k','e','r'};
    struct qcomtee_param reg[]={in(credentials,sizeof(credentials)),newobj()};
    if (invoke(d->root,"registerLegacy",1,reg,2)) return -EACCES;
    d->env=reg[1].object;
    uint32_t loader_id=122;
    struct qcomtee_param open_args[]={in(&loader_id,4),newobj()};
    if (invoke(d->env,"open IQSEEComCompatAppLoader",0,open_args,2)) return -EIO;
    d->loader=open_args[1].object;
    /* A warm worker that just timed out may still be unloading the app. */
    for (int tries=0;;tries++) {
        char name[]="goodixfp64";uint32_t arch=0;
        struct qcomtee_param find[]={in(name,sizeof(name)-1),out(&arch,4),newobj()};
        int present=invoke(d->loader,"lookup goodixfp64",2,find,3);
        if (present==23) break;
        if (present) return -EIO;
        qcomtee_object_refs_dec(find[2].object);
        if (tries==20 || worker_stopping) return -EBUSY;
        usleep(50000);
    }
    d->control=open("/dev/goodix_fp",O_RDWR|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
    struct stat node;struct goodix_fp_state state;
    if (d->control<0 || fstat(d->control,&node) || !S_ISCHR(node.st_mode) || node.st_uid ||
        (node.st_mode&077) || ioctl(d->control,GOODIX_FP_IOC_GET_STATE,&state) || state.version!=1 || state.powered)
        return -EACCES;
    const char *calibration=getenv("GOODIX_CALIBRATION_DIR"),*store=getenv("GOODIX_STORE_DIR");
    if (!calibration) calibration="/var/lib/nx809j-fingerprint/calibration";
    if (!store) store="/var/lib/nx809j-fingerprint/templates";
    if (storage_start(&d->storage,d->root,d->env,calibration,store)) return -EIO;
    storage_ready=milliseconds();
    if (qcomtee_memory_object_alloc(2u*1024*1024,d->root,&d->primary)) return -ENOMEM;
    d->app=load(d->root,d->loader,firmware);
    if (!d->app || authentication_setup(d->root,d->loader,d->app)) return -EIO;
    app_ready=milliseconds();
    if (start>=0 && storage_ready>=start && app_ready>=storage_ready)
        fprintf(stderr,"Goodix preload timing ms storage=%lld app_and_key=%lld total=%lld\n",
            (long long)(storage_ready-start),(long long)(app_ready-storage_ready),(long long)(app_ready-start));
    return worker_stopping ? -ECANCELED : 0;
}

static int worker_activate(struct worker_device *d)
{
    int64_t start=milliseconds(), power_ready, prepared;
    struct goodix_fp_state state;
    if (worker_stopping) return -ECANCELED;
    if (ioctl(d->control,GOODIX_FP_IOC_ENABLE_POWER)) return -EIO;
    d->powered=true;
    if (ioctl(d->control,GOODIX_FP_IOC_GET_STATE,&state) || !state.powered || state.version!=1) return -EIO;
    power_ready=milliseconds();
    int ret=goodix_worker_prepare(&d->operation);
    prepared=milliseconds();
    if (start>=0 && power_ready>=start && prepared>=power_ready)
        fprintf(stderr,"Goodix activation timing ms power=%lld initialize=%lld total=%lld\n",
            (long long)(power_ready-start),(long long)(prepared-power_ready),(long long)(prepared-start));
    fprintf(stderr,"Goodix init status=%d calibration=%u ui_ready=%u slide_enroll=%u\n",ret,
            d->operation.calibration_state,d->operation.ui_ready_required,d->operation.slide_enroll);
    return ret;
}

static bool warm_directory(void)
{
    struct stat directory;
    return !lstat(GOODIX_WARM_DIRECTORY,&directory) && S_ISDIR(directory.st_mode) &&
        !directory.st_uid && !(directory.st_mode&077);
}

/* A warm worker between verification attempts keeps the sensor powered. */
static bool warm_present(void)
{
    struct stat node;
    return warm_directory() && !lstat(GOODIX_WARM_SOCKET,&node) && S_ISSOCK(node.st_mode) && !node.st_uid;
}

static int worker_passive(void)
{
    if (identity()) return 1;
    int fd=open("/dev/goodix_fp",O_RDWR|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK);
    struct stat node;struct goodix_fp_state state;
    int ret=fd<0;
    if (!ret) ret=fstat(fd,&node) || !S_ISCHR(node.st_mode) || node.st_uid ||
        (node.st_mode&077) || ioctl(fd,GOODIX_FP_IOC_GET_STATE,&state) || state.version!=1 ||
        (state.powered && !warm_present());
    if (fd>=0) close(fd);
    return ret?1:0;
}

/* Keep the sensor initialized and asleep, serving its interrupts, until the
 * next attempt's bridge connects; it then carries the records and control. */
static int worker_rest(struct worker_device *d,FILE *records)
{
    if (!warm_directory()) return -EACCES;
    int listener=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC|SOCK_NONBLOCK,0),bridge=-1,ret=0;
    if (listener<0) return -errno;
    struct sockaddr_un address={.sun_family=AF_UNIX};
    memcpy(address.sun_path,GOODIX_WARM_SOCKET,sizeof(GOODIX_WARM_SOCKET));
    unlink(GOODIX_WARM_SOCKET);
    mode_t mask=umask(077);
    if (bind(listener,(struct sockaddr *)&address,sizeof(address)) || listen(listener,1)) ret=-errno;
    umask(mask);
    if (!ret && (fprintf(records,"UFW1 resting\n")<0 || fflush(records))) ret=-EPIPE;
    int64_t deadline=milliseconds()+GOODIX_WARM_MS;
    while (!ret && bridge<0) {
        int64_t now=milliseconds();
        if (worker_stopping) ret=-ECANCELED;
        else if (now<0 || now>=deadline) ret=-ETIMEDOUT;
        if (ret) break;
        struct pollfd p[2]={{.fd=listener,.events=POLLIN},{.fd=d->control,.events=POLLIN}};
        int ready=poll(p,2,deadline-now>1000?1000:(int)(deadline-now));
        if (ready<0 && errno==EINTR) continue;
        if (ready<0 || p[1].revents&(POLLERR|POLLNVAL)) {ret=ready<0?-errno:-EIO;break;}
        if (p[1].revents&POLLIN) {
            uint32_t sequence;
            ssize_t n=read(d->control,&sequence,sizeof(sequence));
            if (n<0 && (errno==EAGAIN || errno==EINTR)) continue;
            ret=n!=sizeof(sequence)?-EIO:goodix_worker_irq(&d->operation);
        }
        if (!ret && p[0].revents&POLLIN) {
            bridge=accept4(listener,NULL,NULL,SOCK_CLOEXEC);
            struct ucred peer;socklen_t size=sizeof(peer);
            if (bridge>=0 && (getsockopt(bridge,SOL_SOCKET,SO_PEERCRED,&peer,&size) ||
                size!=sizeof(peer) || peer.uid)) {close(bridge);bridge=-1;}
        }
    }
    /* No second bridge can reach a worker that is leaving or adopted. */
    unlink(GOODIX_WARM_SOCKET);
    close(listener);
    if (!ret && (dup2(bridge,fileno(records))<0 || dup2(bridge,worker_control_fd)<0)) ret=-errno;
    if (bridge>=0) close(bridge);
    clearerr(records);
    if (!ret) fprintf(stderr,"Goodix worker warm for the next attempt after %lld ms\n",
        (long long)(milliseconds()-deadline+GOODIX_WARM_MS));
    return ret;
}

static int worker_run(const char *firmware,const char *operation,uint32_t identifier,FILE *records)
{
    bool verify=!strcmp(operation,"verify"),enroll=!strcmp(operation,"enroll");
    struct worker_device d={.control=-1,.hat_fd=-1};
    const char *hat_fd=getenv("GOODIX_HAT_FD");
    if (hat_fd) {
        char *end;errno=0;long fd=strtol(hat_fd,&end,10);
        if (errno || *end || fd<3 || fd>INT_MAX || fcntl(fd,F_GETFD)<0) return 2;
        d.hat_fd=(int)fd;
    }
    d.operation=(struct goodix_worker){.context=&d,.exchange=worker_exchange,
        .control=worker_control,.hat=worker_hat,.records=records};
    if ((enroll || !strcmp(operation,"check-enroll")) && !hat_fd && authorization_available())
        d.operation.authorize=worker_authorize;
    int status=worker_preload(&d,firmware);
    /* Verification brings the sensor up and selects the templates while
     * armed, screen on or off; the sensor then sleeps until a touch, and
     * only the capture and comparison follow it. Interrupts are served
     * while it waits. */
    if (!status) status=worker_activate(&d);
    if (!status && verify) status=goodix_worker_database(&d.operation);
    /* An attempt without a match rests and serves the next one; a match,
     * a cancellation or a failure ends here. */
    while (!status && verify) {
        if (fprintf(records,"UFW1 standby\n")<0 || fflush(records)) status=-EPIPE;
        if (!status) {
            char command[16];
            status=worker_control(&d,command,sizeof(command),86400000);
            if (!status && strcmp(command,"activate")) status=-ECANCELED;
        }
        if (!status) status=worker_stopping?-ECANCELED:goodix_worker_operate(&d.operation,operation,identifier);
        if (status!=-ECANCELED || !d.operation.resting || worker_stopping) break;
        status=worker_rest(&d,records);
        /* The rest is spent: a bridge that adopts the worker and then ends
         * its stream, without an attempt of its own, ends the worker. */
        d.operation.resting=false;
        if (status) fprintf(stderr,"Goodix worker rest ended status=%d\n",status);
    }
    if (!status && !verify) status=worker_stopping?-ECANCELED:goodix_worker_operate(&d.operation,operation,identifier);
    int cleaned=worker_cleanup(&d);
    fprintf(stderr,"Goodix worker operation=%s status=%d cleanup=%d\n",operation,status,cleaned);
    if ((verify || enroll) && d.cleanup_ok) {
        if (fprintf(records,"UFW1 cleaned\n")<0 || fflush(records)) status=-EPIPE;
    }
    if (cleaned || worker_stopping) status=-EIO;
    if (!status) {
        if (!strcmp(operation,"list")) for (uint32_t i=0;i<d.operation.count;i++) fprintf(records,"UFF1 item %u\n",d.operation.ids[i]);
        if (fprintf(records,"UFF1 result %u %u\n",d.operation.code,d.operation.identifier)<0 || fflush(records)) status=-EPIPE;
    }
    return status?1:0;
}

#include "goodix-resident.h"

int main(int argc,char **argv)
{
    bool deleting=argc==4 && !strcmp(argv[1],"delete"),serving=argc==3 && !strcmp(argv[1],"serve");
    if (geteuid()!=0 || (argc!=3 && !deleting)) return 2;
    const char *operation=argv[1];
    if (strcmp(operation,"probe") && strcmp(operation,"open") && strcmp(operation,"list") &&
        strcmp(operation,"check-enroll") && strcmp(operation,"enroll") && strcmp(operation,"verify") &&
        strcmp(operation,"delete") && !serving) return 2;
    if (!deleting && !strcmp(operation,"delete")) return 2;
    if (identity() || prctl(PR_SET_DUMPABLE,0)) return 2;
    int firmware=open(argv[2],O_RDONLY|O_CLOEXEC|O_NOFOLLOW);
    struct stat st;
    if (firmware<0) return 2;
    int invalid=fstat(firmware,&st) || !S_ISREG(st.st_mode) || st.st_uid ||
        (st.st_mode&022) || st.st_size!=14180440;
    close(firmware);if (invalid) return 2;
    uint32_t identifier=0;
    if (deleting) {
        char *end;errno=0;unsigned long value=strtoul(argv[3],&end,10);
        if (errno || *end || !value || value>UINT32_MAX || argv[3][0]=='0') return 2;
        identifier=(uint32_t)value;
    }
    int fd=fcntl(STDOUT_FILENO,F_DUPFD_CLOEXEC,4);
    if (fd<0 || dup2(STDERR_FILENO,STDOUT_FILENO)<0) return 2;
    FILE *records=fdopen(fd,"w");if (!records) {close(fd);return 2;}
    signal(SIGPIPE,SIG_IGN);
    struct sigaction stop={.sa_handler=stop_worker};sigemptyset(&stop.sa_mask);
    sigaction(SIGTERM,&stop,NULL);sigaction(SIGINT,&stop,NULL);sigaction(SIGHUP,&stop,NULL);
    int status;
    if (serving) status=resident_serve(argv[2]);
    else if (getenv("GOODIX_RESIDENT")) status=resident_client(argc,argv,records);
    else if (!strcmp(operation,"probe") || !strcmp(operation,"open")) {
        status=worker_passive();
        if (!status && (fprintf(records,"UFF1 result 0 0\n")<0 || fflush(records))) status=1;
    } else status=worker_run(argv[2],operation,identifier,records);
    if (fclose(records)) status=1;
    return status;
}
