//! Location: the modem's GNSS engine, through QMI LOC (service 16).
//!
//! Most LOC requests answer twice: a response that only says the request
//! was taken, then an indication with the same id that carries the outcome.
//! `ask` waits for both, so it needs a client whose indications nobody else
//! reads. Position and satellite reports go to the client that started the
//! session; the modem ends that session when the client's socket closes.

use std::time::Duration;

use async_io::Timer;
use futures_lite::future;

use crate::qmi::{Client, Error, Message};

pub const REGISTER_EVENTS: u16 = 0x21;
const START: u16 = 0x22;
const STOP: u16 = 0x23;
pub const POSITION_REPORT: u16 = 0x24;
pub const SATELLITES: u16 = 0x25;
pub const INJECT_TIME_WANTED: u16 = 0x28;
pub const XTRA_WANTED: u16 = 0x29;
pub const ENGINE_STATE: u16 = 0x2b;
pub const FIX_SESSION_STATE: u16 = 0x2c;
/// Predicted orbits: the older request, which the 12R refuses as not
/// supported, and the newer one it takes.
const INJECT_PREDICTED_ORBITS: u16 = 0x35;
const INJECT_XTRA: u16 = 0xa7;
const NOT_SUPPORTED: u16 = 0x5e;
const GET_XTRA_VALIDITY: u16 = 0x37;
const INJECT_UTC_TIME: u16 = 0x38;
const SET_ENGINE_LOCK: u16 = 0x3a;
const GET_ENGINE_LOCK: u16 = 0x3b;
const GET_OPERATION_MODE: u16 = 0x4b;

const EVENT_POSITION: u64 = 1 << 0;
const EVENT_SATELLITES: u64 = 1 << 1;
const EVENT_INJECT_TIME: u64 = 1 << 4;
const EVENT_XTRA_WANTED: u64 = 1 << 5;
const EVENT_ENGINE_STATE: u64 = 1 << 7;
const EVENT_FIX_SESSION: u64 = 1 << 8;
/// Without this client type the modem turns REGISTER_EVENTS down with
/// InvalidArgument (tested on the 12R, 2026-11-16). 1: the platform's own
/// location framework.
const CLIENT_TYPE_PLATFORM: u32 = 1;

/// Engine locks: who may start a session.
pub const LOCK_NONE: u32 = 1;
pub const LOCK_ALL: u32 = 4;

const PERIODIC: u32 = 1;
/// The accuracy a fix must reach to be final. High held every report "in
/// progress" for 8 minutes by a window on the 12R (4 satellites, ±180 m);
/// low makes each position final and lets it improve report by report.
const ACCURACY_LOW: u32 = 1;
const INTERMEDIATE_ON: u32 = 1;
/// One report a second.
const INTERVAL_MS: u32 = 1000;

const TIMEOUT: Duration = Duration::from_secs(10);

pub fn lock_name(lock: u32) -> &'static str {
    match lock {
        1 => "none",
        2 => "mobile-initiated",
        3 => "network-initiated",
        4 => "all",
        _ => "unknown",
    }
}

pub fn mode_name(mode: u32) -> &'static str {
    match mode {
        1 => "default",
        2 => "MS-based",
        3 => "MS-assisted",
        4 => "standalone",
        5 => "cell ID",
        6 => "WWAN",
        _ => "unknown",
    }
}

fn u32_at(v: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(v.get(..4)?.try_into().ok()?))
}

fn f32_tlv(msg: &Message, t: u8) -> Option<f32> {
    let v = msg.tlv(t)?;
    Some(f32::from_le_bytes(v.get(..4)?.try_into().ok()?)).filter(|f| f.is_finite())
}

fn f64_tlv(msg: &Message, t: u8) -> Option<f64> {
    let v = msg.tlv(t)?;
    Some(f64::from_le_bytes(v.get(..8)?.try_into().ok()?)).filter(|f| f.is_finite())
}

/// Sends a request and waits for the indication that carries its outcome;
/// a status other than 0 in it becomes `Error::Qmi`.
pub async fn ask(loc: &Client, msg: Message) -> Result<Message, Error> {
    let id = msg.id;
    loc.call(msg, TIMEOUT).await?;
    let outcome = async {
        loop {
            let ind = loc.indication().await?;
            if ind.id == id {
                return Ok(ind);
            }
        }
    };
    let timer = async {
        Timer::after(TIMEOUT).await;
        Err(Error::Timeout)
    };
    let ind = future::or(outcome, timer).await?;
    match ind.tlv(0x01).and_then(u32_at) {
        Some(0) => Ok(ind),
        Some(status) => Err(Error::Qmi(status as u16)),
        None => Err(Error::Malformed("LOC status")),
    }
}

