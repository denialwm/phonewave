//! The book itself: for each hour, state and build, how long the phone
//! spent there, the energy it took and what the hardware did. Closed hours
//! are appended to a file of JSON lines; the open ones are kept in /run so
//! a restart of the daemon loses nothing.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::counters::Usage;
use crate::sample::{Sample, State};

pub const STATE_DIR: &str = "/var/lib/battery-stats";
pub const RUN_DIR: &str = "/run/battery-stats";
const HOURS: &str = "hours.jsonl";
const OPEN: &str = "open.json";
/// Hours older than this are dropped.
pub const KEEP_DAYS: i64 = 31;

/// A gap in the monotonic clock longer than this was sleep.
const SLEPT: f64 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bucket {
    /// Unix time the hour starts.
    pub hour: i64,
    pub state: State,
    /// The kernel release and the tagged builds.
    pub build: String,
    pub seconds: f64,
    /// Out of the battery, into it while charging: current times voltage
    /// while awake, the gauge's count across sleep.
    pub wh: f64,
    /// What the gauge's own count says over the same time, to check `wh`.
    #[serde(default)]
    pub gauge_wh: f64,
    /// Brightness, 0 to 1, times seconds.
    #[serde(default)]
    pub brightness: f64,
    /// The battery level at the first and last sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<[f64; 2]>,
    #[serde(default)]
    pub usage: Usage,
}

impl Bucket {
    pub fn new(hour: i64, state: State, build: &str) -> Bucket {
        Bucket {
            hour,
            state,
            build: build.to_owned(),
            seconds: 0.0,
            wh: 0.0,
            gauge_wh: 0.0,
            brightness: 0.0,
            level: None,
            usage: Usage::default(),
        }
    }

    /// Adds `other`, which has the same key or is being merged across keys.
    pub fn add(&mut self, other: &Bucket) {
        self.seconds += other.seconds;
        self.wh += other.wh;
        self.gauge_wh += other.gauge_wh;
        self.brightness += other.brightness;
        self.level = match (self.level, other.level) {
            (Some([a, _]), Some([_, b])) => Some([a, b]),
            (a, b) => a.or(b),
        };
        self.usage.add(&other.usage);
    }
}

pub fn hour_of(unix: f64) -> i64 {
    (unix / 3600.0).floor() as i64 * 3600
}

/// The hours still open.
#[derive(Debug, Default)]
pub struct Book {
    pub open: Vec<Bucket>,
}

/// One stretch of time in one state, before it is split into hours.
struct Span {
    /// Unix times.
    start: f64,
    end: f64,
    state: State,
    seconds: f64,
    wh: f64,
    gauge_wh: f64,
    brightness: f64,
    level: Option<[f64; 2]>,
    usage: Option<Usage>,
}

impl Book {
    fn bucket(&mut self, hour: i64, state: State, build: &str) -> &mut Bucket {
        let i = match self
            .open
            .iter()
            .position(|b| b.hour == hour && b.state == state && b.build == build)
        {
            Some(i) => i,
            None => {
                self.open.push(Bucket::new(hour, state, build));
                self.open.len() - 1
            }
        };
        &mut self.open[i]
    }

    /// Spreads a span over the hours it covers, by time. The hardware's
    /// usage goes to the last hour.
    fn add(&mut self, span: Span, build: &str) {
        let length = (span.end - span.start).max(0.0);
        let mut at = span.start;
        loop {
            let hour = hour_of(at);
            let until = ((hour + 3600) as f64).min(span.end);
            let last = until >= span.end;
            let share = if length > 0.0 {
                (until - at) / length
            } else {
                1.0
            };
            let b = self.bucket(hour, span.state, build);
            b.seconds += span.seconds * share;
            b.wh += span.wh * share;
            b.gauge_wh += span.gauge_wh * share;
            b.brightness += span.brightness * share;
            if let Some([first, now]) = span.level {
                b.level = Some([b.level.map_or(first, |l| l[0]), now]);
            }
            if last {
                if let Some(usage) = &span.usage {
                    b.usage.add(usage);
                }
                break;
            }
            at = until;
        }
    }

