//! Records a Denial output without reading full-resolution frames on the CPU.
//!
//! Frames arrive through `ext-image-copy-capture-v1` in DMA-BUFs, so Denial
//! only blits on the GPU. Each frame is box-filtered down to the target size
//! on the GPU, read back at that size and piped to ffmpeg as constant-rate
//! raw video timed by Denial's presentation timestamps.
//!
//! Usage: pad4-capture [--size N] [--fps N] -- <ffmpeg output arguments>
//! `--size` is the shorter side of the recording, so 720 means 720p on a
//! landscape or portrait output.
//! SIGINT or SIGTERM stops capture; ffmpeg then finishes the file.

use std::error::Error;
use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use gbm::{AsRaw, BufferObject, BufferObjectFlags, Format, Modifier};
use glow::HasContext;
use khronos_egl as egl;
use wayland_client::backend::WaylandError;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1 as frame_v1, ext_image_copy_capture_manager_v1,
    ext_image_copy_capture_session_v1 as session_v1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};

const RENDER_NODE: &str = "/dev/dri/renderD128";
const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
/// How often a stopped capture asks Denial again whether the output is on.
const RETRY: Duration = Duration::from_millis(250);

const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31D7;
const EGL_LINUX_DMA_BUF_EXT: egl::Enum = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: egl::Attrib = 0x3271;
/// (fd, offset, pitch, modifier lo, modifier hi) attributes of planes 0 to 3.
const EGL_PLANE_ATTRIBUTES: [[egl::Attrib; 5]; 4] = [
    [0x3272, 0x3273, 0x3274, 0x3443, 0x3444],
    [0x3275, 0x3276, 0x3277, 0x3445, 0x3446],
    [0x3278, 0x3279, 0x327A, 0x3447, 0x3448],
    [0x3440, 0x3441, 0x3442, 0x3449, 0x344A],
];

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

#[derive(Default)]
struct Constraints {
    size: Option<(u32, u32)>,
    modifiers: Vec<u64>,
}

enum FrameResult {
    Ready(Option<u64>),
    Failed(WEnum<frame_v1::FailureReason>),
}

#[derive(Default)]
struct State {
    pending: Constraints,
    constraints: Option<Constraints>,
    constraints_changed: bool,
    stopped: bool,
    presented: Option<u64>,
    frame: Option<FrameResult>,
}

struct Options {
    size: u32,
    fps: u32,
    ffmpeg: Vec<String>,
}

fn parse_options() -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        size: 720,
        fps: 60,
        ffmpeg: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => options.size = args.next().ok_or("--size needs a value")?.parse()?,
            "--fps" => options.fps = args.next().ok_or("--fps needs a value")?.parse()?,
            "--" => {
                options.ffmpeg = args.collect();
                break;
            }
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    if options.ffmpeg.is_empty() || options.fps == 0 || options.size < 2 {
        return Err("usage: pad4-capture [--size N] [--fps N] -- <ffmpeg output arguments>".into());
    }
    Ok(options)
}

fn monotonic_ns() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid timespec for the kernel to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as u64 * 1_000_000_000 + now.tv_nsec as u64
}

/// One capture target: a GBM allocation shared with Denial as a wl_buffer
/// and sampled here through an EGLImage.
struct CaptureBuffer {
    wl_buffer: wl_buffer::WlBuffer,
    image: egl::Image,
    texture: glow::Texture,
    _bo: BufferObject<()>,
}

struct Gpu {
    egl: egl::DynamicInstance<egl::EGL1_5>,
    display: egl::Display,
    gl: glow::Context,
    image_target_texture: extern "system" fn(u32, *const c_void),
    gbm: gbm::Device<File>,
}

const VERTEX_SHADER: &str = r#"#version 300 es
out highp vec2 uv;
void main() {
    vec2 p = vec2(float((gl_VertexID & 1) << 2), float((gl_VertexID & 2) << 1));
    uv = p * 0.5;
    gl_Position = vec4(p - 1.0, 0.0, 1.0);
}
"#;

