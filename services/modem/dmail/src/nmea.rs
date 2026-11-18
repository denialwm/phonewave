//! Fixes as NMEA 0183 on a Unix socket, for GeoClue's NMEA source
//! (`nmea-socket` in its `[network-nmea]` section). The 12R's engine makes no
//! NMEA of its own, so dmail writes GGA and RMC from each fix. A connection
//! counts as a client that wants location, like `Location1.Start`, until it
//! closes. The socket exists only where GeoClue is installed: it belongs to
//! group `geoclue`.

use std::cell::RefCell;
use std::ffi::CString;
use std::fmt::Write as _;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::rc::Rc;

use async_channel::Sender;
use async_executor::LocalExecutor;
use async_io::Async;
use futures_lite::{AsyncReadExt, AsyncWriteExt, future};

use crate::bus::Command;
use crate::location::Report;

pub const SOCKET: &str = "/run/dmail/nmea.sock";
const GROUP: &str = "geoclue";
/// Sentences a slow reader may fall behind by before it misses some.
const BACKLOG: usize = 16;
const KNOTS_PER_MPS: f64 = 1.943_844;

fn log(what: impl std::fmt::Display) {
    eprintln!("dmail: NMEA: {what}");
}

/// The readers connected now; the core hands each fix to all of them.
#[derive(Clone, Default)]
pub struct Feed {
    readers: Readers,
}

/// Each reader's id and the queue of text to write to it.
type Readers = Rc<RefCell<Vec<(u64, Sender<Rc<str>>)>>>;

impl Feed {
    /// Sends a fix's sentences to every reader. A reader that is behind
    /// misses them.
    pub fn send(&self, r: &Report) {
        if self.readers.borrow().is_empty() {
            return;
        }
        let Some(text) = sentences(r) else {
            return;
        };
        let text: Rc<str> = text.into();
        for (_, tx) in self.readers.borrow().iter() {
            let _ = tx.try_send(text.clone());
        }
    }
}

/// Serves the socket, if group `geoclue` exists. Each reader is a location
/// client named `nmea:N`.
pub fn serve(ex: &LocalExecutor<'static>, commands: Sender<Command>) -> Feed {
    let feed = Feed::default();
    let Some(gid) = group_id(GROUP) else {
        return feed;
    };
    let listener = match listen(gid) {
        Ok(listener) => listener,
        Err(e) => {
            log(format!("{SOCKET}: {e}"));
            return feed;
        }
    };
    let readers = feed.readers.clone();
    ex.spawn(async move {
        // Readers run on an executor of the listener's own.
        let spawner = LocalExecutor::new();
        spawner
            .run(accept(&spawner, listener, readers, commands))
            .await;
    })
    .detach();
    log(format!("serving {SOCKET}"));
    feed
}

async fn accept(
    spawner: &LocalExecutor<'_>,
    listener: Async<UnixListener>,
    readers: Readers,
    commands: Sender<Command>,
) {
    let mut next = 0u64;
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                log(format!("accepting: {e}"));
                return;
            }
        };
        let id = next;
        next += 1;
        let (tx, rx) = async_channel::bounded(BACKLOG);
        readers.borrow_mut().push((id, tx));
        let (readers, commands) = (readers.clone(), commands.clone());
        spawner
            .spawn(async move {
                let name = format!("nmea:{id}");
                let (reply, answer) = async_channel::bounded(1);
                let started = commands
                    .send(Command::LocationStart(name.clone(), reply))
                    .await
                    .is_ok()
                    && matches!(answer.recv().await, Ok(Ok(())));
                if started {
                    stream_to(stream, rx).await;
                } else {
                    log(format!("{name}: location is not available"));
                }
                readers.borrow_mut().retain(|(n, _)| *n != id);
                let _ = commands.send(Command::NameLost(name)).await;
            })
            .detach();
    }
}

/// Writes sentences until the reader closes or falls off.
async fn stream_to(stream: Async<UnixStream>, lines: async_channel::Receiver<Rc<str>>) {
    let stream = Rc::new(stream);
    let writer = {
        let stream = stream.clone();
        async move {
            while let Ok(text) = lines.recv().await {
                if (&*stream).write_all(text.as_bytes()).await.is_err() {
                    return;
                }
            }
        }
    };
    let closed = async {
        let mut buf = [0u8; 256];
        while matches!((&*stream).read(&mut buf).await, Ok(n) if n > 0) {}
    };
    future::or(writer, closed).await;
}

fn listen(gid: libc::gid_t) -> io::Result<Async<UnixListener>> {
    let _ = std::fs::remove_file(SOCKET);
    let listener = UnixListener::bind(SOCKET)?;
    let path = CString::new(SOCKET).expect("no NUL in the path");
    // SAFETY: plain chown(2) on a path that lives for the call.
    if unsafe { libc::chown(path.as_ptr(), 0, gid) } < 0 {
        return Err(io::Error::last_os_error());
    }
    std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o660))?;
    Async::new(listener)
}