pub async fn engine_lock(loc: &Client) -> Result<u32, Error> {
    let ind = ask(loc, Message::request(GET_ENGINE_LOCK)).await?;
    ind.tlv(0x10)
        .and_then(u32_at)
        .ok_or(Error::Malformed("engine lock"))
}

pub async fn set_engine_lock(loc: &Client, lock: u32) -> Result<(), Error> {
    let msg = Message::request(SET_ENGINE_LOCK).with(0x01, lock.to_le_bytes());
    ask(loc, msg).await.map(drop)
}

pub async fn operation_mode(loc: &Client) -> Result<u32, Error> {
    let ind = ask(loc, Message::request(GET_OPERATION_MODE)).await?;
    ind.tlv(0x10)
        .and_then(u32_at)
        .ok_or(Error::Malformed("operation mode"))
}

/// Asks for position and satellite reports, and for the engine's requests.
pub async fn register(loc: &Client) -> Result<(), Error> {
    let mask = EVENT_POSITION
        | EVENT_SATELLITES
        | EVENT_INJECT_TIME
        | EVENT_XTRA_WANTED
        | EVENT_ENGINE_STATE
        | EVENT_FIX_SESSION;
    let msg = Message::request(REGISTER_EVENTS)
        .with(0x01, mask.to_le_bytes())
        .with(0x11, CLIENT_TYPE_PLATFORM.to_le_bytes());
    loc.call(msg, TIMEOUT).await.map(drop)
}

/// Starts a session that reports every second until `stop`.
pub async fn start(loc: &Client, session: u8) -> Result<(), Error> {
    let msg = Message::request(START)
        .with(0x01, [session])
        .with(0x10, PERIODIC.to_le_bytes())
        .with(0x11, ACCURACY_LOW.to_le_bytes())
        .with(0x12, INTERMEDIATE_ON.to_le_bytes())
        .with(0x13, INTERVAL_MS.to_le_bytes());
    loc.call(msg, TIMEOUT).await.map(drop)
}

pub async fn stop(loc: &Client, session: u8) -> Result<(), Error> {
    let msg = Message::request(STOP).with(0x01, [session]);
    loc.call(msg, TIMEOUT).await.map(drop)
}

/// Gives the engine the time, when the system clock is synchronized. Returns
/// whether it did.
pub async fn inject_time(loc: &Client) -> Result<bool, Error> {
    let Some((utc_ms, uncertainty_ms)) = synced_time() else {
        return Ok(false);
    };
    let msg = Message::request(INJECT_UTC_TIME)
        .with(0x01, utc_ms.to_le_bytes())
        .with(0x02, uncertainty_ms.to_le_bytes());
    ask(loc, msg).await.map(|_| true)
}

