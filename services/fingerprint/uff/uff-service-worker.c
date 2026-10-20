/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Blocking secure calls stay in this child process. Only metadata is returned
 * over UFF1; diagnostic output is confined to stderr. No Android HAL is loaded.
 */
#define UFF_PROBE_LIBRARY
#include "uff-probe.c"
#include <signal.h>
#include <poll.h>
#include <stdbool.h>

struct uff_result {uint32_t ids[5],count,identifier,code,total;FILE *records;const char *operation;};
static volatile sig_atomic_t worker_stopping;
static int worker_control_fd=STDIN_FILENO;
static void stop_worker(int sig) {(void)sig;worker_stopping=1;}
static int uff_service_database(struct qcomtee_object *root,struct qcomtee_object *app,
                                 struct uff_result *result)
{
    uint32_t group[0x100/4]={2,2002};
    group[0x30/4]=941002;
    memcpy((char *)group+0x34,"/denial-fingerprint/uff",sizeof("/denial-fingerprint/uff"));
    if (uff_command(root,app,group,sizeof(group))) return -1;
    uint32_t list[0x98/4]={2,2018};
    if (uff_command(root,app,list,sizeof(list))) return -1;
    result->count=list[0x80/4];
    if (result->count>5) return -1;
    for (uint32_t i=0;i<result->count;i++) {
        uint32_t id=list[0x84/4+i];
        if (!id) return -1;
        for (uint32_t j=0;j<i;j++) if (result->ids[j]==id) return -1;
        result->ids[i]=id;
    }
    return 0;
}
static int uff_list_operation(struct qcomtee_object *root,struct qcomtee_object *app,void *opaque)
{return uff_service_database(root,app,opaque);}

/* Non-optical enrollment setup check. It uses only this process's freshly
 * requested TA challenge and the isolated native group. No Android credential
 * is reused, no finger image is acquired and no template is saved.
 */
static int uff_enroll_setup_check(struct qcomtee_object *root,struct qcomtee_object *app,void *opaque)
{
    struct uff_result *r=opaque;
    if (uff_service_database(root,app,r) || r->count) return -1;
    uint32_t pre[0x80/4]={2,2005};
    if (uff_command(root,app,pre,sizeof(pre))) return -1;
    uint64_t challenge;
    memcpy(&challenge,(char *)pre+0x78,8);
    if (!challenge) return -1;
    uint32_t start[0xc8/4]={2,2006};
    start[0x30/4]=941002;start[0x7c/4]=60;
    /* Packed hw_auth_token-shaped native namespace metadata. Root fprintd
     * authorizes enrollment on Linux; firmware enforcement is never disabled
     * or patched. Unsupported token policy must fail, not claim readiness.
     */
    memcpy((char *)start+0x36,&challenge,8);
    uint64_t native_user=941002;
    memcpy((char *)start+0x3e,&native_user,8);
    int status=uff_command(root,app,start,sizeof(start));
    explicit_bzero(pre,sizeof(pre));explicit_bzero(start,sizeof(start));
    fprintf(stderr,"Native enrollment setup status=%d (no capture)\n",status);
    uint32_t post[0x78/4]={2,2010};
    int cleanup=uff_command(root,app,post,sizeof(post));
    if (status || cleanup) return -1;
    return uff_service_database(root,app,r) || r->count ? -1:0;
}

