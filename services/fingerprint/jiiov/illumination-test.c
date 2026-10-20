/* SPDX-License-Identifier: LGPL-2.1-or-later */
/* Screen-on optical diagnostic only: requests a white fullscreen surface for
 * at most 30 seconds. No panel power/HBM commands, authentication, or locking.
 * Launch only after the user authorizes this visible test and unlocks the phone.
 * A frame callback is not a production fingerprint illumination-ready signal.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"
#include "denial-insets-client-protocol.h"
#include "diagnostic-ui.h"

static volatile sig_atomic_t stopped;
static struct wl_compositor *compositor;
static struct wl_shm *shm;
static struct xdg_wm_base *shell;
static struct denial_insets_manager_v1 *insets;
static struct wl_surface *surface;
static struct wl_buffer *buffer;
static int width=1220, height=2712;
static int buffer_width, buffer_height;
static int failed;
static int target_mode, duration=30;
static const char *progress_path;
static char last_phase[16]="";
static int last_remaining=-1;
static struct canvas_buffer {
    struct wl_buffer *buffer;
    uint32_t *pixels;
    size_t size;
    int busy;
} canvases[2];

static void stop(int signal_number) { (void)signal_number; stopped=1; }
static void ping(void *data, struct xdg_wm_base *base, uint32_t serial)
{ (void)data; xdg_wm_base_pong(base,serial); }
static const struct xdg_wm_base_listener shell_listener={.ping=ping};

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version)
{
    (void)data; (void)version;
    if (!strcmp(interface,"wl_compositor"))
        compositor=wl_registry_bind(registry,name,&wl_compositor_interface,1);
    else if (!strcmp(interface,"wl_shm"))
        shm=wl_registry_bind(registry,name,&wl_shm_interface,1);
    else if (!strcmp(interface,"xdg_wm_base")) {
        shell=wl_registry_bind(registry,name,&xdg_wm_base_interface,1);
        xdg_wm_base_add_listener(shell,&shell_listener,NULL);
    } else if (!strcmp(interface,"denial_insets_manager_v1")) {
        insets=wl_registry_bind(registry,name,&denial_insets_manager_v1_interface,1);
    }
}
static void global_remove(void *data, struct wl_registry *registry, uint32_t name)
{ (void)data; (void)registry; (void)name; }
static const struct wl_registry_listener registry_listener={global,global_remove};

static void frame_done(void *data, struct wl_callback *callback, uint32_t time)
{
    (void)data; (void)time;
    wl_callback_destroy(callback);
    puts("First frame callback received; panel illumination readiness is unverified.");
}
static const struct wl_callback_listener frame_listener={frame_done};

static void released(void *data, struct wl_buffer *b)
{ (void)b; ((struct canvas_buffer *)data)->busy=0; }
static const struct wl_buffer_listener buffer_listener={.release=released};

static void redraw(int first)
{
    char phase[16]="PLACE";
    int remaining=20;
    if (progress_path) {
        FILE *f=fopen(progress_path,"re");
        if (f) {
            if (fscanf(f,"%15s %d",phase,&remaining)!=2) { fclose(f); return; }
            fclose(f);
        }
    }
    if (!first && !strcmp(phase,last_phase) && remaining==last_remaining) return;
    struct canvas_buffer *c=NULL;
    for (unsigned i=0;i<2;i++) if (!canvases[i].busy) { c=&canvases[i]; break; }
    if (!c) return;
    if (target_mode) diagnostic_canvas(c->pixels,width,height,phase,remaining);
    else for (size_t i=0;i<c->size/4;i++) c->pixels[i]=0xffffffff;
    strcpy(last_phase,phase); last_remaining=remaining;
    if (first) {
        struct wl_callback *callback=wl_surface_frame(surface);
        wl_callback_add_listener(callback,&frame_listener,NULL);
    }
    c->busy=1;
    wl_surface_attach(surface,c->buffer,0,0);
    wl_surface_damage(surface,0,0,width,height);
    wl_surface_commit(surface);
}

static void configure(void *data, struct xdg_surface *xdg, uint32_t serial)
{
    (void)data;
    xdg_surface_ack_configure(xdg,serial);
    /* Activation commonly sends a second configure with unchanged dimensions.
     * Keep that buffer; stop only if a resize would require a new allocation.
     */
    if (buffer) {
        if (width!=buffer_width || height!=buffer_height) stopped=1;
        else wl_surface_commit(surface);
        return;
    }
    if (width<1 || height<1 || width>4096 || height>4096) { failed=1; stopped=1; return; }
    if (target_mode && width*2712!=height*1220) {
        fprintf(stderr,"Target requires full portrait panel geometry; got %dx%d\n",width,height);
        failed=1; stopped=1; return;
    }
    size_t size=(size_t)width*height*4;
    for (unsigned i=0;i<2;i++) {
    int fd=memfd_create("fingerprint-illumination-test",MFD_CLOEXEC);
    if (fd<0) { failed=1; stopped=1; return; }
    if (ftruncate(fd,(off_t)size)) { close(fd); failed=1; stopped=1; return; }
    uint32_t *pixels=mmap(NULL,size,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0);
    if (pixels==MAP_FAILED) { close(fd); failed=1; stopped=1; return; }
    struct wl_shm_pool *pool=wl_shm_create_pool(shm,fd,(int)size);
    buffer=wl_shm_pool_create_buffer(pool,0,width,height,width*4,WL_SHM_FORMAT_XRGB8888);
    buffer_width=width; buffer_height=height;
    wl_shm_pool_destroy(pool);
    canvases[i]=(struct canvas_buffer){buffer,pixels,size,0};
    wl_buffer_add_listener(buffer,&buffer_listener,&canvases[i]);
    close(fd);
    }
    redraw(1);
    printf("Submitted %s diagnostic surface %dx%d; automatic exit within %d seconds.\n",
           target_mode ? "sensor-target" : "white",width,height,duration);
}
static const struct xdg_surface_listener surface_listener={configure};
static void toplevel_configure(void *data, struct xdg_toplevel *top, int32_t w,
                              int32_t h, struct wl_array *states)
{
    (void)data; (void)top; (void)states;
    if (w>0) width=w;
    if (h>0) height=h;
}
static void toplevel_close(void *data, struct xdg_toplevel *top)
{ (void)data; (void)top; stopped=1; }
static const struct xdg_toplevel_listener top_listener={
    .configure=toplevel_configure,.close=toplevel_close,
};

