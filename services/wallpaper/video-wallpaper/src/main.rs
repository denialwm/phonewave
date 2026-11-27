//! Plays a video, looped, as a background layer surface. The hardware
//! decoder writes each frame into a buffer the compositor samples as it is,
//! and a frame goes out only when the compositor asks for one: Denial asks
//! only while it shows the wallpaper, so a covered or dark wallpaper decodes
//! nothing.

mod mp4;
mod v4l2;

use std::collections::VecDeque;
use std::error::Error;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_callback::{self, WlCallback},
    wl_compositor::WlCompositor,
    wl_region::WlRegion,
    wl_registry::WlRegistry,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

const DRM_FORMAT_NV12: u32 = u32::from_le_bytes(*b"NV12");
const DRM_FORMAT_MOD_LINEAR: u64 = 0;
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// A wait for the compositor this long means it stopped showing us.
const UNSEEN_AFTER: Duration = Duration::from_secs(1);

struct App {
    /// NV12 modifiers the compositor takes.
    nv12_modifiers: Vec<u64>,
    /// The surface size the compositor configured.
    size: Option<(u32, u32)>,
    configured: bool,
    /// The compositor asked for the next frame.
    frame_wanted: bool,
    /// Buffers the compositor gave back, by frame index.
    released: Vec<u32>,
    closed: bool,
}

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_dmabuf_v1::Event::Modifier {
            format,
            modifier_hi,
            modifier_lo,
        } = event
            && format == DRM_FORMAT_NV12
        {
            app.nv12_modifiers
                .push(u64::from(modifier_hi) << 32 | u64::from(modifier_lo));
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for App {
    fn event(
        app: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                app.size = Some((width, height));
                app.configured = false;
            }
            zwlr_layer_surface_v1::Event::Closed => app.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for App {
    fn event(
        app: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            app.frame_wanted = true;
        }
    }
}

impl Dispatch<WlBuffer, u32> for App {
    fn event(
        app: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        index: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            app.released.push(*index);
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            eprintln!("video-wallpaper: the compositor refused a frame buffer");
        }
    }
}

delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlRegion);
delegate_noop!(App: ZwlrLayerShellV1);
delegate_noop!(App: WpViewporter);
delegate_noop!(App: WpViewport);

/// The video to play: the first argument, else the `video` line of
/// `$XDG_CONFIG_HOME/denial/video-wallpaper.conf`.
fn video_path() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = std::env::args_os().nth(1) {
        return Ok(PathBuf::from(path));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or("no HOME")?
        .join("denial/video-wallpaper.conf");
    let text = std::fs::read_to_string(&config)
        .map_err(|error| format!("{}: {error}", config.display()))?;
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == "video")
        .map(|(_, value)| PathBuf::from(value.trim().trim_matches('"')))
        .ok_or_else(|| format!("{}: no `video = PATH` line", config.display()).into())
}

/// The part of a `buffer`-sized frame that covers a `surface`-sized area
/// without stretching: x, y, width, height in buffer pixels.
fn cover(visible: [u32; 4], surface: (u32, u32)) -> [f64; 4] {
    let [x, y, w, h] = visible.map(f64::from);
    let (sw, sh) = (f64::from(surface.0.max(1)), f64::from(surface.1.max(1)));
    let scale = (sw / w).max(sh / h);
    let (cw, ch) = (sw / scale, sh / scale);
    [x + (w - cw) / 2.0, y + (h - ch) / 2.0, cw, ch]
}

struct Timer(OwnedFd);