static int uff_simple(struct qcomtee_object *root,struct qcomtee_object *app,
                       uint32_t command,size_t bytes)
{
    uint32_t p[0x88/4]={2,command};
    if (bytes>sizeof(p)) return -1;
    return uff_command(root,app,p,bytes);
}
static int worker_control(char command[16],int timeout_ms)
{
    size_t n=0;
    while (!worker_stopping && n<15) {
        struct pollfd fd={.fd=worker_control_fd,.events=POLLIN};
        if (uff_heartbeat) uff_heartbeat();
        int rc=poll(&fd,1,timeout_ms>1000?1000:timeout_ms);
        if (!rc && timeout_ms>1000) {timeout_ms-=1000;continue;}
        if (rc<0 && errno==EINTR) continue;
        if (rc<=0 || fd.revents&(POLLERR|POLLNVAL)) return -1;
        char c;
        ssize_t bytes=read(worker_control_fd,&c,1);
        if (bytes<0 && errno==EINTR) continue;
        if (bytes!=1 || (c<' ' && c!='\n')) return -1;
        if (c=='\n') {command[n]=0;return 0;}
        command[n++]=c;
    }
    return -1;
}
static int uff_begin_enrollment(struct qcomtee_object *root,struct qcomtee_object *app)
{
    uint32_t pre[0x80/4]={2,2005},start[0xc8/4]={2,2006};
    if (uff_command(root,app,pre,sizeof(pre))) return -1;
    uint64_t challenge,native_user=941002;
    memcpy(&challenge,(char *)pre+0x78,8);
    if (!challenge) return -1;
    start[0x30/4]=941002;start[0x7c/4]=120;
    memcpy((char *)start+0x36,&challenge,8);
    memcpy((char *)start+0x3e,&native_user,8);
    int status=uff_command(root,app,start,sizeof(start));
    explicit_bzero(pre,sizeof(pre));explicit_bzero(start,sizeof(start));
    return status;
}
static int uff_acquire(struct qcomtee_object *root,struct qcomtee_object *app,int mode)
{
    uint32_t capture[0xb8/4]={2,2004},feature[0xa0/4]={2,2003};
    capture[0x34/4]=mode;capture[0x38/4]=1;
    struct tm date;time_t now=time(NULL);
    if (!localtime_r(&now,&date)) return -1;
    capture[0x4c/4]=(date.tm_year<<16)|(date.tm_mon<<8)|date.tm_mday;
    if (uff_command(root,app,capture,sizeof(capture))) return -1;
    uint32_t kind=capture[0x6c/4];
    fprintf(stderr,"UFF acquisition kind=%u\n",kind);
    if (kind!=0 && kind!=5100) return 3;
    feature[0x34/4]=mode;
    if (uff_command(root,app,feature,sizeof(feature))) return -1;
    fprintf(stderr,"UFF feature kind=%u\n",feature[0x58/4]);
    return feature[0x58/4]?3:0;
}
static int uff_operate(struct qcomtee_object *root,struct qcomtee_object *app,void *opaque)
{
    struct uff_result *r=opaque;
    if (uff_service_database(root,app,r)) return -1;
    int enroll=!strcmp(r->operation,"enroll");
    int verify=!strcmp(r->operation,"verify");
    if (!enroll && !verify) {
        if (strcmp(r->operation,"delete")) return -1;
        uint32_t before=r->count;
        bool found=false;
        for (uint32_t i=0;i<before;i++) found|=r->ids[i]==r->identifier;
        if (!found) return -1;
        uint32_t remove[0x80/4]={2,2017};
        remove[0x30/4]=941002;remove[0x34/4]=r->identifier;
        if (uff_command(root,app,remove,sizeof(remove)) || uff_service_database(root,app,r) || r->count+1!=before) return -1;
        for (uint32_t i=0;i<r->count;i++) if (r->ids[i]==r->identifier) return -1;
        r->identifier=0;return 0;
    }
    uint32_t old_ids[5];memcpy(old_ids,r->ids,sizeof(old_ids));
    uint32_t old_count=r->count;
    if ((enroll && old_count>=5) || (verify && !old_count)) return -1;
    int ret=-1;
    if (enroll) {if (uff_begin_enrollment(root,app)) goto cleanup;}
    else {
        uint32_t auth[0x88/4]={2,2012};auth[0x30/4]=941002;
        if (uff_command(root,app,auth,sizeof(auth))) goto cleanup;
    }
    fprintf(r->records,"UFW1 prepared\n");fflush(r->records);
    uint32_t last_remaining=100;
    for (unsigned attempts=0;attempts<100 && !worker_stopping;attempts++) {
        char command[16];
        if (worker_control(command,120000) || strcmp(command,"capture")) goto cleanup;
        if (enroll) {
            uint32_t down[0x90/4]={2,2007};down[0x44/4]=1000;
            if (uff_command(root,app,down,sizeof(down))) goto cleanup;
        } else {
            uint32_t start[0xa0/4]={2,2013};start[0x44/4]=1;start[0x48/4]=1000;
            if (uff_command(root,app,start,sizeof(start))) goto cleanup;
        }
        int acquired=uff_acquire(root,app,enroll?2:3);
        if (acquired<0 || worker_stopping) goto cleanup;
        uint32_t remaining=last_remaining,id=0,code=3;
        if (!acquired && enroll) {
            uint32_t image[0xb0/4]={2,2008};
            if (uff_command(root,app,image,sizeof(image))) goto cleanup;
            if (!image[0x54/4]) {
                remaining=image[0x8c/4];id=image[0x84/4];
                if (remaining>last_remaining || remaining>100) goto cleanup;
                code=0;
                if (!r->total) r->total=remaining+1;
            }
        } else if (!acquired) {
            uint32_t compare[0xf0/4]={2,2014};
            if (uff_command(root,app,compare,sizeof(compare))) goto cleanup;
            uint32_t kind=compare[0x54/4];id=compare[0x80/4];
            fprintf(stderr,"UFF comparison kind=%u candidate_present=%u\n",kind,id!=0);
            if (!kind && id) {
                bool found=false;
                for (uint32_t i=0;i<old_count;i++) found|=old_ids[i]==id;
                if (!found) goto cleanup;
                code=0;
            } else if (kind==5300 || kind==5301) {code=1;id=0;}
            else {code=3;id=0;}
        }
        fprintf(r->records,"UFW1 sample %u %u %u %u\n",code,remaining,id,r->total);
        if (fflush(r->records) || worker_control(command,120000) || strcmp(command,"finish")) goto cleanup;
        /* Parent sends finish only after scanout-ready capture completed,
         * no early UP was observed, and compositor restoration was confirmed.
         * Incomplete contacts can never commit a template or report a match.
         */
        if (enroll) {
            if (!code && !remaining) {
                if (!id) goto cleanup;
                for (uint32_t i=0;i<old_count;i++) if (old_ids[i]==id) goto cleanup;
                if (uff_simple(root,app,2009,0x80) || uff_simple(root,app,2102,0x30)) goto cleanup;
                r->identifier=id;r->code=0;
                if (uff_service_database(root,app,r) || r->count!=old_count+1) goto cleanup;
                bool found=false;for (uint32_t i=0;i<r->count;i++) found|=r->ids[i]==id;
                if (!found) goto cleanup;
                ret=0;break;
            }
            if (uff_simple(root,app,2102,0x30)) goto cleanup;
            last_remaining=remaining;
            fprintf(r->records,"UFW1 idle\n");
            if (fflush(r->records)) goto cleanup;
        } else {
            r->code=code;r->identifier=id;ret=0;break;
        }
    }
 cleanup:
    if (uff_simple(root,app,enroll?2010:2016,0x78)) ret=-1;
    if (uff_simple(root,app,2019,0x30)) ret=-1;
    return worker_stopping?-1:ret;
}

