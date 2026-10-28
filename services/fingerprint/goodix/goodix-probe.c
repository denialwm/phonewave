/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Native NX809J Goodix diagnostics. No Android libraries are loaded.
 * These operations provide no filesystem callbacks or template commands.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/goodix_tee_fp.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include "qcomtee_object.h"
#include "qcomtee_object_types.h"
#include "keymaster-shared-secret.h"
#include "keymaster-wrapped-key.h"

static struct qcomtee_param in(void *p, size_t n)
{ return (struct qcomtee_param){.attr=QCOMTEE_UBUF_INPUT,.ubuf={p,n}}; }
static struct qcomtee_param out(void *p, size_t n)
{ return (struct qcomtee_param){.attr=QCOMTEE_UBUF_OUTPUT,.ubuf={p,n}}; }
static struct qcomtee_param obj(struct qcomtee_object *p)
{ return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_INPUT,.object=p}; }
static struct qcomtee_param newobj(void)
{ return (struct qcomtee_param){.attr=QCOMTEE_OBJREF_OUTPUT}; }
static int tee_call(int fd, unsigned long op, ...)
{
    va_list ap; va_start(ap,op); void *p=va_arg(ap,void *); va_end(ap);
#ifdef GOODIX_PROBE_LIBRARY
    int old; pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS, &old);
    int ret = ioctl(fd, op, p), saved = errno;
    pthread_setcanceltype(old, NULL); errno = saved;
    return ret;
#else
    return ioctl(fd,op,p);
