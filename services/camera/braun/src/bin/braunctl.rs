//! braunctl: what braun finds, and a preview frame or a photo through it.

use std::os::fd::AsFd;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use braun::camera;
use braun::client::{self, Session};
use braun::proto::{Reply, Request};

const USAGE: &str = "usage:
  braunctl list               the cameras, found here without the daemon
  braunctl cameras            the cameras braun offers
  braunctl preview ID FILE    one preview frame, as a PPM
  braunctl shot ID FILE       a photo, as a JPEG";

/// Frames to stream before a preview or a photo, so exposure, white
/// balance and focus have settled.
const SETTLE_FRAMES: u32 = 60;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["list"] => {
            list();
            Ok(())
        }
        ["cameras"] => cameras(),
        ["preview", id, file] => preview(id, file),
        ["shot", id, file] => shot(id, file),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("braunctl: {error}");
            ExitCode::FAILURE
        }
    }
}

fn list() {
    let discovery = camera::discover();
    for camera in &discovery.cameras {
        println!("{} ({})", camera.id, camera.entity);
        println!(
            "  {} camera{}{}",
            camera.facing.name(),
            camera
                .role
                .map_or(String::new(), |role| format!(", {}", role.name())),
            camera.zoom.map_or(String::new(), |zoom| format!(
                ", sees as the main camera at {zoom}x"
            ))
        );
        println!(
            "  {}x{} {:?} through {}, turned {} clockwise{}",
            camera.width,
            camera.height,
            camera.bayer,
            camera.phy,
            camera.rotation,
            if camera.mirror { ", mirrored" } else { "" }
        );
        println!(
            "  lines of {:.3} us, {} to {} by {}",
            camera.line_us, camera.min_lines, camera.max_lines, camera.line_step
        );
        println!("  black {}, clipped from {}", camera.black, camera.clip);
        match camera.gain {
            Some(model) => println!(
                "  gain codes {}..={} ({}x to {}x), auto exposure to {}x",
                camera.gain_codes.start(),
                camera.gain_codes.end(),
                model.gain(*camera.gain_codes.start()),
                model.gain(*camera.gain_codes.end()),
                camera.max_gain
            ),
            None => println!("  gain not known"),
        }
        match &camera.lens {
            Some(lens) => println!(
                "  lens {}: sweeps {}..={}, starts at {}",
                lens.entity,
                lens.travel.start(),
                lens.travel.end(),
                lens.start
            ),
            None => println!("  fixed focus"),
        }
        println!(
            "  {}",
            if camera.tuned {
                format!(
                    "tuned: {} locus points, {} matrices, {} shading maps",
                    camera.colour.locus.len(),
                    camera.colour.ccms.len(),
                    camera.colour.shading.len()
                )
            } else {
                "untuned".to_string()
            }
        );
        for note in &camera.notes {
            println!("  note: {note}");
        }
    }
    for skipped in &discovery.skipped {
        println!("skipped {skipped}");
    }
}

fn cameras() -> Result<(), String> {
    for camera in client::cameras().map_err(|error| format!("braun: {error}"))? {
        println!(
            "{}: {} {}{}{} {}x{}{}{}",
            camera.id,
            camera.name,
            camera.facing,
            if camera.role.is_empty() {
                String::new()
            } else {
                format!(" {}", camera.role)
            },
            camera
                .zoom
                .map_or(String::new(), |zoom| format!(" {zoom}x")),
            camera.width,
            camera.height,
            if camera.lens { ", focus" } else { "" },
            if camera.tuned { ", tuned" } else { "" }
        );
    }
    Ok(())
}

/// Opens camera `id` and streams it until it has settled; the last frame,
/// BGRA, and its size.
fn settled(session: &mut Session, id: &str) -> Result<(Vec<u8>, u32, u32), String> {
    session
        .send(&Request::Open(id.to_string()), &[])
        .map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mut frames = 0;
    loop {
        match session.next(true).map_err(|error| error.to_string())? {
            Some(Reply::Opened {
                width,
                height,
                lens,
                ..
            }) => {
                eprintln!(
                    "braunctl: {id} open after {:.2} s: {width}x{height}{}",
                    started.elapsed().as_secs_f32(),
                    if lens { ", focus" } else { "" }
                );
            }
            Some(Reply::Frame {
                slot,
                width,
                height,
                ..
            }) => {
                let pixels = session.frame(slot, width, height).map(<[u8]>::to_vec);
                session
                    .send(&Request::Release(slot), &[])
                    .map_err(|error| error.to_string())?;
                frames += 1;
                if frames == SETTLE_FRAMES {
                    eprintln!(
                        "braunctl: {frames} frames in {:.2} s",
                        started.elapsed().as_secs_f32()
                    );
                    return pixels
                        .map(|pixels| (pixels, width, height))
                        .ok_or("a frame outside its slot".into());
                }
            }
            Some(Reply::Failed(_, error)) => return Err(error),
            Some(_) | None => {}
        }
    }
}

fn preview(id: &str, file: &str) -> Result<(), String> {
    let mut session = Session::connect().map_err(|error| format!("braun: {error}"))?;
    let (pixels, width, height) = settled(&mut session, id)?;
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    for bgra in pixels.chunks_exact(4) {
        ppm.extend_from_slice(&[bgra[2], bgra[1], bgra[0]]);
    }
    std::fs::write(file, ppm).map_err(|error| format!("{file}: {error}"))
}

fn shot(id: &str, file: &str) -> Result<(), String> {
    let mut session = Session::connect().map_err(|error| format!("braun: {error}"))?;
    settled(&mut session, id)?;
    let jpeg = std::fs::File::create(file).map_err(|error| format!("{file}: {error}"))?;
    let started = Instant::now();
    session
        .send(&Request::Shoot(1), &[jpeg.as_fd()])
        .map_err(|error| error.to_string())?;
    drop(jpeg);
    let deadline = started + Duration::from_secs(60);
    while Instant::now() < deadline {
        match session.next(true).map_err(|error| error.to_string())? {
            Some(Reply::Frame { slot, .. }) => session
                .send(&Request::Release(slot), &[])
                .map_err(|error| error.to_string())?,
            Some(Reply::Captured(1)) => eprintln!(
                "braunctl: burst in after {:.2} s",
                started.elapsed().as_secs_f32()
            ),
            Some(Reply::Saved(1)) => {
                eprintln!(
                    "braunctl: {file} after {:.2} s",
                    started.elapsed().as_secs_f32()
                );
                return Ok(());
            }
            Some(Reply::Failed(_, error)) => return Err(error),
            Some(_) | None => {}
        }
    }
    Err("no photo after a minute".into())
}