static int uff_standby_verify(struct qcomtee_object *root,struct qcomtee_object *app,void *opaque)
{
    struct uff_result *r=opaque;char command[16];
    /* Exact signed fp_init ends with matcher[2] -> gf_uff_sensor_sleep.
     * Preserve that sleeping sensor's rail/reset state; the kernel gates its
     * IRQ and permits suspend only after this explicit quiescent transition.
     */
    uint32_t standby=1;
    struct uff_fp_state before,after;
    if (uff_power_control<0 || ioctl(uff_power_control,UFF_FP_IOC_ENABLE_POWER) ||
        prepare_sensor(root,app) || uff_service_database(root,app,r) ||
        uff_simple(root,app,2019,0x30) ||
        ioctl(uff_power_control,UFF_FP_IOC_SET_STANDBY,&standby) ||
        ioctl(uff_power_control,UFF_FP_IOC_GET_STATE,&before) ||
        before.version!=1 || before.powered!=1 || before.standby!=1) return -1;
    fprintf(r->records,"UFW1 standby\n");
    if (fflush(r->records) || worker_control(command,86400000) || strcmp(command,"activate") || worker_stopping) return -1;
    struct timespec start,end;clock_gettime(CLOCK_MONOTONIC,&start);
    standby=0;
    if (ioctl(uff_power_control,UFF_FP_IOC_SET_STANDBY,&standby) ||
        ioctl(uff_power_control,UFF_FP_IOC_GET_STATE,&after) || after.version!=1 ||
        after.powered!=1 || after.standby ||
        after.reset_sequence!=before.reset_sequence) return -1;
    clock_gettime(CLOCK_MONOTONIC,&end);
    fprintf(stderr,"UFF timing activate_ms=%.3f\n",(end.tv_sec-start.tv_sec)*1000.0+(end.tv_nsec-start.tv_nsec)/1000000.0);
    fprintf(stderr,"UFF retained reset_sequence=%u\n",after.reset_sequence);
    return uff_operate(root,app,opaque);
}