#endif
}
static int invoke(struct qcomtee_object *o, const char *name, uint32_t op,
                  struct qcomtee_param *p, int count)
{
    qcomtee_result_t result=UINT32_MAX;
    int ret=qcomtee_object_invoke(o,op,p,count,&result);
    fprintf(stderr,"%s: transport=%d service=%d\n",name,ret,(int32_t)result);
    return ret ? -1 : (int32_t)result;
}
static int identity(void)
{
    char b[256];
    int fd=open("/sys/firmware/devicetree/base/compatible",O_RDONLY|O_CLOEXEC);
    if (fd<0) return -1;
    ssize_t n=read(fd,b,sizeof(b)); close(fd);
    const char expected[]="nubia,nx809j";
    return n >= (ssize_t)sizeof(expected) && !memcmp(b,expected,sizeof(expected)) ? 0:-1;
}
static struct qcomtee_object *load(struct qcomtee_object *root,
                                  struct qcomtee_object *loader, const char *path)
{
    struct qcomtee_object *memory=NULL,*app=NULL;
    struct stat st;
    int fd=open(path,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);
    if (fd<0) { perror(path); return NULL; }
    if (fstat(fd,&st) || !S_ISREG(st.st_mode) || st.st_size<64 || st.st_size>32*1024*1024)
        goto done;
    size_t size=(size_t)st.st_size,offset=0;
    if (qcomtee_memory_object_alloc(size,root,&memory)) goto done;
    unsigned char *bytes=qcomtee_memory_object_addr(memory);
    while (offset<size) {
        ssize_t n=read(fd,bytes+offset,size-offset);
        if (n<0 && errno==EINTR) continue;
        if (n<=0) goto done;
        offset+=(size_t)n;
    }
    if (memcmp(bytes,"\177ELF\002\001",6)) goto done;
    char name[]="goodixfp64";
    struct qcomtee_param p[]={in(name,sizeof(name)-1),obj(memory),newobj()};
    if (!invoke(loader,"load goodixfp64",0,p,3)) app=p[2].object;
 done:
    if (memory) qcomtee_memory_object_release(memory);
    close(fd);
    return app;
}
#ifndef GOODIX_PROBE_LIBRARY
static void version_string(const char *label,const unsigned char *p,size_t n)
{
    printf("%s: ",label);
    for (size_t i=0;i<n && p[i];i++) {
        unsigned char c=p[i];
        if (c>=32 && c<=126) putchar(c); else printf("\\x%02x",c);
    }
    putchar('\n');
}
#endif
static int command(struct qcomtee_object *root,struct qcomtee_object *app,
                   uint32_t *packet,size_t size)
{
    struct qcomtee_object *memory=NULL;
    uint32_t request[16]={0},request_out[16]={0},response[16]={0};
    /* IQSEEComCompatApp's architecture argument must select ELF64.
     * A zero here returns service error 1 on this signed application. */
    uint32_t offsets[4]={0,0,0,0},is64=1;
    int ret=-1;
    if (size<32 || size>4096 || size%4) return -1;
    if (qcomtee_memory_object_alloc(4096,root,&memory)) return -1;
    void *mapping=qcomtee_memory_object_addr(memory);
    memset(mapping,0,4096);memcpy(mapping,packet,size);
    request[2]=(uint32_t)size;request[3]=1;
    uint64_t ms=(uint64_t)time(NULL)*1000;
    memcpy(request+4,&ms,sizeof(ms));
    struct qcomtee_param p[]={in(request,64),in(response,64),in(offsets,16),in(&is64,4),
        out(request_out,64),out(response,64),obj(memory),obj(NULL),obj(NULL),obj(NULL)};
    printf("Goodix target=%u command=%u bytes=%zu\n",packet[1],packet[2],size);
    if (invoke(app,"Goodix command",0,p,10)) goto done;
    if (p[5].ubuf.size<32) { fputs("Short response envelope\n",stderr); goto done; }
    memcpy(packet,mapping,size);
    printf("Goodix envelope_status=%d packet_status=%d\n",(int32_t)response[7],(int32_t)packet[3]);
    ret=response[7] ? (int32_t)response[7]:(int32_t)packet[3];
 done:
    explicit_bzero(mapping,4096);
    qcomtee_memory_object_release(memory);
    return ret;
}
#ifndef GOODIX_PROBE_LIBRARY
static int version(struct qcomtee_object *root,struct qcomtee_object *app)
{
    /* UsDebugTools::getVersion: target 1010, command 105, 496 bytes. */
    uint32_t packet[0x1f0/4]={0,1010,105};
    if (command(root,app,packet,sizeof(packet))) return 1;
    const unsigned char *b=(const unsigned char *)packet;
    /* Labels and offsets from UsDebugTools::getVersion in the stock HAL. */
    version_string("algorithm",b+0x20,0x40);
    version_string("preprocess",b+0x60,0x40);
    version_string("factory",b+0xe0,0x40);
    version_string("TEE",b+0x120,0x48);
    version_string("TA",b+0x168,0x40);
    version_string("package",b+0x1a8,0x40);
    printf("chip_id=%#x sensor_version=%#x\n",packet[0x1e8/4],packet[0x1ec/4]);
    /* Zero statuses alone are insufficient: require a returned version. */
    if (!memchr(b+0xe0,0,0x40) || !b[0xe0]) {
        fputs("Missing or unterminated factory version\n",stderr);
        return 1;
    }
    return 0;
}
static int identify(struct qcomtee_object *root,struct qcomtee_object *app)
{
    /* SensorDetector::detectSensor: the last 16 bytes are gf_sensor_info.
     * Do not guess its fields until the hardware response is established.
     */
    uint32_t packet[0x30/4]={0,1002,1};
    if (command(root,app,packet,sizeof(packet))) return 1;
    printf("Goodix sensor_info=%08x %08x %08x %08x\n",
           packet[8],packet[9],packet[10],packet[11]);
    if (!(packet[8]|packet[9]|packet[10]|packet[11])) {
        fputs("Empty sensor identification\n",stderr);
        return 1;
    }
    return 0;
}
#endif
/* Keymaster takes plain buffers, not the Goodix envelope. */
static int keymaster_exchange(void *context, const void *request,
                              size_t request_size, void *response,
                              size_t *response_size)
{
    unsigned char request_out[KEYMASTER_REQUEST_CAPACITY] = {0};
    uint32_t offsets[4] = {0}, is64 = 1;
    if (request_size > sizeof(request_out) || *response_size > KEYMASTER_REPLY_CAPACITY)
        return -EMSGSIZE;
    struct qcomtee_param p[] = {
        in((void *)request, request_size), in(response, *response_size),
        in(offsets, sizeof(offsets)), in(&is64, sizeof(is64)),
        out(request_out, request_size), out(response, *response_size),
        obj(NULL), obj(NULL), obj(NULL), obj(NULL)
    };
    int ret = invoke(context, "Keymaster command", 0, p, 10);
    *response_size = p[5].ubuf.size;
    explicit_bzero(request_out, sizeof(request_out));
    return ret;
}

