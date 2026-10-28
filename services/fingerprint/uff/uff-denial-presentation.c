// SPDX-License-Identifier: GPL-2.0-only
// Aston privileged fprintd presentation worker, adapted from Roadstr's client.
// No biometric data is presented or logged here.
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/dma-buf.h>
#include <linux/dma-heap.h>
#include <linux/input.h>
#include <poll.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>
#include "linux-dmabuf-client-protocol.h"
#include "denial-fingerprint-client-protocol.h"
#include "uff-contact.h"
#include <linux/aston_fp_illumination.h>

#define ARGB8888 0x34325241u
static volatile sig_atomic_t interrupted;
static struct wl_display *display;
static struct zwp_linux_dmabuf_v1 *dmabuf;
static struct denial_fingerprint_manager_v1 *manager;
static struct denial_fingerprint_session_v1 *session;
static struct wl_buffer *buffer;
static uint32_t width, height;
static int32_t x, y;
static bool configured, active, presented, ready, lost, finishing, stopped, released;
static bool linear_argb;
static int inputs[2] = {-1, -1};
static struct uff_contact contact;

static void signal_stop(int signal_number) { (void)signal_number; interrupted = 1; }
static double now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}
static void configure(void *data, struct denial_fingerprint_session_v1 *object,
                      const char *output, uint32_t pw, uint32_t ph, int32_t px,
                      int32_t py, uint32_t w, uint32_t h, uint32_t duration) {
    (void)data; (void)object;
    // This hardware worker is intentionally pinned to the validated board.
    if (strcmp(output, "DSI-1") || pw != 1264 || ph != 2780 || px != 535 ||
        py != 1928 || w != 195 || h != 195 || duration < 3000 || duration > 30000) {
        lost = true;
        return;
    }
    x = px; y = py; width = w; height = h; configured = true;
}
static void presentation_ready(void *data, struct denial_fingerprint_session_v1 *object,
                               uint32_t serial) {
    (void)data; (void)object;
    if (serial != 1 || !active || ready || finishing || lost) return;
    ready = true;
    puts("DFP1 ready 1");
}
static void presentation_stopped(void *data, struct denial_fingerprint_session_v1 *object,
                                 uint32_t serial, uint32_t reason) {
    (void)data; (void)object;
    if (serial != 1 || !active) { lost = true; return; }
    active = false; stopped = true;
    if (!finishing || reason != DENIAL_FINGERPRINT_SESSION_V1_STOP_REASON_RELEASED) lost = true;
    printf("DFP1 stopped %u %u\n", serial, reason);
}
static const struct denial_fingerprint_session_v1_listener session_listener = {
    .configured = configure, .ready = presentation_ready, .stopped = presentation_stopped,
};
static void buffer_release(void *data, struct wl_buffer *object) {
    (void)data; (void)object; released = true;
}
static const struct wl_buffer_listener buffer_listener = {.release = buffer_release};
static void advertised_format(void *data, struct zwp_linux_dmabuf_v1 *object, uint32_t format) {
    (void)data; (void)object; (void)format;
}
static void advertised_modifier(void *data, struct zwp_linux_dmabuf_v1 *object,
                                uint32_t format, uint32_t high, uint32_t low) {
    (void)data; (void)object;
    if (format == ARGB8888 && high == 0 && low == 0) linear_argb = true;
}
static const struct zwp_linux_dmabuf_v1_listener dmabuf_listener = {
    .format = advertised_format, .modifier = advertised_modifier,
};
static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    (void)data;
    if (!strcmp(interface, "denial_fingerprint_manager_v1") && version >= 2)
        manager = wl_registry_bind(registry, name, &denial_fingerprint_manager_v1_interface, 2);
    if (!strcmp(interface, "zwp_linux_dmabuf_v1") && version >= 3) {
        dmabuf = wl_registry_bind(registry, name, &zwp_linux_dmabuf_v1_interface, 3);
        zwp_linux_dmabuf_v1_add_listener(dmabuf, &dmabuf_listener, NULL);
    }
}
static void global_remove(void *data, struct wl_registry *registry, uint32_t name) {
    (void)data; (void)registry; (void)name;
}
static const struct wl_registry_listener registry_listener = {global, global_remove};