    /// Books the time from `before` to `now`, which the hardware spent as
    /// `usage` says.
    pub fn account(&mut self, before: &Sample, now: &Sample, usage: Usage, build: &str) {
        let state = before.state();
        let awake = (now.mono - before.mono).max(0.0);
        let slept = now.slept_since(before);
        let gauge = now.gauge_wh_since(before);
        // Unix times from the boot clock, so a clock set meanwhile does
        // not stretch the span.
        let start = now.real - (now.boot - before.boot);
        let level = match (before.level, now.level) {
            (Some(a), Some(b)) => Some([a, b]),
            _ => None,
        };
        let lit = |seconds: f64| match (state, before.brightness) {
            (State::ScreenOn, Some(b)) => b * seconds,
            _ => 0.0,
        };
        if slept > SLEPT && state != State::Charging {
            // Awake for a moment either side of the sleep, at the rate
            // before it; the gauge's count for the rest.
            let awake_wh = before.watts.max(0.0) * awake / 3600.0;
            let asleep_wh = gauge.map_or(0.0, |g| (g - awake_wh).max(0.0));
            self.add(
                Span {
                    start,
                    end: start + awake,
                    state,
                    seconds: awake,
                    wh: awake_wh,
                    gauge_wh: awake_wh,
                    brightness: lit(awake),
                    level: None,
                    usage: None,
                },
                build,
            );
            self.add(
                Span {
                    start: start + awake,
                    end: now.real,
                    state: State::Asleep,
                    seconds: slept,
                    wh: asleep_wh,
                    gauge_wh: gauge.map_or(0.0, |g| g - awake_wh),
                    brightness: 0.0,
                    level,
                    usage: Some(usage),
                },
                build,
            );
        } else {
            let seconds = awake + slept;
            let wh = if slept > SLEPT {
                // Charging through a sleep: only the gauge knows.
                gauge.unwrap_or(0.0)
            } else {
                (before.watts + now.watts) / 2.0 * seconds / 3600.0
            };
            self.add(
                Span {
                    start,
                    end: now.real,
                    state,
                    seconds,
                    wh,
                    gauge_wh: gauge.unwrap_or(0.0),
                    brightness: lit(seconds),
                    level,
                    usage: Some(usage),
                },
                build,
            );
        }
    }

    /// Takes the buckets of hours before `hour`.
    pub fn close_before(&mut self, hour: i64) -> Vec<Bucket> {
        let (closed, open) = self.open.drain(..).partition(|b| b.hour < hour);
        self.open = open;
        closed
    }
}