static struct qcomtee_object *borrow_keymaster(struct qcomtee_object *loader)
{
    char name[] = "keymaster64";
    uint32_t architecture = 0;
    struct qcomtee_param find[] = {in(name, sizeof(name) - 1),
                                   out(&architecture, sizeof(architecture)), newobj()};
    if (invoke(loader, "lookup keymaster64", 2, find, 3)) return NULL;
    struct qcomtee_object *app = find[2].object;
    if (architecture == 2) return app;
    fputs("Expected ELF64 Keymaster service\n", stderr);
    qcomtee_object_refs_dec(app);
    return NULL;
}

static void keymaster_result(int status, const struct keymaster_agreement_result *result)
{
    printf("Keymaster stage=%s result=%d exchange=%d keymint=%d already_ready=%u\n",
           result->stage, status, result->exchange_status, result->keymint_status,
           (unsigned)result->already_ready);
}

#ifndef GOODIX_PROBE_LIBRARY
static int keymaster(struct qcomtee_object *loader, int agree)
{
    struct qcomtee_object *app = borrow_keymaster(loader);
    if (!app) return 1;
    struct keymaster_agreement_result result;
    int status = agree ? keymaster_shared_secret_agree_single_tee(keymaster_exchange, app, &result)
                       : keymaster_shared_secret_ready(keymaster_exchange, app, &result);
    keymaster_result(status, &result);
    if (!status) puts("Keymaster shared-secret signing is ready");
    /* The app was borrowed by lookup. Never unload it. */
    qcomtee_object_refs_dec(app);
    return status ? 1 : 0;
}

#endif
static int authentication_setup(struct qcomtee_object *root,
                                struct qcomtee_object *loader,
                                struct qcomtee_object *goodix)
{
    /* Goodix must already be loaded: the wrapper resolves the recipient TA.
     * Hold that app through delivery, using the real probe identity. */
    struct qcomtee_object *km = borrow_keymaster(loader);
    if (!km) return 1;
    uint8_t reply[KEYMASTER_WRAPPED_KEY_SIZE] = {0};
    uint32_t packet[288 / 4] = {0, 1000, 10};
    struct keymaster_agreement_result agreement;
    int status = keymaster_shared_secret_agree_single_tee(keymaster_exchange, km, &agreement);
    keymaster_result(status, &agreement);
    if (status) goto done;
    struct keymaster_wrapped_key_result wrapped;
    status = keymaster_fingerprint_wrapped_key(keymaster_exchange, km, reply, &wrapped);
    printf("Fingerprint wrapped key: result=%d exchange=%d status=%d offset=%u length=%u\n",
           status, wrapped.exchange_status, wrapped.status, wrapped.offset, wrapped.length);
    if (status) goto done;
    memcpy((uint8_t *)packet + 0x20, reply, sizeof(reply));
    explicit_bzero(reply, sizeof(reply));
    status = command(root, goodix, packet, sizeof(packet));
    if (!status) puts("Goodix authentication-key setup accepted");
 done:
    explicit_bzero(reply, sizeof(reply));
    explicit_bzero(packet, sizeof(packet));
    qcomtee_object_refs_dec(km);
    return status ? 1 : 0;
}