static int make_image(void) {
    uint32_t stride = (width * 4 + 63) & ~63u;
    size_t size = (size_t)stride * height;
    int heap = open("/dev/dma_heap/system", O_RDWR | O_CLOEXEC);
    if (heap < 0) return -1;
    struct dma_heap_allocation_data allocation = {.len = size, .fd_flags = O_RDWR | O_CLOEXEC};
    int rc = ioctl(heap, DMA_HEAP_IOCTL_ALLOC, &allocation);
    close(heap);
    if (rc < 0) return -1;
    int fd = (int)allocation.fd;
    uint8_t *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) { close(fd); return -1; }
    struct dma_buf_sync sync = {.flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_WRITE};
    rc = ioctl(fd, DMA_BUF_IOCTL_SYNC, &sync);
    if (!rc) {
        // Solid optical acquisition target. The panel's FOD command supplies
        // local illumination; no global backlight changes are made here.
        for (uint32_t row = 0; row < height; row++) {
            uint32_t *line = (uint32_t *)(pixels + row * stride);
            for (uint32_t col = 0; col < width; col++) {
                int dx = 2 * (int)col + 1 - (int)width;
                int dy = 2 * (int)row + 1 - (int)height;
                line[col] = dx * dx + dy * dy <= (int)(width * width) ? 0xffffffff : 0;
            }
        }
        sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_WRITE;
        rc = ioctl(fd, DMA_BUF_IOCTL_SYNC, &sync);
    }
    munmap(pixels, size);
    if (!rc) {
        struct zwp_linux_buffer_params_v1 *params = zwp_linux_dmabuf_v1_create_params(dmabuf);
        zwp_linux_buffer_params_v1_add(params, fd, 0, 0, stride, 0, 0); // DRM_FORMAT_MOD_LINEAR
        buffer = zwp_linux_buffer_params_v1_create_immed(params, (int32_t)width,
                                                       (int32_t)height, ARGB8888, 0);
        zwp_linux_buffer_params_v1_destroy(params);
        wl_buffer_add_listener(buffer, &buffer_listener, NULL);
    }
    close(fd);
    return rc;
}

static int open_inputs(void) {
    DIR *directory=opendir("/dev/input");
    if (!directory) return -1;
    struct dirent *entry;
    while ((entry=readdir(directory))) {
        if (strncmp(entry->d_name,"event",5)) continue;
        int fd=openat(dirfd(directory),entry->d_name,O_RDONLY|O_NONBLOCK|O_CLOEXEC|O_NOFOLLOW);
        if (fd<0) continue;
        char name[128]={0};struct input_id id;
        if (ioctl(fd,EVIOCGNAME(sizeof(name)-1),name)<0 ||
            strcmp(name,"synaptics_tcm_touch") || ioctl(fd,EVIOCGID,&id) || id.bustype!=BUS_SPI) {close(fd);continue;}
        struct input_absinfo slot,ax,ay;
        if (inputs[0]>=0 || ioctl(fd,EVIOCGABS(ABS_MT_SLOT),&slot) ||
            ioctl(fd,EVIOCGABS(ABS_MT_POSITION_X),&ax) || ioctl(fd,EVIOCGABS(ABS_MT_POSITION_Y),&ay) ||
            slot.minimum!=0 || slot.maximum!=9 || slot.value<0 || slot.value>9 ||
            ax.minimum!=0 || ax.maximum!=12639 || ay.minimum!=0 || ay.maximum!=27799) {
            close(fd);closedir(directory);return -1;
        }
        uff_contact_init(&contact,&UFF_CONTACT_ASTON);contact.slot=slot.value;
        /* Snapshot current slots. A finger already held when the worker starts
         * must release before it can authorize a new optical acquisition.
         */
        int values[UFF_CONTACT_SLOTS+1];
        const int axes[]={ABS_MT_TRACKING_ID,ABS_MT_POSITION_X,ABS_MT_POSITION_Y};
        for (unsigned a=0;a<3;a++) {
            values[0]=axes[a];
            if (ioctl(fd,EVIOCGMTSLOTS(sizeof(values)),values)) {close(fd);closedir(directory);return -1;}
            for (unsigned i=0;i<UFF_CONTACT_SLOTS;i++) {
                if (a==0) {contact.slots[i].id=values[i+1];if(values[i+1]>=0)contact.armed=false;}
                else if (a==1) contact.slots[i].x=values[i+1];
                else contact.slots[i].y=values[i+1];
            }
        }
        inputs[0]=fd;
    }
    closedir(directory);
    return inputs[0]>=0?0:-1;
}
static void cancel_contact(void) {
    if (active && !finishing) {
        lost = true;
        if (presented) denial_fingerprint_session_v1_withdraw(session, 1);
        else active=false;
        puts("DFP1 lost 1");
    }
}
static int read_input(int index) {
    struct input_event events[64];
    ssize_t bytes=read(inputs[index],events,sizeof(events));
    if (bytes<0 && errno==EAGAIN) return 0;
    if (bytes<=0 || bytes%sizeof(events[0])) return -1;
    for (size_t i=0;i<(size_t)bytes/sizeof(events[0]);i++) {
        int transition=uff_contact_event(&contact,&events[i]);
        if (transition<0) return -1;
        if (transition==1 && !active && !lost && !stopped && !finishing) {
            active=true;
            struct timespec received;
            clock_gettime(CLOCK_REALTIME,&received);
            double queued_ms=(received.tv_sec-events[i].time.tv_sec)*1000.0+
                received.tv_nsec/1000000.0-events[i].time.tv_usec/1000.0;
            fprintf(stderr,"UFF timing evdev_queue_ms=%.3f\n",queued_ms);
            if (!getenv("UFF_DEFER_PRESENT")) {
                denial_fingerprint_session_v1_present(session,1,buffer,x,y,width,height);
                presented=true;
            }
            puts("DFP1 down 1");
        } else if (transition==2) cancel_contact();
    }
    return 0;
}