// Four bilinear taps a quarter output pixel from the centre average a
// footprint about one output pixel wide, which keeps small text legible
// at a 3x reduction where a single tap would alias.
const FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D source;
uniform vec2 tap_offset;
in vec2 uv;
out vec4 colour;
void main() {
    vec3 sum = texture(source, uv + vec2(-tap_offset.x, -tap_offset.y)).rgb
             + texture(source, uv + vec2(tap_offset.x, -tap_offset.y)).rgb
             + texture(source, uv + vec2(-tap_offset.x, tap_offset.y)).rgb
             + texture(source, uv + vec2(tap_offset.x, tap_offset.y)).rgb;
    colour = vec4(sum * 0.25, 1.0);
}
"#;

impl Gpu {
    fn new(width: u32, height: u32) -> Result<Self, Box<dyn Error>> {
        let node = File::options().read(true).write(true).open(RENDER_NODE)?;
        let gbm = gbm::Device::new(node)?;
        // SAFETY: loads the system EGL library; all later calls go through it.
        let egl = unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required()? };
        // SAFETY: the GBM device outlives this display (both live in `Gpu`).
        let display = unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_GBM_KHR,
                gbm.as_raw() as *mut c_void,
                &[egl::ATTRIB_NONE],
            )?
        };
        egl.initialize(display)?;
        egl.bind_api(egl::OPENGL_ES_API)?;
        // EGL_KHR_no_config_context and EGL_KHR_surfaceless_context: all
        // rendering goes to a framebuffer object.
        // SAFETY: a null config is EGL_NO_CONFIG_KHR.
        let config = unsafe { egl::Config::from_ptr(std::ptr::null_mut()) };
        let context = egl.create_context(
            display,
            config,
            None,
            &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE],
        )?;
        egl.make_current(display, None, None, Some(context))?;
        // SAFETY: the context is current and the loader returns its entry points.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |function| function as *const c_void)
            })
        };
        let image_target_texture = egl
            .get_proc_address("glEGLImageTargetTexture2DOES")
            .ok_or("glEGLImageTargetTexture2DOES is unavailable")?;
        // SAFETY: the extension entry point has this GLES signature.
        let image_target_texture: extern "system" fn(u32, *const c_void) =
            unsafe { std::mem::transmute(image_target_texture) };

        // SAFETY: the context is current; every object belongs to it.
        unsafe {
            let program = gl.create_program()?;
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX_SHADER),
                (glow::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(gl.get_shader_info_log(shader).into());
                }
                gl.attach_shader(program, shader);
                gl.delete_shader(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program).into());
            }
            gl.use_program(Some(program));
            let tap_offset = gl
                .get_uniform_location(program, "tap_offset")
                .ok_or("tap_offset uniform missing")?;
            gl.uniform_2_f32(Some(&tap_offset), 0.25 / width as f32, 0.25 / height as f32);
            gl.bind_vertex_array(Some(gl.create_vertex_array()?));

            let target = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(target));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA8, width as i32, height as i32);
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(target),
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("output framebuffer is incomplete".into());
            }
            gl.viewport(0, 0, width as i32, height as i32);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            // The program, framebuffer and vertex array stay bound for the
            // context's lifetime.
            Ok(Self {
                egl,
                display,
                gl,
                image_target_texture,
                gbm,
            })
        }
    }

    fn create_buffer(
        &self,
        constraints: &Constraints,
        dmabuf: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        qh: &QueueHandle<State>,
    ) -> Result<CaptureBuffer, Box<dyn Error>> {
        let (width, height) = constraints.size.ok_or("Denial sent no buffer size")?;
        let bo = if constraints.modifiers.is_empty() {
            self.gbm.create_buffer_object::<()>(
                width,
                height,
                Format::Xrgb8888,
                BufferObjectFlags::RENDERING,
            )?
        } else {
            self.gbm.create_buffer_object_with_modifiers2::<()>(
                width,
                height,
                Format::Xrgb8888,
                constraints
                    .modifiers
                    .iter()
                    .map(|&modifier| Modifier::from(modifier)),
                BufferObjectFlags::RENDERING,
            )?
        };
        let modifier: u64 = bo.modifier().into();
        let planes = bo.plane_count() as usize;
        if planes > EGL_PLANE_ATTRIBUTES.len() {
            return Err(format!("{planes}-plane buffers are unsupported").into());
        }
        let fds = (0..planes)
            .map(|plane| bo.fd_for_plane(plane as i32))
            .collect::<Result<Vec<OwnedFd>, _>>()?;

        let params = dmabuf.create_params(qh, ());
        let mut attributes = vec![
            egl::WIDTH as egl::Attrib,
            width as egl::Attrib,
            egl::HEIGHT as egl::Attrib,
            height as egl::Attrib,
            EGL_LINUX_DRM_FOURCC_EXT,
            XRGB8888 as egl::Attrib,
        ];
        for (plane, fd) in fds.iter().enumerate() {
            let offset = bo.offset(plane as i32);
            let stride = bo.stride_for_plane(plane as i32);
            params.add(
                fd.as_fd(),
                plane as u32,
                offset,
                stride,
                (modifier >> 32) as u32,
                modifier as u32,
            );
            let [fd_key, offset_key, pitch_key, low_key, high_key] = EGL_PLANE_ATTRIBUTES[plane];
            attributes.extend([
                fd_key,
                fd.as_raw_fd() as egl::Attrib,
                offset_key,
                offset as egl::Attrib,
                pitch_key,
                stride as egl::Attrib,
                low_key,
                (modifier & 0xffff_ffff) as egl::Attrib,
                high_key,
                (modifier >> 32) as egl::Attrib,
            ]);
        }
        attributes.push(egl::ATTRIB_NONE);
        let wl_buffer = params.create_immed(
            width as i32,
            height as i32,
            XRGB8888,
            zwp_linux_buffer_params_v1::Flags::empty(),
            qh,
            (),
        );
        params.destroy();

        // SAFETY: EGL dups the plane fds; null context and buffer are what
        // EGL_LINUX_DMA_BUF_EXT requires.
        let image = self.egl.create_image(
            self.display,
            unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) },
            EGL_LINUX_DMA_BUF_EXT,
            unsafe { egl::ClientBuffer::from_ptr(std::ptr::null_mut()) },
            &attributes,
        )?;
        // SAFETY: the context is current and `image` is a live EGLImage.
        let texture = unsafe {
            let texture = self.gl.create_texture()?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (parameter, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                self.gl
                    .tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
            }
            (self.image_target_texture)(glow::TEXTURE_2D, image.as_ptr());
            if self.gl.get_error() != glow::NO_ERROR {
                return Err("could not bind the capture buffer as a texture".into());
            }
            texture
        };
        eprintln!("pad4-capture: capture buffer {width}x{height} modifier {modifier:#x}");
        Ok(CaptureBuffer {
            wl_buffer,
            image,
            texture,
            _bo: bo,
        })
    }

    fn destroy_buffer(&self, buffer: CaptureBuffer) {
        // SAFETY: the context is current; the texture and image are ours.
        unsafe { self.gl.delete_texture(buffer.texture) };
        let _ = self.egl.destroy_image(self.display, buffer.image);
        buffer.wl_buffer.destroy();
    }

    /// Scales `buffer` into the output framebuffer and reads it back.
    fn scale(&self, buffer: &CaptureBuffer, width: u32, height: u32, pixels: &mut [u8]) {
        // SAFETY: the context is current and `pixels` holds width*height RGBA.
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(buffer.texture));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
            self.gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(pixels)),
            );
        }
    }
}