/// The time in Unix milliseconds and its uncertainty in milliseconds, if the
/// kernel says the clock is synchronized.
fn synced_time() -> Option<(u64, u32)> {
    // SAFETY: an all-zero timex with modes 0 only reads.
    let mut tx: libc::timex = unsafe { std::mem::zeroed() };
    // SAFETY: tx is a valid, writable timex.
    let state = unsafe { libc::adjtimex(&mut tx) };
    if state < 0 || state == libc::TIME_ERROR {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    // maxerror is in microseconds; never claim better than 10 ms.
    let uncertainty = (tx.maxerror.max(0) as u64 / 1000).clamp(10, u32::MAX.into()) as u32;
    Some((now.as_millis() as u64, uncertainty))
}

/// The largest XTRA part the modem takes.
const XTRA_PART: usize = 1024;
const FORMAT_XTRA: u32 = 0;
/// GPS time 0 in Unix seconds: the start the modem reports for no data.
const GPS_EPOCH: u64 = 315_964_800;

/// Gives the engine predicted orbits (an XTRA file), part by part, each
/// acknowledged before the next. Modems that do not know the newer request
/// get the older one.
pub async fn inject_xtra(loc: &Client, data: &[u8]) -> Result<(), Error> {
    match inject_parts(loc, data, INJECT_XTRA).await {
        Err(Error::Qmi(NOT_SUPPORTED)) => inject_parts(loc, data, INJECT_PREDICTED_ORBITS).await,
        done => done,
    }
}

async fn inject_parts(loc: &Client, data: &[u8], id: u16) -> Result<(), Error> {
    let total = u16::try_from(data.len().div_ceil(XTRA_PART))
        .map_err(|_| Error::Malformed("XTRA file too large"))?;
    for (i, part) in data.chunks(XTRA_PART).enumerate() {
        let mut bytes = (part.len() as u16).to_le_bytes().to_vec();
        bytes.extend_from_slice(part);
        let mut msg = Message::request(id)
            .with(0x01, (data.len() as u32).to_le_bytes())
            .with(0x02, total.to_le_bytes())
            .with(0x03, (i as u16 + 1).to_le_bytes())
            .with(0x04, bytes);
        if id == INJECT_PREDICTED_ORBITS {
            msg = msg.with(0x10, FORMAT_XTRA.to_le_bytes());
        }
        ask(loc, msg).await?;
    }
    Ok(())
}

/// The span the engine's predicted orbits cover: Unix seconds it starts at,
/// and hours. None when it holds none.
pub async fn xtra_validity(loc: &Client) -> Result<Option<(u64, u16)>, Error> {
    let ind = ask(loc, Message::request(GET_XTRA_VALIDITY)).await?;
    Ok(parse_validity(&ind))
}

fn parse_validity(ind: &Message) -> Option<(u64, u16)> {
    let v = ind.tlv(0x10)?;
    let start = u64::from_le_bytes(v.get(..8)?.try_into().ok()?);
    let hours = u16::from_le_bytes(v.get(8..10)?.try_into().ok()?);
    (hours > 0 && start > GPS_EPOCH).then_some((start, hours))
}

/// One position report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// 0 a fix, 1 still searching, others an error: see `status_name`.
    pub status: u32,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Horizontal uncertainty, metres.
    pub accuracy: Option<f32>,
    /// Above the WGS 84 ellipsoid, metres.
    pub altitude: Option<f32>,
    /// Above mean sea level, metres.
    pub altitude_msl: Option<f32>,
    /// Horizontal dilution of precision.
    pub hdop: Option<f32>,
    /// Metres a second.
    pub speed: Option<f32>,
    /// Degrees from true north.
    pub heading: Option<f32>,
    /// Unix milliseconds of the fix.
    pub time_ms: Option<u64>,
    /// Satellites the fix used.
    pub used: u32,
}

impl Report {
    /// A position the engine stands behind: final, with coordinates.
    pub fn is_fix(&self) -> bool {
        self.status == 0 && self.latitude.is_some() && self.longitude.is_some()
    }
}

pub fn status_name(status: u32) -> &'static str {
    match status {
        0 => "fix",
        1 => "searching",
        2 => "failed",
        3 => "timed out",
        4 => "ended",
        5 => "bad parameter",
        6 => "phone offline",
        7 => "engine locked",
        _ => "unknown",
    }
}

pub fn parse_report(msg: &Message) -> Option<Report> {
    let status = msg.tlv(0x01).and_then(u32_at)?;
    Some(Report {
        status,
        latitude: f64_tlv(msg, 0x10),
        longitude: f64_tlv(msg, 0x11),
        accuracy: f32_tlv(msg, 0x12),
        altitude: f32_tlv(msg, 0x1a),
        altitude_msl: f32_tlv(msg, 0x1b),
        // PDOP, HDOP, VDOP.
        hdop: msg
            .tlv(0x24)
            .and_then(|v| Some(f32::from_le_bytes(v.get(4..8)?.try_into().ok()?)))
            .filter(|f| f.is_finite()),
        speed: f32_tlv(msg, 0x18),
        heading: f32_tlv(msg, 0x20),
        time_ms: msg
            .tlv(0x25)
            .and_then(|v| Some(u64::from_le_bytes(v.get(..8)?.try_into().ok()?))),
        used: msg
            .tlv(0x2c)
            .and_then(|v| v.first())
            .map_or(0, |&n| n.into()),
    })
}

/// A satellite the engine is tracking: it hears it.
const SV_TRACKING: u32 = 3;
/// One satellite of the satellite report: the classic list (TLV 0x10) holds
/// 28 bytes each, the expanded list (0x11) one more, the GLONASS frequency.
const SV_ENTRY: usize = 28;