#include "uff-resident.h"

int main(int argc,char **argv)
{
    int serving=argc==3&&!strcmp(argv[1],"serve");
    int deleting=argc==4&&!strcmp(argv[1],"delete");
    if (geteuid()!=0 || (!deleting && argc!=3) || (!serving && strcmp(argv[1],"probe") && strcmp(argv[1],"open") && strcmp(argv[1],"list") && strcmp(argv[1],"check-enroll") && strcmp(argv[1],"enroll") && strcmp(argv[1],"verify") && strcmp(argv[1],"delete"))) return 2;
    if (!deleting && !strcmp(argv[1],"delete")) return 2;
    struct stat st;
    int firmware=open(argv[2],O_RDONLY|O_CLOEXEC|O_NOFOLLOW);
    if (firmware<0 || fstat(firmware,&st) || !S_ISREG(st.st_mode) || st.st_uid!=0 ||
        (st.st_mode&022) || st.st_size!=14426200) {if(firmware>=0)close(firmware);return 2;}
    close(firmware);
    int records=fcntl(STDOUT_FILENO,F_DUPFD_CLOEXEC,4);
    if (records<0 || dup2(STDERR_FILENO,STDOUT_FILENO)<0) return 2;
    FILE *output=fdopen(records,"w");
    if (!output) {close(records);return 2;}
    signal(SIGPIPE,SIG_IGN);
    struct sigaction stop={.sa_handler=stop_worker};sigemptyset(&stop.sa_mask);
    sigaction(SIGTERM,&stop,NULL);sigaction(SIGINT,&stop,NULL);sigaction(SIGHUP,&stop,NULL);
    if (serving) {
        int status=uff_run_device("--standby",argv[2],uff_resident_serve,NULL);
        fclose(output);return status;
    }
    if (getenv("UFF_RESIDENT")) {
        int status=uff_resident_client(argc,argv,output);
        if (fclose(output)) status=1;
        return status;
    }
    struct uff_result result={.records=output,.operation=argv[1]};
    if (deleting) {
        char *end;errno=0;unsigned long id=strtoul(argv[3],&end,10);
        if (errno || *end || !id || id>UINT32_MAX) {fclose(output);return 2;}
        result.identifier=(uint32_t)id;
    }
    if (!strcmp(argv[1],"probe") || !strcmp(argv[1],"open")) {
        /* Logical discovery/open needs the validated board/control ABI, not
         * repeated algorithm initialization. No power or TA command is sent.
         */
        int fd=open("/dev/fingerprint_dev",O_RDWR|O_CLOEXEC|O_NOFOLLOW);
        struct stat node;int status=device_identity() || fd<0;
        if (!status) status=fstat(fd,&node) || !S_ISCHR(node.st_mode) || node.st_uid!=0 || (node.st_mode&077);
        if (fd>=0) close(fd);
        if (!status) fprintf(output,"UFF1 result 0 0\n");
        if (fclose(output)) status=1;
        return status?1:0;
    }
    int ret=uff_run_device(!strcmp(argv[1],"verify")?"--standby":"--prepare",argv[2],!strcmp(argv[1],"verify")?uff_standby_verify:!strcmp(argv[1],"check-enroll")?uff_enroll_setup_check:
        (!strcmp(argv[1],"enroll") || !strcmp(argv[1],"verify") || deleting)?uff_operate:uff_list_operation,&result);
    /* Successful metadata is emitted only after secure-app unload and power
     * cleanup both succeeded. A failed setup never becomes an empty list.
     */
    if (!strcmp(argv[1],"enroll") || !strcmp(argv[1],"verify")) {
        if (uff_cleanup_ok) fprintf(output,"UFW1 cleaned\n");
        if (worker_stopping) ret=1;
    }
    if (!ret) {
        if (!strcmp(argv[1],"list"))
            for (uint32_t i=0;i<result.count;i++) fprintf(output,"UFF1 item %u\n",result.ids[i]);
        fprintf(output,"UFF1 result %u %u\n",result.code,result.identifier);
    }
    if (fclose(output)) ret=1;
    return ret;
}