/// Writes frames at a constant rate. A captured frame fills every output tick
/// from its presentation time until the next captured frame. Each tick opens
/// a quarter period before its nominal time, so the refresh a capture aims at
/// is never on a tick boundary, whether the output refreshes at the
/// recording rate or faster.
struct ConstantRate {
    period_ns: u64,
    start_ns: Option<u64>,
    emitted: u64,
    repeated: u64,
}

impl ConstantRate {
    fn tick(&self, time_ns: u64) -> u64 {
        self.start_ns.map_or(0, |start| {
            (time_ns + self.period_ns / 4).saturating_sub(start) / self.period_ns
        })
    }

    /// Whether a frame presented at `time_ns` starts a tick not yet written.
    fn wants(&self, time_ns: u64) -> bool {
        self.start_ns.is_none() || self.tick(time_ns) >= self.emitted
    }

    /// Opening of the first tick not yet written. Denial fills a request on
    /// the first output refresh after it arrives, so a request made here
    /// returns a frame for exactly that tick. At the nominal tick time it
    /// would race the refresh it aims at and, on a 60 Hz output, land on
    /// every second one.
    fn next_request_ns(&self) -> u64 {
        self.start_ns.map_or(0, |start| {
            (start + self.emitted * self.period_ns).saturating_sub(self.period_ns / 4)
        })
    }