#ifndef GOODIX_PROBE_LIBRARY
int main(int argc,char **argv)
{
    int transport=argc==2 && !strcmp(argv[1],"--transport");
    int km_status=argc==2 && !strcmp(argv[1],"--keymaster-status");
    int km_agree=argc==2 && !strcmp(argv[1],"--keymaster-agree");
    int load_only=argc==3 && !strcmp(argv[1],"--load");
    int query=argc==3 && !strcmp(argv[1],"--version");
    int detect=argc==3 && !strcmp(argv[1],"--identify");
    int auth=argc==3 && !strcmp(argv[1],"--auth-setup");
    if (!transport && !load_only && !query && !detect && !auth && !km_status && !km_agree) {
        fprintf(stderr,"Usage: %s --transport | --keymaster-status | --keymaster-agree | --auth-setup goodixfp64.elf | --load goodixfp64.elf | --version goodixfp64.elf | --identify goodixfp64.elf\n",argv[0]);
        return 2;
    }
    if (identity()) { fputs("Expected nubia,nx809j\n",stderr); return 1; }
    setvbuf(stdout,NULL,_IONBF,0);
    int ret=1,control=-1;
    struct qcomtee_object *root=NULL,*env=NULL,*loader=NULL,*app=NULL;
    root=qcomtee_object_root_init("/dev/tee0",tee_call,NULL,NULL);
    if (!root) goto done;
    uint32_t uid=geteuid();
    unsigned char credentials[]={0xa2,1,0x1a,uid>>24,uid>>16,uid>>8,uid,3,0x6c,
        'g','o','o','d','i','x','-','p','r','o','b','e'};
    struct qcomtee_param reg[]={in(credentials,sizeof(credentials)),newobj()};
    if (invoke(root,"registerLegacy",1,reg,2)) goto done;
    env=reg[1].object;
    uint32_t id=122;
    struct qcomtee_param open_args[]={in(&id,4),newobj()};
    if (invoke(env,"open IQSEEComCompatAppLoader",0,open_args,2)) goto done;
    loader=open_args[1].object;
    if (km_status || km_agree) { ret=keymaster(loader,km_agree); goto done; }
    char name[]="goodixfp64"; uint32_t arch=0;
    struct qcomtee_param find[]={in(name,sizeof(name)-1),out(&arch,4),newobj()};
    int status=invoke(loader,"lookup goodixfp64",2,find,3);
    if (!status) {
        printf("goodixfp64 already loaded, ELF class=%u\n",arch);
        qcomtee_object_refs_dec(find[2].object);
        if (!transport) { fputs("Existing fingerprint session; refusing to change it\n",stderr); goto done; }
    } else if (status==23) puts("goodixfp64 not loaded");
    else goto done;
    if (transport) { ret=0; goto done; }
    if (detect) {
        control=open("/dev/goodix_fp",O_RDWR|O_CLOEXEC|O_NOFOLLOW);
        if (control<0) { perror("Goodix control"); goto done; }
        struct goodix_fp_state state;
        if (ioctl(control,GOODIX_FP_IOC_GET_STATE,&state) || state.version!=1 || state.powered) {
            fputs("Expected idle Goodix native control interface v1\n",stderr); goto done;
        }
        if (ioctl(control,GOODIX_FP_IOC_ENABLE_POWER)) { perror("Goodix power on"); goto done; }
    }
    app=load(root,loader,argv[2]);
    if (!app) goto done;
    ret=auth ? authentication_setup(root,loader,app):detect ? identify(root,app):query ? version(root,app):0;
    if (invoke(app,"unload owned goodixfp64",2,NULL,0)) ret=1;
 done:
    if (app) qcomtee_object_refs_dec(app);
    if (control>=0) {
        if (ioctl(control,GOODIX_FP_IOC_DISABLE_POWER)) { perror("Goodix power off"); ret=1; }
        close(control);
    }
    if (loader) qcomtee_object_refs_dec(loader);
    if (env) qcomtee_object_refs_dec(env);
    if (root) qcomtee_object_refs_dec(root);
    return ret;
}

#endif
