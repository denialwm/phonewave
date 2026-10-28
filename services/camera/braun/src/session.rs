//! One client's session, on a thread of its own: the camera it has open,
//! the preview rendered from every frame, auto exposure, white balance and
//! the focus sweep on it, and the bursts its photos are developed from.
//!
//! One camera streams at a time. The session that opens one last takes it:
//! the one streaming hears `Failed(0, ..)` and closes.

use std::fs::File;
use std::io::Write;
use std::ops::RangeInclusive;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::camera::{self, Camera, Lens};
use crate::device::Device;
use crate::isp::{self, Exposure, Preview, Rendering, Tone};
use crate::photo::{self, Group};
use crate::proto::{self, Reply, Request, Shared};

/// Frames the sensor takes to apply new exposure controls, plus one.
const SETTLE: usize = 3;
/// Auto exposure looks again after this many frames, once the last change
/// has landed.
const AE_EVERY: usize = 3;
/// The burst, relative to the exposure auto exposure chose: a group longer
/// for the shadows, a shorter one, and a much shorter one for the lights.
const BRACKET: [(f32, usize); 3] = [(2.0, 8), (0.25, 4), (1.0 / 16.0, 4)];
/// The focus sweep: coarse over the whole travel, then fine around the
/// best coarse position. The steps are for a travel of `TRAVEL` positions
/// and scale with the lens's.
const COARSE: u32 = 80;
const FINE: u32 = 16;
const TRAVEL: u32 = 1023;
/// Preview slots: one being written, one with the client, one spare.
const SLOTS: usize = 3;

/// Held by the session streaming.
static HARDWARE: Mutex<()> = Mutex::new(());
/// The session that should be streaming.
static WANTED: AtomicU64 = AtomicU64::new(0);
static SESSIONS: AtomicU64 = AtomicU64::new(1);
/// The cameras last found.
static FOUND: Mutex<Option<Arc<Vec<Arc<Camera>>>>> = Mutex::new(None);

/// The cameras there are. They are found once, and again when asked to
/// `refresh` or when none were: the camera drivers may load after braun.
pub fn cameras(refresh: bool) -> Arc<Vec<Arc<Camera>>> {
    let mut found = FOUND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cameras) = found
        .as_ref()
        .filter(|cameras| !refresh && !cameras.is_empty())
    {
        return cameras.clone();
    }
    let discovery = camera::discover();
    for camera in &discovery.cameras {
        eprintln!(
            "braun: {} ({}, {}): {}x{} {:?}, {:.3} us lines, {}..{} lines, gain to {}x{}{}",
            camera.id,
            camera.facing.name(),
            camera.role.map_or("no role", |role| role.name()),
            camera.width,
            camera.height,
            camera.bayer,
            camera.line_us,
            camera.min_lines,
            camera.max_lines,
            camera.max_gain,
            if camera.lens.is_some() { ", focus" } else { "" },
            if camera.tuned { ", tuned" } else { "" },
        );
        for note in &camera.notes {
            eprintln!("braun: {}: {note}", camera.id);
        }
    }
    for skipped in &discovery.skipped {
        eprintln!("braun: skipped {skipped}");
    }
    let cameras = Arc::new(
        discovery
            .cameras
            .into_iter()
            .map(Arc::new)
            .collect::<Vec<_>>(),
    );
    *found = Some(cameras.clone());
    cameras
}

fn find(id: &str) -> Option<Arc<Camera>> {
    let have = |cameras: &[Arc<Camera>]| cameras.iter().find(|camera| camera.id == id).cloned();
    have(&cameras(false)).or_else(|| have(&cameras(true)))
}

/// Serves the client at the other end of `socket` until it leaves.
pub fn start(socket: OwnedFd) -> std::io::Result<()> {
    let session = Session {
        id: SESSIONS.fetch_add(1, Ordering::Relaxed),
        socket: Arc::new(socket),
        zoom: 1.0,
    };
    std::thread::Builder::new()
        .name(format!("session {}", session.id))
        .spawn(move || session.run())
        .map(drop)
}

