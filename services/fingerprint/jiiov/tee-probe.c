/* SPDX-License-Identifier: GPL-2.0-only */
/* Native Linux QSEECom-compatible fingerprint discovery and chip identification.
 * UAPI: Motorola securemsm include/uapi/linux/smcinvoke.h.
 * Operations: IClientEnv.h and IQSEEComCompatAppLoader.h.
 * No Android library dependency. Loads the existing signed firmware unchanged.
 */
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <sys/stat.h>
#include <elf.h>
#include <sys/mman.h>
#include <linux/dma-heap.h>
#include <linux/dma-buf.h>
#include <sys/file.h>
#include <time.h>
#include <stddef.h>
#include <signal.h>
#include "smcinvoke.h"
#include "anc-protocol.h"

_Static_assert(sizeof(union smcinvoke_arg) == 24, "smcinvoke argument ABI");
_Static_assert(sizeof(struct smcinvoke_cmd_req) == 32, "smcinvoke request ABI");

static volatile sig_atomic_t interrupted;
static int progress_fd=-1;
static uint32_t delete_identifier;
static int capture_ready_fd = -1;
static uint64_t timing_start;
#include "capture-gate.h"
static void timing(const char *phase)
{
    if (capture_ready_fd >= 0)
        printf("JTIM1 %s %llu\n", phase, (unsigned long long)(monotonic_us() - timing_start));
}
static void interrupt_probe(int signo) { interrupted=signo; }
static void diagnostic_progress(const char *phase, uint32_t remaining)
{
    if (progress_fd<0) return;
    char message[64];
    int n=snprintf(message,sizeof(message),"%s %u\n",phase,remaining);
    if (n>0 && n<(int)sizeof(message) && pwrite(progress_fd,message,n,0)==n)
        (void)ftruncate(progress_fd,n);
}
static void diagnostic_pause(long milliseconds)
{
    struct timespec t={milliseconds/1000,(milliseconds%1000)*1000000};
    while (!interrupted && nanosleep(&t,&t) && errno==EINTR) {}
}

static uint32_t counts(unsigned bi, unsigned bo, unsigned oi, unsigned oo)
{ return bi | bo << 4 | oi << 8 | oo << 12; }
static union smcinvoke_arg buffer(void *p, size_t size)
{ return (union smcinvoke_arg){.b = {.addr=(uintptr_t)p, .size=size}}; }
static union smcinvoke_arg null_object(void)
{ return (union smcinvoke_arg){.o = {.fd=-1, .cb_server_fd=-1}}; }
static int invoke(int fd, const char *name, unsigned op, unsigned n,
                  union smcinvoke_arg *args)
{
    struct smcinvoke_cmd_req req = {.op=op, .counts=n, .result=INT32_MIN,
        .argsize=sizeof(*args), .args=(uintptr_t)args};
    int ret = ioctl(fd, SMCINVOKE_IOCTL_INVOKE_REQ, &req);
    int saved_errno = errno;
    printf("%s: ioctl=%d errno=%d result=%" PRId32 "\n", name, ret,
           ret < 0 ? saved_errno : 0, req.result);
    if (ret < 0) fprintf(stderr, "%s: %s\n", name, strerror(saved_errno));
    return ret < 0 ? -saved_errno : req.result;
}
#include "tee-files.h"
static void *read_firmware(const char *path, size_t *size)
{
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    struct stat st;
    void *data = NULL;
    if (fd < 0) { perror(path); return NULL; }
    if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_size < 64 ||
        st.st_size > 32*1024*1024) goto fail;
    *size = st.st_size;
    data = malloc(*size);
    if (!data) goto fail;
    size_t done = 0;
    while (done < *size) {
        ssize_t n = read(fd, (char *)data+done, *size-done);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) goto fail;
        done += n;
    }
    Elf64_Ehdr *eh = data;
    if (memcmp(eh->e_ident, "\177ELF\2\1", 6) || eh->e_machine != EM_AARCH64)
        goto fail;
    close(fd);
    return data;