impl Timer {
    fn new() -> std::io::Result<Self> {
        // SAFETY: plain syscall.
        let fd = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: just created.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Fires once after `delay`; None disarms it.
    fn set(&self, delay: Option<Duration>) {
        let mut spec: libc::itimerspec = unsafe { std::mem::zeroed() };
        if let Some(delay) = delay {
            let delay = delay.max(Duration::from_micros(1));
            spec.it_value.tv_sec = delay.as_secs() as _;
            spec.it_value.tv_nsec = delay.subsec_nanos() as _;
        }
        // SAFETY: a valid timer and spec.
        unsafe { libc::timerfd_settime(self.0.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };
    }

    fn drain(&self) {
        let mut expirations = 0u64;
        // SAFETY: reads eight bytes into a u64.
        unsafe { libc::read(self.0.as_raw_fd(), (&raw mut expirations).cast(), 8) };
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("video-wallpaper: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let path = video_path()?;
    let track = mp4::Track::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let codec = match track.codec {
        mp4::Codec::H264 => v4l2::PIX_FMT_H264,
        mp4::Codec::Hevc => v4l2::PIX_FMT_HEVC,
    };
    eprintln!(
        "video-wallpaper: {} ({:?} {}x{}, {} frames, {:.2} fps)",
        path.display(),
        track.codec,
        track.width,
        track.height,
        track.len(),
        1.0 / track.frame_interval.as_secs_f64()
    );

    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<App>(&connection)?;
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ())?;
    let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ())?;
    let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ())?;
    // Version 3 lists formats and modifiers as events.
    let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;

    let mut app = App {
        nv12_modifiers: Vec::new(),
        size: None,
        configured: false,
        frame_wanted: true,
        released: Vec::new(),
        closed: false,
    };
    queue.roundtrip(&mut app)?;
    let modifier = [DRM_FORMAT_MOD_LINEAR, DRM_FORMAT_MOD_INVALID]
        .into_iter()
        .find(|modifier| app.nv12_modifiers.contains(modifier))
        .ok_or("the compositor takes no linear NV12 buffers")?;

    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    // Touches belong to the shell above.
    let no_input = compositor.create_region(&qh, ());
    surface.set_input_region(Some(&no_input));
    let layer = layer_shell.get_layer_surface(
        &surface,
        None,
        Layer::Background,
        "wallpaper".into(),
        &qh,
        (),
    );
    layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
    layer.set_exclusive_zone(-1);
    surface.commit();

    // Bitstream buffers fit the largest sample with its parameter sets.
    let bitstream_size = (track.max_sample * 2 + track.parameter_sets.len() + 4096).max(512 * 1024);
    let mut decoder = v4l2::Decoder::open(codec, track.width, track.height, bitstream_size)?;
    let mut scratch = Vec::new();
    let mut next_sample = 0usize;
    let mut buffers: Vec<WlBuffer> = Vec::new();
    let mut ready: VecDeque<u32> = VecDeque::new();
    let timer = Timer::new()?;
    let mut next_due = Instant::now();
    let mut asked_at: Option<Instant> = None;
    let mut presented = 0u64;

    loop {
        // Feed the decoder: it stops by itself once every frame buffer is
        // decoded ahead or on screen.
        decoder.reclaim_bitstream()?;
        while decoder.has_free_bitstream() {
            let index = next_sample;
            next_sample = (next_sample + 1) % track.len();
            decoder
                .queue_bitstream(|memory| track.annex_b(index, index == 0, &mut scratch, memory))?;
        }
        while let Some(notice) = decoder.next_notice()? {
            match notice {
                v4l2::Notice::SourceChange if decoder.layout.is_none() => {
                    let layout = decoder.setup_frames()?;
                    for (index, frame) in decoder.frames.iter().enumerate() {
                        let params = dmabuf.create_params(&qh, ());
                        let (hi, lo) = ((modifier >> 32) as u32, modifier as u32);
                        params.add(frame.as_fd(), 0, 0, layout.stride, hi, lo);
                        params.add(
                            frame.as_fd(),
                            1,
                            layout.chroma_offset,
                            layout.stride,
                            hi,
                            lo,
                        );
                        let [x, y, w, h] = layout.visible;
                        buffers.push(params.create_immed(
                            (x + w) as i32,
                            (y + h) as i32,
                            DRM_FORMAT_NV12,
                            zwp_linux_buffer_params_v1::Flags::empty(),
                            &qh,
                            index as u32,
                        ));
                        params.destroy();
                    }
                }
                v4l2::Notice::SourceChange => {
                    return Err("the video changed size, which is not handled".into());
                }
                v4l2::Notice::EndOfStream => {}
            }
        }
        while let Some(index) = decoder.next_frame()? {
            ready.push_back(index);
        }
        // A released buffer is never read again until attached again.
        for index in app.released.drain(..) {
            decoder.queue_frame(index)?;
        }

        if app.closed {
            return Ok(());
        }
        if let (Some(size), Some(layout)) = (app.size, decoder.layout)
            && !app.configured
        {
            let [x, y, w, h] = cover(layout.visible, size);
            viewport.set_source(x, y, w, h);
            viewport.set_destination(size.0 as i32, size.1 as i32);
            let opaque = compositor.create_region(&qh, ());
            opaque.add(0, 0, size.0 as i32, size.1 as i32);
            surface.set_opaque_region(Some(&opaque));
            opaque.destroy();
            app.configured = true;
            // A new size shows at once.
            app.frame_wanted = true;
        }

        // Present the next frame when the compositor wants one and its time
        // has come; otherwise wait for whichever is missing.
        let now = Instant::now();
        timer.set(None);
        if app.configured && app.frame_wanted && !ready.is_empty() {
            if now >= next_due {
                let index = ready.pop_front().unwrap();
                if let Some(waited) = asked_at.map(|asked| now.saturating_duration_since(asked))
                    && waited >= UNSEEN_AFTER
                {
                    eprintln!(
                        "video-wallpaper: shown again after {:.1} s, {presented} frames so far",
                        waited.as_secs_f64()
                    );
                }
                surface.attach(Some(&buffers[index as usize]), 0, 0);
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                surface.frame(&qh, ());
                surface.commit();
                presented += 1;
                app.frame_wanted = false;
                asked_at = Some(now);
                // A late frame restarts the clock instead of hurrying the
                // next ones: time stands still while nobody watches.
                next_due = if now.saturating_duration_since(next_due) > track.frame_interval {
                    now + track.frame_interval
                } else {
                    next_due + track.frame_interval
                };
            } else {
                timer.set(Some(next_due - now));
            }
        }

        connection.flush()?;
        if queue.dispatch_pending(&mut app)? > 0 {
            continue;
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let wayland_fd = guard.connection_fd().as_raw_fd();
        let mut fds = [
            libc::pollfd {
                fd: wayland_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: timer.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                // Polling a decoder with nothing queued reports an error at
                // once; it has nothing to say then but events.
                fd: decoder.raw_fd(),
                events: libc::POLLPRI
                    | if decoder.busy() {
                        libc::POLLIN | libc::POLLOUT
                    } else {
                        0
                    },
                revents: 0,
            },
        ];
        // SAFETY: three valid pollfds.
        let polled = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
        if polled < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
        if fds[0].revents & libc::POLLIN != 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            drop(guard);
        }
        if fds[1].revents & libc::POLLIN != 0 {
            timer.drain();
        }
        queue.dispatch_pending(&mut app)?;
    }
}

#[cfg(test)]
mod tests {
    use super::cover;

    #[test]
    fn cover_crops_the_long_side() {
        // A 9:16 video on a 9:20 screen loses its sides.
        let [x, y, w, h] = cover([0, 0, 720, 1280], (432, 960));
        assert!((h - 1280.0).abs() < 1e-9);
        assert!((w - 576.0).abs() < 1e-9);
        assert!((x - 72.0).abs() < 1e-9 && y == 0.0);
    }
}