static int dispatch(struct wl_display *display)
{
    while (wl_display_prepare_read(display)!=0) {
        if (wl_display_dispatch_pending(display)<0) return -1;
        if (stopped) return 0;
    }
    int flush=wl_display_flush(display);
    if (flush<0 && errno!=EAGAIN) { wl_display_cancel_read(display); return -1; }
    struct pollfd p={wl_display_get_fd(display),POLLIN,0};
    if (flush<0) p.events|=POLLOUT;
    int result=poll(&p,1,250);
    if (result>0 && p.revents&POLLIN) {
        if (wl_display_read_events(display)<0) return -1;
    } else wl_display_cancel_read(display);
    if (result<0 && errno!=EINTR) return -1;
    if (p.revents&(POLLERR|POLLHUP|POLLNVAL)) return -1;
    return wl_display_dispatch_pending(display);
}

int main(int argc, char **argv)
{
    if (argc==3 && !strcmp(argv[1],"--target")) {
        target_mode=1; duration=120; progress_path=argv[2];
    } else if (argc!=1) return 2;
    setvbuf(stdout,NULL,_IONBF,0);
    struct sigaction action={.sa_handler=stop};
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGALRM,&action,NULL) || sigaction(SIGTERM,&action,NULL) ||
        sigaction(SIGINT,&action,NULL)) return 1;
    alarm(duration);
    struct wl_display *display=wl_display_connect(NULL);
    if (!display) { perror("Wayland connection"); return 1; }
    struct wl_registry *registry=wl_display_get_registry(display);
    wl_registry_add_listener(registry,&registry_listener,NULL);
    while (!stopped) {
        if (!surface && compositor && shm && shell) {
            surface=wl_compositor_create_surface(compositor);
            if (target_mode) {
                if (!insets) { fputs("Denial inset control unavailable\n",stderr); failed=1; break; }
                denial_insets_manager_v1_set_self_managed(insets,surface);
            }
            struct xdg_surface *xdg=xdg_wm_base_get_xdg_surface(shell,surface);
            xdg_surface_add_listener(xdg,&surface_listener,NULL);
            struct xdg_toplevel *top=xdg_surface_get_toplevel(xdg);
            xdg_toplevel_add_listener(top,&top_listener,NULL);
            xdg_toplevel_set_title(top,"Fingerprint optical diagnostic");
            xdg_toplevel_set_app_id(top,"org.droidloom.FingerprintOpticalDiagnostic");
            xdg_toplevel_set_fullscreen(top,NULL);
            wl_surface_commit(surface);
        }
        if (dispatch(display)<0) { failed=1; break; }
        if (buffer && target_mode) redraw(0);
    }
    wl_display_disconnect(display);
    for (unsigned i=0;i<2;i++) if (canvases[i].pixels) munmap(canvases[i].pixels,canvases[i].size);
    puts("Diagnostic surface closed.");
    return failed || !buffer;
}