fail:
    fprintf(stderr,"Invalid or unreadable firmware: %s\n",path);
    free(data);
    close(fd);
    return NULL;
}
static int firmware_command_dma(int app, uint32_t group, uint32_t command,
                               uint32_t parameter, uint32_t rsp_out[0x140/4],
                               int dma, uint32_t dma_size)
{
    uint32_t req[0x240/4] = {0}, rsp[0x140/4] = {0};
    uint32_t req_out[0x240/4] = {0};
    memset(rsp_out,0,0x140);
    uint32_t is64 = 1;
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC,&now)) return -1;
    uint64_t ms = (uint64_t)now.tv_sec*1000 + now.tv_nsec/1000000;
    /* AncCaSendCommand sets the logical length; AncCaTransmit clears 4..15. */
    anc_request_init((void *)req,group,command,parameter,ms);
    /* Algorithm extraction's type field selects verification rather than
     * enrollment. Stock AlgoVerifyExtractFeature sets sub-payload 0x14=1.
     */
    if (group==3 && command==ANC_EXTRACT && parameter==1) req[0x30/4]=1;
    uint32_t offsets[4]={4,0,0,0};
    if (dma>=0) req[3]=dma_size;
    union smcinvoke_arg args[] = {buffer(req,sizeof(req)),buffer(rsp,sizeof(rsp)),
        buffer(dma>=0 ? offsets : NULL,dma>=0 ? sizeof(offsets) : 0),buffer(&is64,4),
        buffer(req_out,sizeof(req_out)),buffer(rsp_out,0x140),
        null_object(),null_object(),null_object(),null_object()};
    if (dma>=0) args[6].o.fd=dma;
    printf("Firmware group=%u command=%u parameter=%u\n",group,command,parameter);
    int result = invoke(app,"sensor sendRequest",0,counts(4,2,4,0),args);
    if (result) return result;
    uint32_t vendor_status;
    if (anc_response_status(rsp_out,args[5].b.size,&vendor_status)) {
        fputs("Invalid firmware response length\n",stderr); return -1;
    }
    printf("Sensor status=%u value=0x%08x response_bytes=%llu\n",rsp_out[0],rsp_out[1],
           (unsigned long long)args[5].b.size);
    return vendor_status;
}
static int firmware_command(int app, uint32_t group, uint32_t command,
                            uint32_t parameter, uint32_t rsp[0x140/4])
{
    return firmware_command_dma(app,group,command,parameter,rsp,-1,0);
}
static int capture_diagnostic(int app)
{
    /* Fixed, bounded allocation for the identified JV0307. Firmware exports
     * a 44-byte header followed by pixels (AncExtensionGetImage, 0x275c).
     * No image is logged or persisted by this diagnostic.
     */
    const uint32_t capacity=65536;
    int status=1, heap=-1, dma=-1;
    void *mapping=MAP_FAILED;
    uint32_t rsp[0x140/4];
    heap=open("/dev/dma_heap/qcom,qseecom",O_RDWR|O_CLOEXEC);
    if (heap<0) { perror("image heap"); goto out; }
    struct dma_heap_allocation_data alloc={.len=capacity,.fd_flags=O_RDWR|O_CLOEXEC};
    if (ioctl(heap,DMA_HEAP_IOCTL_ALLOC,&alloc)) { perror("image allocation"); goto out; }
    dma=alloc.fd;
    mapping=mmap(NULL,capacity,PROT_READ|PROT_WRITE,MAP_SHARED,dma,0);
    if (mapping==MAP_FAILED) { perror("image mmap"); goto out; }
    struct dma_buf_sync sync={.flags=DMA_BUF_SYNC_START|DMA_BUF_SYNC_WRITE};
    if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) goto out;
    memset(mapping,0,capacity);
    sync.flags=DMA_BUF_SYNC_END|DMA_BUF_SYNC_WRITE;
    if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) goto out;
    if (interrupted) goto out;
    int capture_status=firmware_command(app,2,5,1,rsp);
    printf("Diagnostic capture status=%d (illumination not coordinated)\n",capture_status);
    if (capture_status<0 || interrupted) goto out;
    if (firmware_command_dma(app,5,1,0,rsp,dma,capacity)) goto out;
    sync.flags=DMA_BUF_SYNC_START|DMA_BUF_SYNC_READ;
    if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) goto out;
    uint32_t header[11];
    memcpy(header,mapping,sizeof(header));
    printf("Export header: bytes=%u width=%u height=%u format_words=%u,%u,%u,%u,%u,%u,%u,%u\n",
        header[0],header[1],header[2],header[3],header[4],header[5],header[6],header[7],header[8],header[9],header[10]);
    struct anc_frame frame;
    if (!anc_jv0307_frame_parse(mapping,capacity,&frame)) {
        uint16_t minimum=UINT16_MAX, maximum=0;
        uint64_t sum=0;
        for (uint32_t i=0;i<header[0];i+=2) {
            uint16_t sample;
            memcpy(&sample,(char *)mapping+sizeof(header)+i,2);
            if (sample<minimum) minimum=sample;
            if (sample>maximum) maximum=sample;
            sum+=sample;
        }
        printf("Frame samples=%u min=%u max=%u mean=%.2f\n",header[0]/2,minimum,maximum,
               (double)sum/(header[0]/2));
        status=capture_status ? 1 : 0;
    } else fputs("Unexpected frame dimensions or payload size\n",stderr);
    sync.flags=DMA_BUF_SYNC_END|DMA_BUF_SYNC_READ;
    if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) status=1;