struct Sweep {
    x: f32,
    y: f32,
    positions: Vec<u32>,
    next: usize,
    /// Frames to skip before the next measurement.
    wait: usize,
    scores: Vec<(u32, f32)>,
    fine: bool,
    travel: RangeInclusive<u32>,
    coarse_step: u32,
    fine_step: u32,
}

impl Sweep {
    fn coarse(x: f32, y: f32, lens: &Lens) -> Self {
        let span = lens.travel.end() - lens.travel.start();
        let coarse_step = (COARSE * span / TRAVEL).max(1);
        Self {
            x,
            y,
            positions: lens.travel.clone().step_by(coarse_step as usize).collect(),
            next: 0,
            wait: 0,
            scores: Vec::new(),
            fine: false,
            travel: lens.travel.clone(),
            coarse_step,
            fine_step: (FINE * span / TRAVEL).max(1),
        }
    }
}

/// Why streaming stopped.
enum Stop {
    Open(String),
    Close,
    Gone,
}

struct Session {
    id: u64,
    socket: Arc<OwnedFd>,
    zoom: f32,
}

/// Sends `reply` on `socket`; a client that left hears nothing.
fn tell(socket: &OwnedFd, reply: &Reply) {
    let _ = proto::send(socket.as_fd(), &reply.encode(), &[]);
}

impl Session {
    /// The client's next request, or `None` once it has left. Without
    /// `wait`, `Ok(None)` when it has said nothing.
    fn next(&self, wait: bool) -> Result<Option<(Request, Vec<OwnedFd>)>, ()> {
        loop {
            match proto::recv(self.socket.as_fd(), wait) {
                Ok(Some((bytes, fds))) => match Request::decode(&bytes) {
                    Some(request) => return Ok(Some((request, fds))),
                    None => eprintln!("braun: session {}: a message braun does not know", self.id),
                },
                Ok(None) => return Ok(None),
                Err(_) => return Err(()),
            }
        }
    }

    fn run(mut self) {
        let mut open = None;
        loop {
            let request = match open.take() {
                Some(id) => Request::Open(id),
                None => match self.next(true) {
                    Ok(Some((request, _))) => request,
                    Ok(None) => continue,
                    Err(()) => return,
                },
            };
            match request {
                Request::Open(id) => match self.stream(&id) {
                    Ok(Stop::Open(next)) => open = Some(next),
                    Ok(Stop::Close) => {}
                    Ok(Stop::Gone) => return,
                    Err(error) => {
                        eprintln!("braun: session {}: {error}", self.id);
                        tell(&self.socket, &Reply::Failed(0, error));
                    }
                },
                Request::Zoom(zoom) => self.zoom = zoom,
                Request::Shoot(shot) => tell(
                    &self.socket,
                    &Reply::Failed(shot, "no camera is open".into()),
                ),
                Request::Close | Request::Focus(..) | Request::Release(_) => {}
            }
        }
    }