    /// Counts the repeats of the current image that fill every tick before
    /// the one containing `time_ns`.
    fn fill(&mut self, time_ns: u64) -> u64 {
        if self.start_ns.is_none() {
            return 0;
        }
        let count = self.tick(time_ns).saturating_sub(self.emitted);
        self.emitted += count;
        self.repeated += count;
        count
    }

    fn push(&mut self, time_ns: u64) {
        self.start_ns.get_or_insert(time_ns);
        self.emitted += 1;
    }
}

enum Encode {
    /// Makes this the image that later writes repeat.
    Show(Vec<u8>),
    /// Writes the current image this many times.
    Write(u64),
}

/// Feeds ffmpeg from its own thread, so encoding never delays a capture
/// request. Images circulate between the two threads instead of being
/// allocated per frame.
struct Encoder {
    messages: SyncSender<Encode>,
    free: Receiver<Vec<u8>>,
    thread: JoinHandle<std::io::Result<()>>,
    stopped: bool,
}

impl Encoder {
    fn start(mut pipe: ChildStdin, frame_bytes: usize) -> Self {
        const IMAGES: usize = 4;
        let (messages, inbox) = sync_channel(16);
        let (recycle, free) = sync_channel(IMAGES);
        for _ in 1..IMAGES {
            recycle
                .send(vec![0u8; frame_bytes])
                .expect("free image queue");
        }
        let thread = std::thread::spawn(move || {
            let mut image = vec![0u8; frame_bytes];
            for message in inbox {
                match message {
                    Encode::Show(next) => {
                        let _ = recycle.send(std::mem::replace(&mut image, next));
                    }
                    Encode::Write(count) => {
                        for _ in 0..count {
                            pipe.write_all(&image)?;
                        }
                    }
                }
            }
            Ok(())
        });
        Self {
            messages,
            free,
            thread,
            stopped: false,
        }
    }

    fn send(&mut self, message: Encode) {
        self.stopped |= self.messages.send(message).is_err();
    }

    /// A free image to draw into, or None once the encoder has stopped.
    fn image(&mut self) -> Option<Vec<u8>> {
        let image = self.free.recv().ok();
        self.stopped |= image.is_none();
        image
    }

    fn show(&mut self, image: Vec<u8>) {
        self.send(Encode::Show(image));
    }

    fn repeat(&mut self, count: u64) {
        if count > 0 {
            self.send(Encode::Write(count));
        }
    }

    /// Writes everything queued, then closes ffmpeg's input.
    fn finish(self) -> std::io::Result<()> {
        drop(self.messages);
        self.thread.join().expect("encoder thread panicked")
    }
}