out:
    if (mapping!=MAP_FAILED) munmap(mapping,capacity);
    if (dma>=0) close(dma);
    if (heap>=0) close(heap);
    return status;
}
static int sensor_command(int app, uint32_t command, uint32_t parameter, uint32_t *value)
{
    uint32_t rsp_out[0x140/4];
    int result=firmware_command(app,2,command,parameter,rsp_out);
    if (result) return result;
    if (value) *value=rsp_out[1];
    if (command==4 && rsp_out[2]!=parameter) return -1;
    if (command==4 && parameter==0x3b) {
        const unsigned char *product=(const void *)&rsp_out[3];
        fputs("JIIOV product ID: ",stdout);
        for (size_t i=0; i<16 && product[i]; i++)
            putchar(product[i]>=32 && product[i]<127 ? product[i] : '?');
        putchar('\n');
    }
    return 0;
}
static int storage_diagnostic(int app, uint32_t *count, uint32_t ids[5])
{
    /* Isolated virtual namespace. Writes require an explicitly supplied
     * private storage directory; existing vendor enrollment is inaccessible.
     */
    static const char path[]="/droidloom-fingerprint-probe";
    const uint32_t capacity=4096;
    int status=1, heap=-1, dma=-1;
    void *mapping=MAP_FAILED;
    uint32_t rsp[ANC_RESPONSE_SIZE/4];
    heap=open("/dev/dma_heap/qcom,qseecom",O_RDWR|O_CLOEXEC);
    if (heap<0) goto out;
    struct dma_heap_allocation_data alloc={.len=capacity,.fd_flags=O_RDWR|O_CLOEXEC};
    if (ioctl(heap,DMA_HEAP_IOCTL_ALLOC,&alloc)) goto out;
    dma=alloc.fd;
    mapping=mmap(NULL,capacity,PROT_READ|PROT_WRITE,MAP_SHARED,dma,0);
    if (mapping==MAP_FAILED) goto out;
    struct dma_buf_sync sync={.flags=DMA_BUF_SYNC_START|DMA_BUF_SYNC_WRITE};
    if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) goto out;
    memset(mapping,0,capacity);
    memcpy(mapping,path,sizeof(path));
    sync.flags=DMA_BUF_SYNC_END|DMA_BUF_SYNC_WRITE;
    if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync) || interrupted) goto out;
    if (firmware_command_dma(app,3,ANC_TEMPLATE_SET_GROUP,941001,rsp,dma,capacity)) goto out;
    if (interrupted) goto out;
    int load_status=firmware_command(app,3,ANC_TEMPLATE_LOAD,0,rsp);
    printf("Isolated template load status=%d\n",load_status);
    if (interrupted) goto out;
    int list_status=firmware_command(app,3,ANC_TEMPLATE_LIST,0,rsp);
    printf("Isolated template list status=%d count=%u\n",list_status,
           anc_get_le32((char *)rsp+0x28));
    *count=anc_get_le32((char *)rsp+0x28);
    if (*count>5) goto out;
    for (unsigned i=0;i<*count;i++) {
        ids[i]=anc_get_le32((char *)rsp+0x14+4*i);
        printf("Isolated template ID=%u\n",ids[i]);
    }
    status=load_status || list_status;