    /// Streams camera `id` until the client asks for another, closes it or
    /// leaves, or another session takes the camera.
    fn stream(&mut self, id: &str) -> Result<Stop, String> {
        WANTED.store(self.id, Ordering::SeqCst);
        let _hardware = HARDWARE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let camera = find(id).ok_or_else(|| format!("no camera {id}"))?;
        let failed = |error: std::io::Error| format!("{}: {error}", camera.name);
        let mut device = Device::open(camera.clone()).map_err(failed)?;
        let mut exposure = Exposure::of(20_000.0, &camera);
        device
            .expose(exposure.lines, exposure.gain)
            .map_err(failed)?;
        let mut rendering = Rendering::neutral(&camera.colour);
        let mut sweep = camera
            .lens
            .as_ref()
            .map(|lens| Sweep::coarse(0.5, 0.5, lens));
        let mut focus = camera.lens.as_ref().map_or(0, |lens| lens.start);
        device.focus(focus).map_err(failed)?;

        let slot_bytes = isp::largest_preview(&camera);
        let mut shared = Shared::create(SLOTS * slot_bytes).map_err(failed)?;
        let mut held = [false; SLOTS];
        let (width, height) = camera.upright();
        let opened = Reply::Opened {
            width,
            height,
            lens: camera.lens.is_some(),
            slots: SLOTS as u32,
            slot_bytes: slot_bytes as u32,
        };
        if proto::send(self.socket.as_fd(), &opened.encode(), &[shared.fd()]).is_err() {
            return Ok(Stop::Gone);
        }
        let tone = Tone::new();
        let mut since_exposure = 0;
        let mut sequence = 0u64;
        loop {
            if WANTED.load(Ordering::SeqCst) != self.id {
                tell(
                    &self.socket,
                    &Reply::Failed(0, "another app took the camera".into()),
                );
                return Ok(Stop::Close);
            }
            loop {
                let (request, fds) = match self.next(false) {
                    Ok(Some(next)) => next,
                    Ok(None) => break,
                    Err(()) => return Ok(Stop::Gone),
                };
                match request {
                    Request::Open(next) if next != id => return Ok(Stop::Open(next)),
                    Request::Open(_) => {}
                    Request::Close => return Ok(Stop::Close),
                    Request::Zoom(zoom) => self.zoom = zoom,
                    Request::Focus(x, y) => {
                        if let Some(lens) = &camera.lens {
                            sweep = Some(Sweep::coarse(x, y, lens));
                        }
                    }
                    Request::Release(slot) => {
                        if let Some(held) = held.get_mut(slot as usize) {
                            *held = false;
                        }
                    }
                    Request::Shoot(shot) => {
                        self.shoot(shot, fds, &mut device, exposure, &rendering)
                            .map_err(failed)?;
                        // Back to the preview's exposure.
                        device
                            .expose(exposure.lines, exposure.gain)
                            .map_err(failed)?;
                        since_exposure = 0;
                    }
                }
            }

            let frame = device.frame().map_err(failed)?;
            let (preview, stats) = isp::preview(
                frame.data,
                frame.stride,
                &camera,
                &rendering,
                exposure.digital,
                self.zoom,
                &tone,
            );
            since_exposure += 1;
            if since_exposure >= AE_EVERY
                && let Some(next) = isp::meter(&stats, exposure, &camera)
            {
                exposure = next;
                device
                    .expose(exposure.lines, exposure.gain)
                    .map_err(failed)?;
                since_exposure = 0;
            }
            isp::balance(&stats, &mut rendering, &camera.colour, exposure.digital);
            if let Some(active) = &mut sweep
                && step_sweep(active, &preview, &device, &mut focus).map_err(failed)?
            {
                sweep = None;
            }
            // A client still reading every slot misses this frame.
            if let Some(slot) = held.iter().position(|held| !held)
                && shared.write(slot * slot_bytes, &preview.bgra)
            {
                held[slot] = true;
                sequence += 1;
                let frame = Reply::Frame {
                    slot: slot as u32,
                    width: preview.width as u32,
                    height: preview.height as u32,
                    sequence,
                };
                if proto::send(self.socket.as_fd(), &frame.encode(), &[]).is_err() {
                    return Ok(Stop::Gone);
                }
            }
        }
    }

    /// Takes photo `shot`: keeps the raw frame if asked, takes the burst,
    /// and develops it on a thread of its own into the client's file.
    fn shoot(
        &self,
        shot: u32,
        fds: Vec<OwnedFd>,
        device: &mut Device,
        exposure: Exposure,
        rendering: &Rendering,
    ) -> std::io::Result<()> {
        let mut files = fds.into_iter().map(File::from);
        let Some(jpeg) = files.next() else {
            tell(
                &self.socket,
                &Reply::Failed(shot, "no file for the photo".into()),
            );
            return Ok(());
        };
        if let (Some(raw), Some(notes)) = (files.next(), files.next()) {
            keep_raw(device, raw, notes, exposure, rendering)?;
        }
        let groups = burst(device, exposure)?;
        tell(&self.socket, &Reply::Captured(shot));
        let camera = device.camera.clone();
        let stride = device.stride();
        let socket = self.socket.clone();
        let rendering = *rendering;
        let base = exposure.on_sensor(&camera);
        let zoom = self.zoom;
        std::thread::spawn(move || {
            let reply = match photo::develop(
                &groups,
                &camera,
                stride,
                base,
                exposure.digital,
                zoom,
                &rendering,
                jpeg,
            ) {
                Ok(()) => Reply::Saved(shot),
                Err(error) => Reply::Failed(shot, format!("developing the photo: {error}")),
            };
            tell(&socket, &reply);
        });
        Ok(())
    }
}

