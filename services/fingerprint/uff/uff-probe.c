/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Native Aston UFF transport. No Android HAL or vendor userspace is loaded.
 * --transport only registers this client's own credentials and looks up TAs.
 * --identify powers the sensor and runs the factory SPI selection query only.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include "qcomtee_object.h"
#include "qcomtee_object_types.h"
#include <linux/uff_fp.h>

static int invoke(struct qcomtee_object *o, const char *what, uint32_t op,
                  struct qcomtee_param *p, int n)
{
    qcomtee_result_t result = -1;
    int ret = qcomtee_object_invoke(o, op, p, n, &result);
    fprintf(stderr, "%s: transport=%d service=%d\n", what, ret, result);
    return ret ? -1 : (int32_t)result;
}
static struct qcomtee_param input(void *p, size_t n)
{
    return (struct qcomtee_param){.attr=QCOMTEE_UBUF_INPUT,.ubuf={p,n}};
}
static struct qcomtee_param output(void *p, size_t n)
{
    return (struct qcomtee_param){.attr=QCOMTEE_UBUF_OUTPUT,.ubuf={p,n}};
}
static struct qcomtee_param object_out(void)
{
    return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_OUTPUT};
}
static struct qcomtee_param object_in(struct qcomtee_object *p)
{
    return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_INPUT,.object=p};
}
#include "uff-storage.h"