out:
    if (mapping!=MAP_FAILED) munmap(mapping,capacity);
    if (dma>=0) close(dma);
    if (heap>=0) close(heap);
    return status;
}
static int enroll_sample_diagnostic(int app)
{
    uint32_t rsp[ANC_RESPONSE_SIZE/4];
    int status=1;
    int begin=firmware_command(app,3,ANC_ENROLL_BEGIN,0,rsp);
    if (begin || interrupted) goto out;
    diagnostic_progress("HOLD",20);
    diagnostic_pause(1000);
    if (firmware_command(app,2,5,0,rsp) || interrupted) goto out;
    int extract=firmware_command(app,3,ANC_EXTRACT,0,rsp);
    printf("Enrollment feature extraction status=%d (300 means extracted)\n",extract);
    if (extract!=300 || interrupted) goto out;
    int add=firmware_command(app,3,ANC_ENROLL_ADD,0,rsp);
    printf("Enrollment sample status=%d remaining=%u; session will be aborted\n",
           add,anc_get_le32((char *)rsp+4));
    status=add==400 || add==401 ? 0 : 1;
out:
    diagnostic_progress("LIFT",20);
    if (firmware_command(app,3,ANC_ENROLL_END,0,rsp)) status=1;
    return status;
}
static int persistent_diagnostic(int app, int operation, uint32_t count, uint32_t ids[5])
{
    uint32_t rsp[ANC_RESPONSE_SIZE/4];
    if (operation==4) return 0;
    if (operation==5) {
        if (count) { fputs("Storage write check requires an empty diagnostic database\n",stderr); return 1; }
        int update=firmware_command(app,3,24,0,rsp);
        printf("Empty database authenticator update status=%d\n",update);
        if (!update) {
            int remove=firmware_command(app,3,ANC_TEMPLATE_DELETE,0,rsp);
            printf("Empty database delete-all status=%d\n",remove);
        }
        return update ? 1 : 0;
    }
    if (operation==3) {
        int found=0;
        for (uint32_t i=0;i<count;i++) if (ids[i]==delete_identifier) found=1;
        if (!found) { fputs("Requested template is absent\n",stderr); return 1; }
        if (interrupted || firmware_command(app,3,ANC_TEMPLATE_DELETE,delete_identifier,rsp)) return 1;
        if (firmware_command(app,3,ANC_TEMPLATE_LIST,0,rsp)) return 1;
        uint32_t remaining=anc_get_le32((char *)rsp+0x28);
        printf("After deletion template count=%u\n",remaining);
        if (remaining!=count-1) return 1;
        for (uint32_t i=0;i<remaining;i++)
            if (anc_get_le32((char *)rsp+0x14+4*i)==delete_identifier) return 1;
        return 0;
    }
    if (operation==2) {
        if (!count) { fputs("No diagnostic template to verify\n",stderr); return 1; }
        int status=1;
        if (firmware_command(app,3,ANC_VERIFY_BEGIN,0,rsp) || interrupted) goto verify_out;
        // Guided diagnostics need time for a person to position a finger.
        // Integrated verification starts only after real DOWN and settled HBM.
        if (progress_fd >= 0) {
            diagnostic_progress("PLACE",20);
            diagnostic_pause(800);
            diagnostic_progress("HOLD",20);
            diagnostic_pause(1000);
        }
        timing("prepared");
        if (capture_ready_fd >= 0 && capture_gate_wait(capture_ready_fd, 3000)) {
            fputs("Capture permission denied or cancelled\n", stderr);
            goto verify_out;
        }
        if (interrupted) goto verify_out;
        timing("capture-start");
        if (firmware_command(app,2,5,1,rsp) || interrupted) goto verify_out;
        timing("capture-end");
        int extract=firmware_command(app,3,ANC_EXTRACT,1,rsp);
        timing("extract-end");
        printf("Verification extraction status=%d\n",extract);
        if (extract!=300 || interrupted) goto verify_out;
        int compare=firmware_command(app,3,ANC_COMPARE,0,rsp);
        timing("compare-end");
        printf("Verification compare status=%d matched_id=%u\n",compare,
               anc_get_le32((char *)rsp+0x10));
        status=compare==500 ? 0 : 1;
verify_out:
        if (firmware_command(app,3,ANC_VERIFY_END,0,rsp)) status=1;
        return status;
    }
    if (count>=5) { fputs("Diagnostic database is full\n",stderr); return 1; }
    int status=1;
    if (firmware_command(app,3,ANC_ENROLL_BEGIN,0,rsp) || interrupted) goto enroll_out;
    uint32_t remaining=20;
    for (unsigned attempt=0; attempt<40 && !interrupted; attempt++) {
        diagnostic_progress("PLACE",remaining);
        diagnostic_pause(500);
        diagnostic_progress("HOLD",remaining);
        diagnostic_pause(1000);
        if (interrupted) break;
        printf("Enrollment capture attempt=%u\n",attempt+1);
        int capture=firmware_command(app,2,5,0,rsp);
        if (capture<0) break;
        if (capture) goto next_sample;
        int extract=firmware_command(app,3,ANC_EXTRACT,0,rsp);
        if (extract<0) break;
        if (extract!=300) { printf("Enrollment retry extraction=%d\n",extract); goto next_sample; }
        int add=firmware_command(app,3,ANC_ENROLL_ADD,0,rsp);
        uint32_t next_remaining=anc_get_le32((char *)rsp+4);
        if (add==400 || add==401) {
            if (next_remaining>remaining || next_remaining>20) break;
            remaining=next_remaining;
        }
        printf("Enrollment progress status=%d remaining=%u\n",add,remaining);
        if (add<0) break;
        if ((add==400 || add==401) && !remaining) {
            if (interrupted) break;
            int finish=firmware_command(app,3,ANC_ENROLL_END,1,rsp);
            printf("Enrollment finalize status=%d template_id=%u\n",finish,
                   anc_get_le32((char *)rsp+0x10));
            diagnostic_progress(finish ? "LIFT" : "DONE",remaining);
            return finish ? 1 : 0;
        }
next_sample:
        diagnostic_progress("LIFT",remaining);
        diagnostic_pause(800);
    }
enroll_out:
    if (firmware_command(app,3,ANC_ENROLL_END,0,rsp)) status=1;
    return status;
}
static int identify_sensor(int app, int prepare, int capture, int sessions, int storage, int sample, int operation)
{
    int fd = open("/dev/jiiov_fp",O_RDWR|O_CLOEXEC);
    if (fd < 0) { perror("/dev/jiiov_fp"); return 1; }
    int status = 1, resources = 0, powered = 0, initialized = 0, algorithm = 0;
    uint32_t rsp[0x140/4];
    if (flock(fd,LOCK_EX|LOCK_NB)) { perror("sensor lock"); goto done; }
    if (ioctl(fd,0x6101,0)) { perror("request sensor resources"); goto done; }
    resources = 1;
    if (interrupted) goto done;
    if (ioctl(fd,0x610b,0)) { perror("power sensor"); goto done; }
    powered = 1;
    if (interrupted) goto done;
    if (ioctl(fd,0x6100,0)) { perror("reset sensor"); goto done; }
    if (interrupted) goto done;
    initialized = 1;
    int init_status = sensor_command(app,1,0,NULL);
    /* OTP loading may fail after SPI initialization; a read-only chip-ID
     * query helps distinguish sensor transport from missing OTP services.
     * Keep the overall result unsuccessful when initialization failed.
     */
    if (interrupted || (init_status && init_status != 34)) goto done;
    uint32_t chip = 0;
    if (sensor_command(app,6,0,&chip)) goto done;
    if (!chip || chip == UINT32_MAX) { fputs("Invalid chip ID\n",stderr); goto done; }
    printf("JIIOV chip ID: 0x%08x\n",chip);
    if (interrupted || sensor_command(app,4,0x3b,NULL)) goto done;
    if (prepare && !init_status) {
        if (chip!=0x46544301) { fputs("Unvalidated chip for preparation\n",stderr); goto done; }
        if (firmware_command(app,2,4,0x33,rsp) || rsp[2]!=0x33) goto done;
        printf("Sensor geometry words: %u %u %u %u\n",rsp[3],rsp[4],rsp[5],rsp[6]);
        if (interrupted) goto done;
        algorithm=1;
        if (firmware_command(app,3,1,0,rsp)) goto done;
        if (interrupted || firmware_command(app,5,25,0,rsp)) goto done;
        puts("Algorithm and optical calibration preparation succeeded.");
        timing("sensor-ready");
        uint32_t count=0, ids[5]={0};
        if (storage && storage_diagnostic(app,&count,ids)) goto done;
        timing("templates-ready");
        if (operation && persistent_diagnostic(app,operation,count,ids)) goto done;
        if (sample && enroll_sample_diagnostic(app)) goto done;
        if (sessions) {
            if (interrupted || firmware_command(app,3,18,0,rsp)) goto done;
            printf("Enrollment stages: %u\n",anc_get_le32((char *)rsp+0x30));
            if (interrupted) goto done;
            int enroll_status=firmware_command(app,3,3,0,rsp);
            /* Command 5 with parameter zero aborts without generating a
             * template. Never use a positive finalize parameter here.
             */
            int abort_status=firmware_command(app,3,5,0,rsp);
            if (enroll_status || abort_status || interrupted) goto done;
            int verify_status=firmware_command(app,3,6,0,rsp);
            int end_status=firmware_command(app,3,12,0,rsp);
            if (verify_status || end_status) goto done;
            puts("Enrollment/verification session start and abort succeeded; no template created.");
        }
        if (capture && capture_diagnostic(app)) goto done;
    }
    status = init_status ? 1 : 0;
done:
    if (algorithm && firmware_command(app,3,2,0,rsp)) status=1;
    if (initialized && sensor_command(app,2,0,NULL)) status = 1;
    if (powered && ioctl(fd,0x610c,0)) { perror("power off sensor"); status = 1; }
    if (resources && ioctl(fd,0x6102,0)) { perror("release resources"); status = 1; }
    close(fd);
    return status;
}
int main(int argc, char **argv)
{
    timing_start = monotonic_us();
    int gated = argc == 5 && !strcmp(argv[1], "--verify-gated-with-files");
    const char *gate = getenv("ANC_CAPTURE_READY_FD");
    if (gated) {
        if (!gate || !*gate) return 2;
        char *end;
        errno = 0;
        long fd = strtol(gate, &end, 10);
        struct stat descriptor;
        if (errno || *end || fd < 3 || fd > 1024 || fstat((int)fd, &descriptor)
            || !S_ISFIFO(descriptor.st_mode) || (fcntl((int)fd, F_GETFL) & O_ACCMODE) != O_RDONLY)
            return 2;
        capture_ready_fd = (int)fd;
    } else if (gate) return 2;
    const char *progress=getenv("ANC_DIAGNOSTIC_PROGRESS_FD");
    if (gated && progress) return 2;
    if (progress) {
        char *end;
        long fd=strtol(progress,&end,10);
        if (*end || fd<3 || fd>1024 || fcntl(fd,F_GETFD)<0) return 2;
        progress_fd=(int)fd;
    }
    int operation=0;
    if (argc==5) {
        if (!strcmp(argv[1],"--enroll-with-files")) operation=1;
        if (!strcmp(argv[1],"--verify-with-files") || gated) operation=2;
        if (!strcmp(argv[1],"--list-with-files")) operation=4;
        if (!strcmp(argv[1],"--storage-write-check-with-files")) operation=5;
    }
    if (argc==6 && !strcmp(argv[1],"--delete-with-files")) {
        char *end;
        errno=0;
        unsigned long value=strtoul(argv[5],&end,10);
        if (errno || *end || argv[5][0]<'1' || argv[5][0]>'9' ||
            !value || value>UINT32_MAX) return 2;
        delete_identifier=(uint32_t)value;
        operation=3;
    }
    int capture=argc==4 && !strcmp(argv[1],"--capture-diagnostic-with-files");
    int sessions=argc==4 && !strcmp(argv[1],"--algorithm-sessions-with-files");
    int storage=operation || (argc==4 && !strcmp(argv[1],"--storage-diagnostic-with-files"));
    int sample=argc==4 && !strcmp(argv[1],"--enroll-sample-with-files");
    int prepare=operation || (argc==4 && (!strcmp(argv[1],"--prepare-with-files") || capture || sessions || storage || sample));
    int with_files=operation || (argc==4 && (!strcmp(argv[1],"--identify-with-files") || prepare));
    if (!with_files && argc != 1 && (argc != 3 || (strcmp(argv[1],"--load") && strcmp(argv[1],"--identify")))) {
        fprintf(stderr,"Usage: %s [--load|--identify ancapp64.elf]\n"
                       "       %s --identify-with-files|--prepare-with-files|--capture-diagnostic-with-files|--algorithm-sessions-with-files|--storage-diagnostic-with-files|--enroll-sample-with-files ancapp64.elf calibration-copy-dir\n",argv[0],argv[0]);
        return 2;
    }
    setvbuf(stdout, NULL, _IONBF, 0);
    struct sigaction action={.sa_handler=interrupt_probe};
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGINT,&action,NULL) || sigaction(SIGTERM,&action,NULL)) {
        perror("signal handlers"); return 1;
    }
    int root = -1, env = -1, loader = -1, app = -1, status = 1, files_started = 0;
    root = open("/dev/smcinvoke", O_RDWR | O_CLOEXEC);
    if (root < 0) { perror("/dev/smcinvoke"); goto out; }
    /* Legacy CBOR credential attributes: actual effective UID and own name.
     * These are self-reported, not a forged Android system identity.
     */
    uint32_t uid = geteuid();
    unsigned char cred[] = {0xa2, 0x01, 0x1a,
        uid >> 24, uid >> 16, uid >> 8, uid,
        0x03, 0x78, 24,
        'm','o','t','o','7','0','-','f','i','n','g','e','r','p','r','i','n','t','-','p','r','o','b','e'};
    printf("Native client uid=%u\n", uid);
    union smcinvoke_arg reg[] = {buffer(cred,sizeof(cred)),null_object()};
    if (invoke(root,"registerLegacy",1,counts(1,0,0,1),reg)) goto out;
    env = reg[1].o.fd;
    if (env < 0) { fputs("No client environment returned\n",stderr); goto out; }
    uint32_t loader_uid = 122;
    union smcinvoke_arg args[] = {buffer(&loader_uid,4),null_object()};
    if (invoke(env,"open compat loader",0,counts(1,0,0,1),args)) goto out;
    loader = args[1].o.fd;
    if (loader < 0) { fputs("No loader returned\n",stderr); goto out; }
    char name[] = "ancapp64";
    uint32_t arch = 0;
    union smcinvoke_arg lookup[] = {buffer(name,strlen(name)),buffer(&arch,4),null_object()};
    int result = invoke(loader,"lookup ancapp64",2,counts(1,1,0,1),lookup);
    if (result == 23) {
        puts("Transport works; JIIOV firmware is not loaded.");
        if (argc == 1) { status = 0; goto out; }
        if (with_files) {
            if (fs_start(env,argv[3],operation ? argv[4] : NULL)) goto out;
            files_started=1;
        }
        size_t size = 0;
        void *image = read_firmware(argv[2], &size);
        if (!image) goto out;
        int heap = open("/dev/dma_heap/qcom,qseecom-ta",O_RDWR|O_CLOEXEC);
        if (heap < 0) { perror("QSEECom TA heap"); free(image); goto out; }
        struct dma_heap_allocation_data alloc = {.len=(size+4095)&~(size_t)4095,
            .fd_flags=O_RDWR|O_CLOEXEC};
        if (ioctl(heap,DMA_HEAP_IOCTL_ALLOC,&alloc)) {
            perror("DMA_HEAP_IOCTL_ALLOC"); close(heap); free(image); goto out;
        }
        close(heap);
        int dma = alloc.fd;
        void *mapping = mmap(NULL,alloc.len,PROT_READ|PROT_WRITE,MAP_SHARED,dma,0);
        if (mapping == MAP_FAILED) { perror("mmap"); close(dma); free(image); goto out; }
        struct dma_buf_sync sync = {.flags=DMA_BUF_SYNC_START|DMA_BUF_SYNC_WRITE};
        if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) {
            perror("DMA sync start"); munmap(mapping,alloc.len); close(dma); free(image); goto out;
        }
        memset(mapping,0,alloc.len);
        memcpy(mapping,image,size);
        free(image);
        sync.flags = DMA_BUF_SYNC_END|DMA_BUF_SYNC_WRITE;
        if (ioctl(dma,DMA_BUF_IOCTL_SYNC,&sync)) {
            perror("DMA sync end"); munmap(mapping,alloc.len); close(dma); goto out;
        }
        union smcinvoke_arg load[] = {buffer(name,strlen(name)),
            {.o={.fd=dma,.cb_server_fd=-1}},null_object()};
        result = invoke(loader,"load ancapp64 from DMA region",0,counts(1,0,1,1),load);
        munmap(mapping,alloc.len);
        close(dma);
        if (result) goto out;
        app = load[2].o.fd;
        if (app < 0) { fputs("No application returned\n",stderr); goto out; }
        puts("Firmware loaded from DMA region.");
        timing("firmware-loaded");
        int sensor_status = interrupted ? 1 : 0;
        if (!interrupted && (with_files || !strcmp(argv[1],"--identify"))) sensor_status = identify_sensor(app,prepare,capture,sessions,storage,sample,operation);
        /* Only unload an application this invocation loaded. */
        if (invoke(app,"unload ancapp64",2,0,NULL)) goto out;
        timing("cleanup-done");
        status = sensor_status;
    } else if (result == 0) {
        app = lookup[2].o.fd;
        if (app < 0) { fputs("No application returned\n",stderr); goto out; }
        printf("JIIOV firmware found, ELF class=%u\n",arch);
        if (with_files || (argc == 3 && !strcmp(argv[1],"--identify"))) {
            fputs("Refusing to change a sensor owned by an existing firmware session\n",stderr);
            goto out;
        }
        status = 0;
    }
out:
    if (app >= 0) close(app);
    if (files_started) fs_stop();
    if (loader >= 0) close(loader);
    if (env >= 0) close(env);
    if (root >= 0) close(root);
    return interrupted ? 128+interrupted : status;
}