/// Measures the frame for the sweep and moves the lens on; true once the
/// lens is at the sharpest position found.
fn step_sweep(
    sweep: &mut Sweep,
    preview: &Preview,
    device: &Device,
    focus: &mut u32,
) -> std::io::Result<bool> {
    if sweep.wait > 0 {
        sweep.wait -= 1;
        return Ok(false);
    }
    if sweep.next > 0 {
        let at = sweep.positions[sweep.next - 1];
        sweep
            .scores
            .push((at, isp::sharpness(preview, sweep.x, sweep.y)));
    }
    if sweep.next == sweep.positions.len() {
        let best = sweep
            .scores
            .iter()
            .fold((sweep.positions[0], f32::MIN), |a, &b| {
                if b.1 > a.1 { b } else { a }
            })
            .0;
        if sweep.fine {
            *focus = best;
            device.focus(best)?;
            return Ok(true);
        }
        sweep.fine = true;
        let start = best
            .saturating_sub(sweep.coarse_step / 2)
            .max(*sweep.travel.start());
        let end = (best + sweep.coarse_step / 2).min(*sweep.travel.end());
        sweep.positions = (start..=end).step_by(sweep.fine_step as usize).collect();
        sweep.scores.clear();
        sweep.next = 0;
    }
    let at = sweep.positions[sweep.next];
    device.focus(at)?;
    sweep.next += 1;
    // The frame being exposed while the lens moves is soft; measure the next.
    sweep.wait = 1;
    Ok(false)
}

/// Takes the bracketed burst around `exposure`.
fn burst(device: &mut Device, exposure: Exposure) -> std::io::Result<Vec<Group>> {
    let camera = device.camera.clone();
    let mut groups = Vec::new();
    for (factor, count) in BRACKET {
        let group_exposure = Exposure::of(exposure.on_sensor(&camera) * factor, &camera);
        device.expose(group_exposure.lines, group_exposure.gain)?;
        for _ in 0..SETTLE {
            device.frame()?;
        }
        let mut frames = Vec::with_capacity(count);
        for _ in 0..count {
            frames.push(device.frame()?.data.to_vec());
        }
        groups.push(Group {
            exposure: group_exposure.on_sensor(&camera),
            frames,
        });
    }
    Ok(groups)
}

/// Writes the raw frame the preview shows to `raw` (packed RAW10 as the
/// sensor sends it) and how it was exposed and rendered to `notes`.
fn keep_raw(
    device: &mut Device,
    mut raw: File,
    mut notes: File,
    exposure: Exposure,
    rendering: &Rendering,
) -> std::io::Result<()> {
    let camera = device.camera.clone();
    let stride = device.stride();
    let frame = device.frame()?;
    raw.write_all(frame.data)?;
    let text = format!(
        "sensor {}\nwidth {}\nheight {}\nstride {}\nbayer {:?}\nblack {}\nlines {}\ngain {}\ndigital {}\nexposure_us {}\nwb {} {} {}\nlight {}\nct {}\nccm {}\n",
        camera.name,
        camera.width,
        camera.height,
        stride,
        camera.bayer,
        camera.black,
        exposure.lines,
        exposure.gain,
        exposure.digital,
        exposure.lines as f32 * camera.line_us,
        rendering.wb[0],
        rendering.wb[1],
        rendering.wb[2],
        rendering.light,
        rendering.ct,
        rendering.ccm.map(|value| value.to_string()).join(" "),
    );
    notes.write_all(text.as_bytes())
}