static int device_identity(void)
{
    char b[16384];
    FILE *f=fopen("/proc/bootconfig","r");
    if (!f) return -1;
    size_t n=fread(b,1,sizeof(b)-1,f);b[n]=0;fclose(f);
    return strstr(b,"androidboot.serialno = \"f683d9c9\"") &&
           strstr(b,"androidboot.slot_suffix = \"_b\"") ? 0:-1;
}
/* A complete signed split ELF is assembled locally, with segment hashes. */
static struct qcomtee_object *load_app(struct qcomtee_object *root,
                                      struct qcomtee_object *loader,
                                      const char *name, const char *path)
{
    struct qcomtee_object *memory=NULL,*app=NULL;
    FILE *f=fopen(path,"rb");
    if (!f) return NULL;
    if (fseek(f,0,SEEK_END)) goto out;
    long size=ftell(f);
    if (size<64 || size>16*1024*1024 || fseek(f,0,SEEK_SET)) goto out;
    if (qcomtee_memory_object_alloc((size_t)size,root,&memory)) goto out;
    void *p=qcomtee_memory_object_addr(memory);
    if (fread(p,1,size,f)!=(size_t)size || memcmp(p,"\177ELF\002\001",6)) goto out;
    struct qcomtee_param args[]={input((void *)name,strlen(name)),object_in(memory),object_out()};
    if (!invoke(loader,name,0,args,3)) app=args[2].object;
 out:
    fclose(f);
    if (memory) qcomtee_memory_object_release(memory);
    return app;
}
static int identify(struct qcomtee_object *root,struct qcomtee_object *app)
{
    struct qcomtee_object *memory=NULL;
    int ret=-1;
    uint32_t request[16]={0},request_out[16]={0},response[16]={0};
    uint32_t offsets[4]={4,0,0,0},is64=1;
    /* HalContext::doChipSelected: module 0, command -1, payload 0x198. */
    if (qcomtee_memory_object_alloc(4096,root,&memory)) return -1;
    uint32_t *packet=qcomtee_memory_object_addr(memory);
    memset(packet,0,4096);
    packet[1]=UINT32_MAX;packet[6]=0x198;
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC,&ts)) goto out;
    uint64_t ms=(uint64_t)ts.tv_sec*1000+ts.tv_nsec/1000000;
    memcpy((char *)packet+0x10,&ms,sizeof(ms));
    request[0]=0x198;
    struct qcomtee_param args[]={
        input(request,sizeof(request)),input(response,sizeof(response)),
        input(offsets,sizeof(offsets)),input(&is64,sizeof(is64)),
        output(request_out,sizeof(request_out)),output(response,sizeof(response)),
        object_in(memory),object_in(NULL),object_in(NULL),object_in(NULL)};
    if (invoke(app,"UFF SPI selection query",0,args,10)) goto out;
    if (args[5].ubuf.size<20 || response[4]) {
        fprintf(stderr,"UFF envelope status=%u bytes=%zu\n",response[4],args[5].ubuf.size);
        goto out;
    }
    /* The factory packet's chip count is at 0xbc and TA IDs follow it.
     * Do not accept a match or enroll result from this diagnostic query.
     */
    uint32_t count=packet[0xbc/4];
    printf("SPI selection response: packet_status=%u candidates=%u\n",packet[2],count);
    if (packet[2] || count<1 || count>6) goto out;
    for (uint32_t i=0;i<count;i++) {
        const char *sensor=(const char *)packet+0xd8+32*i;
        printf("candidate %u ta_id=%u sensor=%.32s\n",i,packet[0xc0/4+i],sensor);
    }
    ret=0;
 out:
    qcomtee_memory_object_release(memory);
    return ret;
}
#include "uff-wire.h"
/* UFF command transport shared by the fixed diagnostic operations below. */
static void (*uff_heartbeat)(void);
static int uff_command(struct qcomtee_object *root, struct qcomtee_object *app,
                       uint32_t *packet, size_t size)
{
    struct qcomtee_object *memory=NULL;
    uint32_t req[16]={0},req_out[16]={0},rsp[16]={0};
    uint32_t offsets[4]={4,0,0,0},is64=1;
    int ret=-1;
    if (!uff_wire_valid(packet,size)) return -1;
    size_t allocation=(size+4095)&~(size_t)4095;
    if (qcomtee_memory_object_alloc(allocation,root,&memory)) return -1;
    void *mapping=qcomtee_memory_object_addr(memory);
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC,&now)) goto out;
    uint64_t ms=(uint64_t)now.tv_sec*1000+now.tv_nsec/1000000;
    memcpy((char *)packet+0x10,&ms,8);packet[6]=(uint32_t)size;packet[7]=0;
    memset(mapping,0,allocation);memcpy(mapping,packet,size);req[0]=(uint32_t)size;
    struct qcomtee_param args[]={input(req,64),input(rsp,64),input(offsets,16),input(&is64,4),
        output(req_out,64),output(rsp,64),object_in(memory),object_in(NULL),object_in(NULL),object_in(NULL)};
    if (uff_heartbeat) uff_heartbeat();
    fprintf(stderr,"UFF module=%u command=%u\n",packet[0],packet[1]);
    struct timespec command_start,command_end;clock_gettime(CLOCK_MONOTONIC,&command_start);
    if (invoke(app,"UFF command",0,args,10)) goto out;
    if (args[5].ubuf.size<20) goto out;
    memcpy(packet,mapping,size);
    ret=(int32_t)rsp[4];
    clock_gettime(CLOCK_MONOTONIC,&command_end);
    if (uff_heartbeat) uff_heartbeat();
    fprintf(stderr,"UFF status=%d command=%u duration_ms=%.3f\n",ret,packet[1],(command_end.tv_sec-command_start.tv_sec)*1000.0+(command_end.tv_nsec-command_start.tv_nsec)/1000000.0);
 out:
    explicit_bzero(mapping,allocation);
    qcomtee_memory_object_release(memory);
    return ret;
}
static int inspect_config(struct qcomtee_object *root,struct qcomtee_object *app)
{
    uint32_t config[0xb0/4]={2,2100};
    /* Factory FingerprintTaConfig::init, exact project/panel/sensor metadata.
     * This sets volatile TA configuration; no template/storage commands run.
     */
    memcpy((char *)config+0x30,"23861",6);
    memcpy((char *)config+0x50,"BOE",4);
    memcpy((char *)config+0x70,"G_OPTICAL_G7s_uff",17);
    memcpy((char *)config+0x90,"unlocked",9);
    if (uff_command(root,app,config,sizeof(config))) return -1;
    const uint32_t keys[]={10,11,12,13,14,15,16,0x14,0x15,0x16,0x17};
    for (size_t i=0;i<sizeof(keys)/sizeof(keys[0]);i++) {
        uint32_t p[0x240/4]={2,2101};
        uint32_t count=i<7?32:4;
        p[0x30/4]=keys[i];p[0x238/4]=count;
        if (uff_command(root,app,p,sizeof(p))) return -1;
        if (i<7) printf("config %u: %.32s\n",keys[i],(char *)p+0x38);
        else printf("config %u: %u\n",keys[i],p[0x38/4]);
    }
    return 0;
}
static int load_factory_calibration(struct qcomtee_object *root,struct qcomtee_object *app)
{
    const char *path="/var/lib/aston-fingerprint/calibration/fingerprint_cali.bin";
    struct stat st;
    int fd=open(path,O_RDONLY|O_CLOEXEC|O_NOFOLLOW),ret=-1;
    unsigned char *data=NULL;
    if (fd<0) {perror("protected calibration");return -1;}
    if (fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_uid!=0 ||
        (st.st_mode&0077) || st.st_size<=0 || st.st_size>4*1024*1024) goto out;
    size_t size=(size_t)st.st_size,done=0;
    data=malloc(size);
    if (!data) goto out;
    while (done<size) {
        ssize_t n=read(fd,data+done,size-done);
        if (n<0 && errno==EINTR) continue;
        if (n<=0) goto out;
        done+=(size_t)n;
    }
    /* FactoryTest::transfer(..., 0x15): import signed calibration in 1 MiB
     * chunks. This never invokes the factory calibration/export command 0x14.
     */
    for (size_t offset=0;offset<size;) {
        size_t count=size-offset;
        if (count>1024*1024) count=1024*1024;
        size_t bytes=count+0x38;
        uint32_t *packet=calloc(1,bytes);
        if (!packet) goto out;
        packet[0]=5;packet[1]=0x15;
        packet[8]=(uint32_t)size;packet[9]=(uint32_t)offset;packet[12]=(uint32_t)count;
        memcpy((char *)packet+0x34,data+offset,count);
        int status=uff_command(root,app,packet,bytes);
        explicit_bzero(packet,bytes);free(packet);
        if (status) goto out;
        offset+=count;
    }
    puts("Protected factory calibration imported successfully");
    ret=0;
 out:
    if (data) {explicit_bzero(data,(size_t)st.st_size);free(data);}
    close(fd);
    return ret;
}
static int prepare_sensor(struct qcomtee_object *root,struct qcomtee_object *app)
{
    uint32_t packet[0x1c0/4]={2,2000};
    if (inspect_config(root,app)) return -1;
    /* IFingerprintCore::init: initialize sensor/algorithm, without selecting
     * an Android group, opening templates, enrolling or removing anything.
     */
    int status=uff_command(root,app,packet,sizeof(packet));
    if (status) return -1;
    printf("Sensor initialization: calibration_ready=%u algorithm=%.32s\n",
           packet[0xdc/4],(char *)packet+0xe0);
    if (!packet[0xdc/4]) return load_factory_calibration(root,app);
    return 0;
}
/* Native database namespace is deliberately unrelated to Android users/data.
 * The storage adapter must explicitly allow its filenames before any access.
 */