/// The satellites a report lists: how many, and how many the engine tracks.
/// The 12R fills the expanded list (TLV 0x11) and leaves the classic one out.
pub fn parse_satellites(msg: &Message) -> Option<(u32, u32)> {
    let (list, size) = match (msg.tlv(0x11), msg.tlv(0x10)) {
        (Some(list), _) => (list, SV_ENTRY + 1),
        (None, Some(list)) => (list, SV_ENTRY),
        (None, None) => return None,
    };
    let (&n, entries) = list.split_first()?;
    let entries = entries.get(..usize::from(n) * size)?;
    // Status is a u32 after the valid mask, system, id and health.
    let tracked = entries
        .chunks_exact(size)
        .filter(|e| u32_at(&e[11..]) == Some(SV_TRACKING))
        .count();
    Some((n.into(), tracked as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indication(id: u16, tlvs: &[(u8, Vec<u8>)]) -> Message {
        let mut bytes = vec![4, 0, 0];
        bytes.extend_from_slice(&id.to_le_bytes());
        let body: usize = tlvs.iter().map(|(_, v)| 3 + v.len()).sum();
        bytes.extend_from_slice(&(body as u16).to_le_bytes());
        for (t, v) in tlvs {
            bytes.push(*t);
            bytes.extend_from_slice(&(v.len() as u16).to_le_bytes());
            bytes.extend_from_slice(v);
        }
        Message::decode(&bytes).unwrap()
    }

    #[test]
    fn reads_a_fix() {
        let msg = indication(
            POSITION_REPORT,
            &[
                (0x01, 0u32.to_le_bytes().to_vec()),
                (0x02, vec![1]),
                (0x10, 45.4642f64.to_le_bytes().to_vec()),
                (0x11, 9.19f64.to_le_bytes().to_vec()),
                (0x12, 87.5f32.to_le_bytes().to_vec()),
                // As the 12R sends them: UTC time, leap seconds, used list.
                (0x25, 1_763_000_000_000u64.to_le_bytes().to_vec()),
                (0x26, vec![18]),
                (0x2c, vec![2, 3, 0, 9, 0]),
            ],
        );
        let r = parse_report(&msg).unwrap();
        assert!(r.is_fix());
        assert_eq!(r.latitude, Some(45.4642));
        assert_eq!(r.accuracy, Some(87.5));
        assert_eq!(r.altitude, None);
        assert_eq!(r.time_ms, Some(1_763_000_000_000));
        assert_eq!(r.used, 2);
    }

    #[test]
    fn reads_xtra_validity() {
        // As the 12R answered with no data: the GPS epoch, 0 hours.
        let none = indication(
            0x37,
            &[(0x01, vec![0; 4]), (0x10, hex("803dd512000000000000"))],
        );
        assert_eq!(parse_validity(&none), None);
        let mut v = 1_795_000_000u64.to_le_bytes().to_vec();
        v.extend_from_slice(&168u16.to_le_bytes());
        let week = indication(0x37, &[(0x01, vec![0; 4]), (0x10, v)]);
        assert_eq!(parse_validity(&week), Some((1_795_000_000, 168)));
    }

    #[test]
    fn counts_tracked_satellites() {
        // The first entry as the 12R sent it (GPS 5, searched for), then the
        // same satellite tracked.
        let searched = hex("7b000000010000000500000200000002c3cc8f419d6f24420000000000");
        let mut tracked = searched.clone();
        tracked[11] = 3;
        let mut list = vec![2];
        list.extend_from_slice(&searched);
        list.extend_from_slice(&tracked);
        let msg = indication(SATELLITES, &[(0x01, vec![0]), (0x11, list)]);
        assert_eq!(parse_satellites(&msg), Some((2, 1)));
        let short = indication(SATELLITES, &[(0x11, vec![3, 0, 0])]);
        assert_eq!(parse_satellites(&short), None);
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn searching_is_no_fix() {
        let msg = indication(
            POSITION_REPORT,
            &[
                (0x01, 1u32.to_le_bytes().to_vec()),
                (0x10, 45.0f64.to_le_bytes().to_vec()),
                (0x11, 9.0f64.to_le_bytes().to_vec()),
                (0x12, 1.0e6f32.to_le_bytes().to_vec()),
            ],
        );
        let r = parse_report(&msg).unwrap();
        assert!(!r.is_fix());
        assert_eq!(status_name(r.status), "searching");
    }
}