fn group_id(name: &str) -> Option<libc::gid_t> {
    let name = CString::new(name).ok()?;
    // SAFETY: getgrnam(3) with a valid C string; dmail's core is one thread,
    // so its static result is read before anything else can call it.
    let group = unsafe { libc::getgrnam(name.as_ptr()) };
    // SAFETY: a non-null result points at a valid group entry.
    (!group.is_null()).then(|| unsafe { (*group).gr_gid })
}

/// GGA and RMC for a fix, each ending in CR LF. None for anything but a fix.
pub fn sentences(r: &Report) -> Option<String> {
    if !r.is_fix() {
        return None;
    }
    let (lat, lon) = (r.latitude?, r.longitude?);
    let ms = r.time_ms?;
    let (secs, millis) = (ms / 1000, ms % 1000);
    let day = secs / 86400;
    let tod = secs % 86400;
    let time = format!(
        "{:02}{:02}{:02}.{:02}",
        tod / 3600,
        tod / 60 % 60,
        tod % 60,
        millis / 10
    );
    let (y, m, d) = civil(day as i64);
    let date = format!("{d:02}{m:02}{:02}", y % 100);
    let lat_s = angle(lat, 2, ['N', 'S']);
    let lon_s = angle(lon, 3, ['E', 'W']);
    let hdop = r.hdop.map_or(String::new(), |h| format!("{h:.1}"));
    // GGA: altitude above sea level, then the geoid's height above the
    // ellipsoid, when the engine reports both.
    let (msl, sep) = match (r.altitude_msl, r.altitude) {
        (Some(msl), Some(ell)) => (format!("{msl:.1},M"), format!("{:.1},M", ell - msl)),
        (Some(msl), None) => (format!("{msl:.1},M"), ",".into()),
        _ => (",".into(), ",".into()),
    };
    let gga = format!(
        "GPGGA,{time},{lat_s},{lon_s},1,{:02},{hdop},{msl},{sep},,",
        r.used
    );
    let speed = r.speed.map_or(String::new(), |v| {
        format!("{:.2}", f64::from(v) * KNOTS_PER_MPS)
    });
    let course = r.heading.map_or(String::new(), |h| format!("{h:.1}"));
    let rmc = format!("GPRMC,{time},A,{lat_s},{lon_s},{speed},{course},{date},,,A");
    let mut out = String::new();
    for body in [gga, rmc] {
        let sum = body.bytes().fold(0u8, |a, b| a ^ b);
        let _ = write!(out, "${body}*{sum:02X}\r\n");
    }
    Some(out)
}

/// Degrees as NMEA's dddmm.mmmmm with its hemisphere letter.
fn angle(deg: f64, width: usize, hemispheres: [char; 2]) -> String {
    let side = if deg < 0.0 {
        hemispheres[1]
    } else {
        hemispheres[0]
    };
    let deg = deg.abs();
    let whole = deg.trunc();
    let minutes = (deg - whole) * 60.0;
    format!("{:0width$}{minutes:08.5},{side}", whole as u32)
}

/// Year, month and day of a day count since 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days_from_civil, inverted.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix() -> Report {
        Report {
            status: 0,
            latitude: Some(48.1173),
            longitude: Some(-11.516_666_7),
            accuracy: Some(10.0),
            altitude: Some(593.1),
            altitude_msl: Some(545.4),
            hdop: Some(0.9),
            speed: Some(11.524),
            heading: Some(84.4),
            // 1994-03-23 12:35:19 UTC.
            time_ms: Some(764_426_119_000),
            used: 8,
        }
    }

    #[test]
    fn writes_gga_and_rmc() {
        let text = sentences(&fix()).unwrap();
        let lines: Vec<_> = text.split("\r\n").collect();
        assert_eq!(
            lines[0],
            "$GPGGA,123519.00,4807.03800,N,01131.00000,W,1,08,0.9,545.4,M,47.7,M,,*74"
        );
        assert!(
            lines[1].starts_with(
                "$GPRMC,123519.00,A,4807.03800,N,01131.00000,W,22.40,84.4,230394,,,A*"
            )
        );
        assert_eq!(lines[2], "");
    }

    #[test]
    fn checksums_match() {
        for line in sentences(&fix()).unwrap().lines() {
            let (body, sum) = line[1..].split_once('*').unwrap();
            let want = body.bytes().fold(0u8, |a, b| a ^ b);
            assert_eq!(format!("{want:02X}"), sum);
        }
    }

    #[test]
    fn nothing_without_a_fix() {
        let searching = Report { status: 1, ..fix() };
        assert_eq!(sentences(&searching), None);
    }

    #[test]
    fn counts_days() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(8847), (1994, 3, 23));
        assert_eq!(civil(20_775), (2026, 11, 18));
    }
}