fn wait_readable(fd: libc::c_int, timeout: Duration) -> bool {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `poll` is one valid pollfd.
    unsafe { libc::poll(&mut poll, 1, timeout.as_millis().clamp(1, 100) as i32) > 0 }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("pad4-capture: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_options()?;
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        let handler = request_stop as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }

    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
    let qh = queue.handle();
    let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    let dmabuf: zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=4, ())?;
    let sources: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let copier: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let source = sources.create_source(&output, &qh, ());
    let open_session = || {
        copier.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &qh,
            (),
        )
    };

    // Denial offers capture only while the output is powered, and stops
    // every session when it powers off.
    let mut state = State::default();
    let mut session = Some(open_session());
    let mut announced = false;
    let constraints = loop {
        queue.blocking_dispatch(&mut state)?;
        if let Some(constraints) = state.constraints.take() {
            break constraints;
        }
        if state.stopped {
            if !std::mem::replace(&mut announced, true) {
                eprintln!("pad4-capture: waiting for the output to power on");
            }
            state.stopped = false;
            session.take().expect("capture session").destroy();
            std::thread::sleep(RETRY);
            if STOP.load(Ordering::Relaxed) {
                return Ok(());
            }
            session = Some(open_session());
        }
    };
    state.constraints_changed = false;
    let (source_width, source_height) = constraints.size.ok_or("Denial sent no buffer size")?;
    let shorter = source_width.min(source_height) as u64;
    let scaled = |side: u32| ((side as u64 * options.size as u64 / shorter) as u32 + 1) & !1;
    let (width, height) = (scaled(source_width), scaled(source_height));
    eprintln!(
        "pad4-capture: {source_width}x{source_height} -> {width}x{height} at {} fps",
        options.fps
    );

    let gpu = Gpu::new(width, height)?;
    let mut buffers = [
        Some(gpu.create_buffer(&constraints, &dmabuf, &qh)?),
        Some(gpu.create_buffer(&constraints, &dmabuf, &qh)?),
    ];
    let mut constraints = constraints;

    let mut ffmpeg = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "warning", "-nostdin"])
        .args(["-f", "rawvideo", "-pixel_format", "rgb0"])
        .args(["-video_size", &format!("{width}x{height}")])
        .args(["-framerate", &options.fps.to_string(), "-i", "pipe:0"])
        .args(&options.ffmpeg)
        .stdin(Stdio::piped())
        // Terminal Ctrl+C reaches only this process; ffmpeg ends at EOF.
        .process_group(0)
        .spawn()?;
    let pipe = ffmpeg.stdin.take().ok_or("ffmpeg has no stdin")?;
    let mut encoder = Encoder::start(pipe, width as usize * height as usize * 4);
    let mut rate = ConstantRate {
        period_ns: 1_000_000_000 / options.fps as u64,
        start_ns: None,
        emitted: 0,
        repeated: 0,
    };
    let mut in_flight: Option<(frame_v1::ExtImageCopyCaptureFrameV1, usize)> = None;
    // A captured buffer, scaled once the next capture has been requested.
    let mut captured_buffer: Option<usize> = None;
    let mut next_buffer = 0;
    let mut captured = 0u64;
    let mut skipped = 0u64;
    let mut failed = 0u64;
    // Time from a refresh to its Ready event, and requests sent after the
    // refresh they aimed at. Either one caps the capture rate.
    let (mut ready_lag_ns, mut ready_lag_max_ns) = (0u64, 0u64);
    let mut late = 0u64;
    let mut retry_ns = 0;
    let mut dark = false;

    while !STOP.load(Ordering::Relaxed) && !encoder.stopped {
        // One capture per output tick: a 120 Hz output is neither copied by
        // Denial nor scaled here twice per written frame. New buffers wait
        // until the captured one has been scaled.
        let next_request_ns = rate.next_request_ns();
        if let Some(live_session) = session.as_ref()
            && !state.stopped
            && in_flight.is_none()
            && monotonic_ns() >= next_request_ns
            && !(state.constraints_changed && captured_buffer.is_some())
        {
            if rate.start_ns.is_some() && monotonic_ns() > next_request_ns + rate.period_ns / 4 {
                late += 1;
            }
            if state.constraints_changed {
                state.constraints_changed = false;
                constraints = state.constraints.take().unwrap_or(constraints);
                for slot in &mut buffers {
                    if let Some(buffer) = slot.take() {
                        gpu.destroy_buffer(buffer);
                    }
                    *slot = Some(gpu.create_buffer(&constraints, &dmabuf, &qh)?);
                }
            }
            let buffer = buffers[next_buffer].as_ref().expect("capture buffer");
            let (buffer_width, buffer_height) = constraints.size.expect("buffer size");
            let frame = live_session.create_frame(&qh, ());
            frame.attach_buffer(&buffer.wl_buffer);
            frame.damage_buffer(0, 0, buffer_width as i32, buffer_height as i32);
            frame.capture();
            in_flight = Some((frame, next_buffer));
            next_buffer ^= 1;
        }

        if let Some(index) = captured_buffer.take() {
            let Some(mut image) = encoder.image() else {
                break;
            };
            gpu.scale(
                buffers[index].as_ref().expect("capture buffer"),
                width,
                height,
                &mut image,
            );
            encoder.show(image);
            encoder.repeat(1);
        }

        if state.stopped {
            // The output powered off. The recording shows black until a new
            // session gets frames again.
            state.stopped = false;
            if let Some((frame, _)) = in_flight.take() {
                frame.destroy();
            }
            state.frame = None;
            session.take().expect("capture session").destroy();
            encoder.repeat(rate.fill(monotonic_ns()));
            let Some(mut black) = encoder.image() else {
                break;
            };
            black.fill(0);
            encoder.show(black);
            retry_ns = monotonic_ns() + RETRY.as_nanos() as u64;
            if !std::mem::replace(&mut dark, true) {
                eprintln!("pad4-capture: output off, recording black");
            }
        }
        if session.is_none() {
            if monotonic_ns() >= retry_ns {
                session = Some(open_session());
            } else {
                encoder.repeat(rate.fill(monotonic_ns()));
                std::thread::sleep(RETRY);
                continue;
            }
        }

        connection.flush()?;
        if let Some(guard) = queue.prepare_read() {
            let wait = if in_flight.is_some() {
                Duration::from_millis(100)
            } else {
                Duration::from_nanos(next_request_ns.saturating_sub(monotonic_ns()))
            };
            if wait_readable(guard.connection_fd().as_raw_fd(), wait) {
                match guard.read() {
                    Err(WaylandError::Io(io)) if io.kind() == std::io::ErrorKind::WouldBlock => {}
                    result => {
                        result?;
                    }
                }
            }
        }
        queue.dispatch_pending(&mut state)?;

        let Some(result) = state.frame.take() else {
            continue;
        };
        let (frame, index) = in_flight.take().expect("frame result without a frame");
        frame.destroy();
        match result {
            FrameResult::Ready(presented) => {
                captured += 1;
                if std::mem::take(&mut dark) {
                    eprintln!("pad4-capture: output on, recording again");
                }
                let time = presented.unwrap_or_else(monotonic_ns);
                let ready_lag = monotonic_ns().saturating_sub(time);
                ready_lag_ns += ready_lag;
                ready_lag_max_ns = ready_lag_max_ns.max(ready_lag);
                if !rate.wants(time) {
                    skipped += 1;
                    continue;
                }
                encoder.repeat(rate.fill(time));
                rate.push(time);
                captured_buffer = Some(index);
            }
            FrameResult::Failed(WEnum::Value(frame_v1::FailureReason::Stopped)) => {
                state.stopped = true;
            }
            FrameResult::Failed(reason) => {
                failed += 1;
                if failed % 60 == 1 {
                    eprintln!("pad4-capture: frame failed: {reason:?}");
                }
            }
        }
    }

    // Hold the last image until the stop request so a still ending keeps
    // its length.
    if let Some(index) = captured_buffer.take()
        && let Some(mut image) = encoder.image()
    {
        gpu.scale(
            buffers[index].as_ref().expect("capture buffer"),
            width,
            height,
            &mut image,
        );
        encoder.show(image);
        encoder.repeat(1);
    }
    encoder.repeat(rate.fill(monotonic_ns()));
    let written = encoder.finish();
    let status = ffmpeg.wait()?;
    eprintln!(
        "pad4-capture: {captured} captured ({skipped} early), {} written ({} repeated), {failed} failed",
        rate.emitted, rate.repeated
    );
    eprintln!(
        "pad4-capture: ready {:.1} ms after refresh (max {:.1}), {late} late requests",
        ready_lag_ns as f64 / captured.max(1) as f64 / 1e6,
        ready_lag_max_ns as f64 / 1e6
    );
    written?;
    if !status.success() {
        return Err(format!("ffmpeg exited with {status}").into());
    }
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<session_v1::ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &session_v1::ExtImageCopyCaptureSessionV1,
        event: session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            session_v1::Event::BufferSize { width, height } => {
                state.pending.size = Some((width, height));
            }
            session_v1::Event::DmabufFormat { format, modifiers } if format == XRGB8888 => {
                state.pending.modifiers = modifiers
                    .chunks_exact(8)
                    .map(|bytes| u64::from_ne_bytes(bytes.try_into().expect("8 bytes")))
                    .collect();
            }
            session_v1::Event::Done => {
                state.constraints = Some(std::mem::take(&mut state.pending));
                state.constraints_changed = true;
            }
            session_v1::Event::Stopped => state.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<frame_v1::ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &frame_v1::ExtImageCopyCaptureFrameV1,
        event: frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            frame_v1::Event::PresentationTime {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                state.presented = Some(seconds * 1_000_000_000 + u64::from(tv_nsec));
            }
            frame_v1::Event::Ready => {
                state.frame = Some(FrameResult::Ready(state.presented.take()));
            }
            frame_v1::Event::Failed { reason } => {
                state.presented = None;
                state.frame = Some(FrameResult::Failed(reason));
            }
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($($interface:ty),* $(,)?) => {$(
        impl Dispatch<$interface, ()> for State {
            fn event(
                _: &mut Self,
                _: &$interface,
                _: <$interface as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignore_events!(
    wl_output::WlOutput,
    wl_buffer::WlBuffer,
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
);