/// Appends buckets to the hours file and makes sure they reach the disk.
pub fn append(dir: &Path, buckets: &[Bucket]) -> io::Result<()> {
    if buckets.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(dir.join(HOURS))?;
    // A line cut short by a crash would swallow the next one.
    if file.metadata()?.len() > 0 {
        let mut last = [0u8];
        file.seek(SeekFrom::End(-1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    let mut text = String::new();
    for b in buckets {
        text.push_str(&serde_json::to_string(b).map_err(io::Error::other)?);
        text.push('\n');
    }
    file.write_all(text.as_bytes())?;
    file.sync_data()
}

/// Every bucket in the hours file; lines that do not read are skipped.
pub fn load(dir: &Path) -> Vec<Bucket> {
    fs::read_to_string(dir.join(HOURS))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn replace(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("new");
    let mut file = File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_data()?;
    fs::rename(tmp, path)
}

/// Drops the hours before `oldest`, if there are any.
pub fn trim(dir: &Path, oldest: i64) -> io::Result<()> {
    let path = dir.join(HOURS);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let keep: Vec<&str> = text
        .lines()
        .filter(|l| serde_json::from_str::<Bucket>(l).is_ok_and(|b| b.hour >= oldest))
        .collect();
    if keep.len() == text.lines().count() {
        return Ok(());
    }
    let mut out = keep.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    replace(&path, &out)
}

pub fn save_open(dir: &Path, buckets: &[Bucket]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(OPEN);
    let tmp = path.with_extension("new");
    fs::write(&tmp, serde_json::to_vec(buckets).map_err(io::Error::other)?)?;
    fs::rename(tmp, path)
}

pub fn load_open(dir: &Path) -> Vec<Bucket> {
    fs::read(dir.join(OPEN))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn remove_open(dir: &Path) {
    let _ = fs::remove_file(dir.join(OPEN));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(mono: f64, boot: f64, real: f64, watts: f64, charge: f64) -> Sample {
        Sample {
            mono,
            boot,
            real,
            watts,
            volts: 4.0,
            energy_wh: None,
            charge_ah: Some(charge),
            level: Some(50.0),
            plugged: false,
            screen: false,
            brightness: Some(0.5),
        }
    }

    #[test]
    fn awake_time_by_current() {
        let mut book = Book::default();
        let a = sample(100.0, 100.0, 7200.0, 1.0, 4.0);
        let b = sample(110.0, 110.0, 7210.0, 2.0, 4.0);
        book.account(&a, &b, Usage::default(), "k");
        assert_eq!(book.open.len(), 1);
        let bucket = &book.open[0];
        assert_eq!(bucket.state, State::ScreenOff);
        assert_eq!(bucket.seconds, 10.0);
        assert!((bucket.wh - 1.5 * 10.0 / 3600.0).abs() < 1e-12);
    }

    #[test]
    fn sleep_by_the_gauge() {
        let mut book = Book::default();
        // 10 s awake at 0.36 W, then 2 h asleep; the gauge lost 0.1 Ah at 4 V.
        let a = sample(100.0, 100.0, 3600.0 * 10.0, 0.36, 4.0);
        let b = sample(110.0, 110.0 + 7200.0, 3600.0 * 12.0 + 10.0, 0.36, 3.9);
        book.account(&a, &b, Usage::default(), "k");
        let asleep: Vec<&Bucket> = book
            .open
            .iter()
            .filter(|b| b.state == State::Asleep)
            .collect();
        let seconds: f64 = asleep.iter().map(|b| b.seconds).sum();
        let wh: f64 = asleep.iter().map(|b| b.wh).sum();
        assert!((seconds - 7200.0).abs() < 1e-6);
        assert!((wh - (0.4 - 0.001)).abs() < 1e-9, "{wh}");
        // Two hours and a few seconds: spread over three.
        assert_eq!(asleep.len(), 3);
        let awake = book
            .open
            .iter()
            .find(|b| b.state == State::ScreenOff)
            .unwrap();
        assert_eq!(awake.seconds, 10.0);
    }

    #[test]
    fn closes_past_hours() {
        let mut book = Book::default();
        book.open.push(Bucket::new(0, State::ScreenOn, "k"));
        book.open.push(Bucket::new(3600, State::ScreenOn, "k"));
        let closed = book.close_before(3600);
        assert_eq!(closed.len(), 1);
        assert_eq!(book.open.len(), 1);
    }

    #[test]
    fn hours_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("battery-stats-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut b = Bucket::new(3600, State::Asleep, "k");
        b.usage.ddr.insert(547, 2.0);
        append(&dir, &[b.clone(), Bucket::new(7200, State::ScreenOn, "k")]).unwrap();
        // A torn line from a crash.
        OpenOptions::new()
            .append(true)
            .open(dir.join(HOURS))
            .unwrap()
            .write_all(b"{\"hour\":")
            .unwrap();
        append(&dir, &[Bucket::new(10800, State::ScreenOff, "k")]).unwrap();
        let all = load(&dir);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], b);
        trim(&dir, 7200).unwrap();
        assert_eq!(load(&dir).len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }
}