static int inspect_database(struct qcomtee_object *root,struct qcomtee_object *app)
{
    uint32_t group[0x100/4]={2,2002};
    group[0x30/4]=941002;
    static const char path[]="/denial-fingerprint/uff";
    memcpy((char *)group+0x34,path,sizeof(path));
    if (uff_command(root,app,group,sizeof(group))) return -1;
    uint32_t list[0x98/4]={2,2018};
    if (uff_command(root,app,list,sizeof(list))) return -1;
    uint32_t count=list[0x80/4];
    if (count>5) {fputs("Invalid template count\n",stderr);return -1;}
    printf("Native database templates=%u\n",count);
    return 0;
}
static int uff_cleanup_ok;
static int uff_power_control=-1;
typedef int (*uff_operation_fn)(struct qcomtee_object *,struct qcomtee_object *,void *);
static int uff_run_device(const char *mode,const char *firmware,
                           uff_operation_fn operation,void *opaque)
{
    struct qcomtee_object *root=NULL,*env=NULL,*loader=NULL,*app=NULL;
    uff_cleanup_ok=1;
    int ret=1,control=-1,owned=0;
    struct uff_storage storage={0};
    int database=!strcmp(mode,"--database");
    int standby=!strcmp(mode,"--standby");
    int prepare=standby || database || !strcmp(mode,"--prepare");
    int inspect=prepare || !strcmp(mode,"--inspect");
    int query=inspect || !strcmp(mode,"--identify");
    if (device_identity()) {fputs("Refusing non-Aston or non-B-slot device\n",stderr);return 1;}
    setvbuf(stdout,NULL,_IONBF,0);
    root=qcomtee_object_root_init("/dev/tee0",tee_call,NULL,NULL);
    if (!root) goto out;
    /* registerLegacy: actual effective UID and this native client's own name. */
    uint32_t uid=geteuid();
    unsigned char cred[]={0xa2,1,0x1a,uid>>24,uid>>16,uid>>8,uid,3,0x6f,
                          'a','s','t','o','n','-','u','f','f','-','p','r','o','b','e'};
    struct qcomtee_param reg[]={input(cred,sizeof(cred)),object_out()};
    if (invoke(root,"registerLegacy",1,reg,2)) goto out;
    env=reg[1].object;
    uint32_t id=122;
    struct qcomtee_param open_args[]={input(&id,4),object_out()};
    if (invoke(env,"open IQSEEComCompatAppLoader",0,open_args,2)) goto out;
    loader=open_args[1].object;
    char *names[]={"uff_spi","uff_gx"};
    for (size_t i=0;i<2;i++) {
        uint32_t arch=0;
        struct qcomtee_param args[]={input(names[i],strlen(names[i])),output(&arch,4),object_out()};
        int status=invoke(loader,names[i],2,args,3);
        if (!status) {
            printf("%s already loaded, ELF class=%u\n",names[i],arch);
            qcomtee_object_refs_dec(args[2].object);
            if (query) {fputs("Refusing to alter an existing fingerprint session\n",stderr);goto out;}
        } else if (status==23) printf("%s not loaded\n",names[i]);
        else goto out;
    }
    if (!query) {ret=0;goto out;}
    control=openat(AT_FDCWD,"/dev/fingerprint_dev",O_RDWR|O_CLOEXEC|O_NOFOLLOW);
    if (control<0) {perror("fingerprint control");goto out;}
    uff_power_control=control;
    if (!standby && ioctl(control,UFF_FP_IOC_ENABLE_POWER)) {perror("fingerprint power");goto out;}
    if (prepare && storage_start(&storage,root,env)) goto out;
    struct timespec load_start,load_end;clock_gettime(CLOCK_MONOTONIC,&load_start);
    app=load_app(root,loader,inspect?"uff_gx":"uff_spi",firmware);
    clock_gettime(CLOCK_MONOTONIC,&load_end);
    fprintf(stderr,"UFF timing load_ms=%.3f\n",(load_end.tv_sec-load_start.tv_sec)*1000.0+(load_end.tv_nsec-load_start.tv_nsec)/1000000.0);
    if (!app) goto out;
    owned=1;
    ret=standby?0:(prepare?prepare_sensor(root,app):inspect?inspect_config(root,app):identify(root,app)) ? 1:0;
    if (!ret && database) ret=inspect_database(root,app)?1:0;
    if (!ret && operation) ret=operation(root,app,opaque)?1:0;
 out:
    if (owned && invoke(app,"unload owned UFF app",2,NULL,0)) {ret=1;uff_cleanup_ok=0;}
    if (app) qcomtee_object_refs_dec(app);
    if (control>=0) {
        if (ioctl(control,UFF_FP_IOC_DISABLE_POWER)) {perror("fingerprint power off");ret=1;uff_cleanup_ok=0;}
        close(control);
        uff_power_control=-1;
    }
    storage_stop(&storage);
    if (loader) qcomtee_object_refs_dec(loader);
    if (env) qcomtee_object_refs_dec(env);
    if (root) qcomtee_object_refs_dec(root);
    return ret;
}

#ifndef UFF_PROBE_LIBRARY
int main(int argc,char **argv)
{
    int transport=argc==2&&!strcmp(argv[1],"--transport");
    int query=argc==3&&(!strcmp(argv[1],"--identify") || !strcmp(argv[1],"--inspect") ||
        !strcmp(argv[1],"--prepare") || !strcmp(argv[1],"--database"));
    if (!transport && !query) {
        fprintf(stderr,"Usage: %s --transport | --identify uff_spi.elf | --inspect uff_gx.elf | --prepare uff_gx.elf | --database uff_gx.elf\n",argv[0]);return 2;
    }
    return uff_run_device(argv[1],query?argv[2]:NULL,NULL,NULL);
}
#endif