/* Suspend keeps the evdev descriptors open but leaves their events queued.
 * A contact spanning kernel resume is handled by the normal DOWN/UP path.
 * Never retain an active illuminated acquisition across system sleep. */
static bool suspended;
static int control_command(const char *command, bool *finish_requested, bool *present_requested) {
    if (!strcmp(command, "suspend") && getenv("UFF_SUSPEND_PROTOCOL")) {
        if (active || lost || stopped || finishing) puts("DFP1 suspend-busy");
        else if (!suspended) { suspended = true; puts("DFP1 suspended"); }
        else return -1;
    } else if (!strcmp(command, "resume") && suspended) {
        suspended = false;
        puts("DFP1 resumed");
    } else if (!strcmp(command, "present") && getenv("UFF_DEFER_PRESENT")) {
        *present_requested = true;
    } else if (!strcmp(command, "finish")) {
        *finish_requested = true;
    } else return -1;
    return 0;
}

int main(void) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    signal(SIGINT, signal_stop); signal(SIGTERM, signal_stop); signal(SIGHUP, signal_stop);
    signal(SIGPIPE, SIG_IGN);
    if (geteuid() != 0) return 2;
    char boot[16384];FILE *identity=fopen("/proc/bootconfig","r");
    if (!identity) return 2;
    size_t identity_size=fread(boot,1,sizeof(boot)-1,identity);boot[identity_size]=0;fclose(identity);
    if (!strstr(boot,"androidboot.serialno = \"f683d9c9\"") ||
        !strstr(boot,"androidboot.slot_suffix = \"_b\"")) return 2;
    // No contact exists during setup. Bound synchronous registry/import
    // roundtrips even if the compositor has stopped dispatching clients.
    alarm(5);
    display = wl_display_connect(NULL);
    if (!display) { perror("fingerprint Wayland connect"); return 2; }
    /* Connect only to the exact system-managed Denial process selected by
     * the root helper, even if another process replaces its socket pathname.
     */
    const char *expected_pid=getenv("UFF_DENIAL_PID");char *end=NULL;
    unsigned long pid=expected_pid?strtoul(expected_pid,&end,10):0;
    struct ucred peer;socklen_t peer_size=sizeof(peer);
    if (!pid || pid>INT32_MAX || !end || *end ||
        getsockopt(wl_display_get_fd(display),SOL_SOCKET,SO_PEERCRED,&peer,&peer_size) ||
        peer_size!=sizeof(peer) || peer.pid!=(pid_t)pid || peer.uid!=1001) goto fail;
    struct wl_registry *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    if (wl_display_roundtrip(display) < 0 || !manager || !dmabuf) goto fail;
    session = denial_fingerprint_manager_v1_get_session(manager, "goodix-g7s-uff");
    denial_fingerprint_session_v1_add_listener(session, &session_listener, NULL);
    int illumination=open("/dev/aston_fp_illumination",O_RDWR|O_CLOEXEC|O_NOFOLLOW);
    if (illumination<0) goto fail;
    denial_fingerprint_session_v1_set_illumination_fd(session,illumination);
    close(illumination);
    if (wl_display_roundtrip(display) < 0 || !configured || !linear_argb || lost || make_image() || open_inputs()) goto fail;
    if (wl_display_roundtrip(display) < 0) goto fail;
    alarm(0);
    puts("DFP1 waiting");
    double cleanup_deadline = 0;
    char control_buffer[32];
    size_t control_length = 0;
    while (true) {
        if (interrupted && !cleanup_deadline) {
            cancel_contact();
            if (!active) break;
            cleanup_deadline = now() + 3;
        }
        if (stopped && released) break;
        if ((lost || finishing) && !cleanup_deadline) cleanup_deadline = now() + 3;
        if (cleanup_deadline && now() >= cleanup_deadline) break;
        while (wl_display_prepare_read(display) != 0) {
            if (wl_display_dispatch_pending(display) < 0) goto fail;
        }
        int flushed = wl_display_flush(display);
        if (flushed < 0 && errno != EAGAIN) { wl_display_cancel_read(display); goto fail; }
        struct pollfd fds[4] = {
            {.fd = wl_display_get_fd(display), .events = POLLIN | (flushed < 0 ? POLLOUT : 0)},
            {.fd = suspended ? -1 : inputs[0], .events = POLLIN},
            {.fd = suspended ? -1 : inputs[1], .events = POLLIN},
            {.fd = STDIN_FILENO, .events = POLLIN},
        };
        int count = poll(fds, 4, 100);
        if (count < 0 && errno != EINTR) { wl_display_cancel_read(display); goto fail; }
        if (count > 0 && (fds[0].revents & POLLIN)) {
            if (wl_display_read_events(display) < 0) goto fail;
        } else wl_display_cancel_read(display);
        if (wl_display_dispatch_pending(display) < 0) goto fail;
        if (count <= 0) continue;
        for (int i = 0; i < 3; i++) if (fds[i].revents & (POLLERR | POLLHUP | POLLNVAL)) goto fail;
        // Pause before reading any simultaneous DOWN. On resume the same
        // descriptors deliver the buffered physical contact, including UP.
        bool finish_requested = false, present_requested = false;
        if (fds[3].revents & (POLLIN | POLLHUP)) {
            ssize_t n = read(STDIN_FILENO, control_buffer + control_length,
                             sizeof(control_buffer) - 1 - control_length);
            if (n <= 0) interrupted = 1;
            else {
                control_length += (size_t)n;
                control_buffer[control_length] = '\0';
                char *newline;
                while ((newline = memchr(control_buffer, '\n', control_length))) {
                    *newline = '\0';
                    if (control_command(control_buffer, &finish_requested, &present_requested)) interrupted = 1;
                    size_t consumed = (size_t)(newline - control_buffer) + 1;
                    memmove(control_buffer, control_buffer + consumed, control_length - consumed);
                    control_length -= consumed;
                }
                if (control_length == sizeof(control_buffer) - 1) interrupted = 1;
            }
        }
        // Process UP before applying finish so early release cannot validate
        // acquisition. A queued DOWN+UP still cancels rather than matching.
        for (int i = 0; i < 2; i++)
            if (!suspended && !interrupted && (fds[i + 1].revents & POLLIN) && read_input(i)) goto fail;
        if (present_requested) {
            if (!suspended && active && !presented && !lost && !finishing) {
                presented=true;
                denial_fingerprint_session_v1_present(session,1,buffer,x,y,width,height);
            } else interrupted=1;
        }
        if (finish_requested) {
            if (!suspended && ready && active && !lost && !finishing) {
                finishing = true;
                denial_fingerprint_session_v1_withdraw(session, 1);
            } else {
                interrupted = 1;
            }
        }
    }
    if (session) denial_fingerprint_session_v1_destroy(session);
    wl_display_flush(display);
    wl_display_disconnect(display);
    for (int i = 0; i < 2; i++) if (inputs[i] >= 0) close(inputs[i]);
    return finishing && stopped && released && !lost && !interrupted ? 0 : 2;
fail:
    fputs("DFP1 unavailable\n", stderr);
    // Disconnect is also a compositor-enforced cancellation with a deadline.
    wl_display_disconnect(display);
    for (int i = 0; i < 2; i++) if (inputs[i] >= 0) close(inputs[i]);
    return 2;
}
