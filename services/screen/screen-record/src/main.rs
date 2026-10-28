//! Records the screen of a Denial device into `~/Videos/Screen recordings`.
//!
//! `screen-record` (or `screen-record toggle`) starts a recording, or stops
//! the one running; `screen-record stop` only stops. One recording runs at a
//! time: it holds a lock in `$XDG_RUNTIME_DIR` while it lives, and while it
//! records, `$XDG_RUNTIME_DIR/screen-record` names its process and file.
//! That file goes once the video is finished, so a shell can follow it.
//!
//! `screen-record capture [--size N] [--fps N] -- <ffmpeg output arguments>`
//! captures into ffmpeg as given, for other formats and sizes.

mod capture;
mod v4l2;

use std::error::Error;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The shorter side of a recording.
const SIZE: u32 = 1080;
const FPS: u32 = 60;
/// What the hardware encoder aims at: screen text stays sharp at 1080p60.
const BITRATE: u32 = 20_000_000;

/// Without a hardware encoder, x264 at its fastest preset, which keeps
/// 1080p60 on one big core with room to spare.
const ENCODE: [&str; 10] = [
    "-c:v",
    "libx264",
    "-preset",
    "ultrafast",
    "-crf",
    "20",
    "-threads",
    "1",
    "-pix_fmt",
    "yuv420p",
];

fn main() {
    let mut args = std::env::args().skip(1);
    let result = match args.next().as_deref() {
        None | Some("toggle") => toggle(),
        Some("stop") => stop().map(drop),
        Some("capture") => capture::parse_options(args).and_then(capture::run),
        Some(other) => Err(format!("unknown command {other}; use toggle, stop or capture").into()),
    };
    if let Err(error) = result {
        eprintln!("screen-record: {error}");
        std::process::exit(1);
    }
}

fn runtime_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or("XDG_RUNTIME_DIR is not set")?
        .into())
}

/// The lock a recording holds while it lives, taken without waiting; None
/// when another recording holds it.
fn lock() -> Result<Option<File>, Box<dyn Error>> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(runtime_dir()?.join("screen-record.lock"))?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(None),
        _ => Err(error.into()),
    }
}

/// Stops the running recording; false when none runs.
fn stop() -> Result<bool, Box<dyn Error>> {
    if lock()?.is_some() {
        return Ok(false);
    }
    // The recording writes its state right after taking the lock.
    let state = fs::read_to_string(runtime_dir()?.join("screen-record"))?;
    let pid: libc::pid_t = state
        .lines()
        .next()
        .and_then(|line| line.parse().ok())
        .ok_or("no recording process")?;
    // SAFETY: a signal to the process that holds the lock.
    if unsafe { libc::kill(pid, libc::SIGINT) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(true)
}

fn toggle() -> Result<(), Box<dyn Error>> {
    match lock()? {
        Some(lock) => record(lock),
        None => stop().map(drop),
    }
}

fn record(_lock: File) -> Result<(), Box<dyn Error>> {
    let folder = Path::new(&std::env::var_os("HOME").ok_or("HOME is not set")?)
        .join("Videos/Screen recordings");
    fs::create_dir_all(&folder)?;
    let file = folder.join(format!("Screen recording {}.mp4", now()));
    let state = runtime_dir()?.join("screen-record");
    let written = state.with_extension("new");
    fs::write(
        &written,
        format!("{}\n{}\n", std::process::id(), file.display()),
    )?;
    fs::rename(&written, &state)?;
    let result = capture::run(capture::Options {
        size: SIZE,
        fps: FPS,
        hardware: v4l2::find_h264_encoder().map(|device| capture::Hardware {
            device,
            bitrate: std::env::var("SR_BITRATE").ok().and_then(|b| b.parse().ok()).unwrap_or(BITRATE),
        }),
        codec: ENCODE.iter().map(|arg| (*arg).to_owned()).collect(),
        ffmpeg: vec![
            "-movflags".into(),
            "+faststart".into(),
            "-y".into(),
            file.display().to_string(),
        ],
    });
    // A recording stopped before the screen showed anything leaves no video.
    if fs::metadata(&file).is_ok_and(|meta| meta.len() == 0) {
        let _ = fs::remove_file(&file);
    }
    let _ = fs::remove_file(&state);
    result
}

/// The local time as "2026-10-22 20.31.05".
fn now() -> String {
    // SAFETY: time and localtime_r only fill the values given.
    let tm = unsafe {
        let time = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&time, &mut tm);
        tm
    };
    format!(
        "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
