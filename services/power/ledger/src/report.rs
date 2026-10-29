//! The ledger as text: totals per state, what the hardware did in each, a
//! line per hour, and the states' averages per build.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::book::Bucket;
use crate::counters::Usage;
use crate::sample::State;

/// Qualcomm sleep stats that only count while the whole phone sleeps.
const SYSTEM_SLEEP: [(&str, &str); 3] = [("aosd", "whole chip"), ("cxsd", "CX"), ("ddr", "DDR")];
/// Stats that are not a subsystem sleeping: the CPUs' own count and the
/// DCP's, and the DSPs' low-power islands.
fn not_a_subsystem(name: &str) -> bool {
    matches!(name, "apss" | "dcp") || name.ends_with("_island")
}

pub fn duration(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}h{:02}", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// A Unix time in local time, through strftime.
pub fn local(unix: i64, format: &str) -> String {
    let t = unix as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let mut out = [0u8; 64];
    let Ok(format) = std::ffi::CString::new(format) else {
        return String::new();
    };
    // SAFETY: valid pointers, and out's size is passed.
    let n = unsafe {
        libc::localtime_r(&t, &mut tm);
        libc::strftime(out.as_mut_ptr().cast(), out.len(), format.as_ptr(), &tm)
    };
    String::from_utf8_lossy(&out[..n]).into_owned()
}

fn percent(part: f64, whole: f64) -> String {
    if whole <= 0.0 {
        return "–".into();
    }
    format!("{:.0}%", (part / whole * 100.0).clamp(0.0, 100.0))
}

fn watts(b: &Bucket) -> f64 {
    if b.seconds > 0.0 {
        b.wh * 3600.0 / b.seconds
    } else {
        0.0
    }
}

/// The buckets added up per state.
fn by_state<'a>(buckets: impl IntoIterator<Item = &'a Bucket>) -> BTreeMap<State, Bucket> {
    let mut states: BTreeMap<State, Bucket> = BTreeMap::new();
    for b in buckets {
        states
            .entry(b.state)
            .or_insert_with(|| Bucket::new(b.hour, b.state, ""))
            .add(b);
    }
    states
}

/// The level at the start of the earliest hour and the end of the latest.
fn levels(buckets: &[Bucket]) -> Option<(f64, f64)> {
    let first = buckets
        .iter()
        .filter(|b| b.level.is_some())
        .min_by_key(|b| b.hour)?;
    let last = buckets
        .iter()
        .filter(|b| b.level.is_some())
        .max_by_key(|b| b.hour)?;
    Some((first.level?[0], last.level?[1]))
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// What the hardware did in one state, a few lines.
fn hardware(state: State, b: &Bucket) -> Vec<String> {
    let u: &Usage = &b.usage;
    let mut lines = Vec::new();
    let awake = state != State::Asleep;
    if state == State::ScreenOn && b.seconds > 0.0 {
        lines.push(format!("brightness {}", percent(b.brightness, b.seconds)));
    }
    if awake && !u.cpu.is_empty() {
        let mut parts = Vec::new();
        let mut model = 0.0;
        let mut modelled = false;
        for (name, c) in &u.cpu {
            let at: f64 = c.at.values().sum();
            let mean = if at > 0.0 {
                c.at.iter().map(|(mhz, s)| *mhz as f64 * s).sum::<f64>() / at
            } else {
                0.0
            };
            parts.push(format!(
                "{name} {} busy at {:.2} GHz",
                percent(c.busy, c.cpus as f64 * b.seconds),
                mean / 1000.0
            ));
            if let Some(j) = c.model_j {
                model += j;
                modelled = true;
            }
        }
        let mut line = format!("CPU      {}", parts.join(", "));
        if modelled && b.seconds > 0.0 {
            let _ = write!(line, "; model {:.2} W", model / b.seconds);
        }
        lines.push(line);
    }
    if awake && !u.on.is_empty() {
        let parts: Vec<String> =
            u.on.iter()
                .map(|(name, t)| format!("{name} {}", percent(t.active, t.total)))
                .collect();
        lines.push(format!("powered  {}", parts.join(", ")));
    }
    let ddr: f64 = u.ddr.values().sum();
    if ddr > 0.0 {
        let lowest = u.ddr.iter().next();
        let top = u.ddr.iter().next_back();
        let mean = u.ddr.iter().map(|(mhz, s)| *mhz as f64 * s).sum::<f64>() / ddr;
        let mut line = String::from("DDR      ");
        if let Some((mhz, s)) = lowest {
            let _ = write!(line, "{mhz} MHz {}", percent(*s, ddr));
        }
        if let Some((mhz, s)) = top.filter(|_| u.ddr.len() > 1) {
            let _ = write!(line, ", {mhz} MHz {}", percent(*s, ddr));
        }
        let _ = write!(line, ", mean {:.2} GHz", mean / 1000.0);
        lines.push(line);
    }
    if !u.sleep.is_empty() && b.seconds > 0.0 {
        let parts: Vec<String> = if awake {
            u.sleep
                .iter()
                .filter(|(name, _)| {
                    !SYSTEM_SLEEP.iter().any(|(n, _)| n == name) && !not_a_subsystem(name)
                })
                .map(|(name, s)| format!("{name} {}", percent(*s, b.seconds)))
                .collect()
        } else {
            SYSTEM_SLEEP
                .iter()
                .filter_map(|(name, label)| {
                    Some(format!(
                        "{label} {}",
                        percent(*u.sleep.get(*name)?, b.seconds)
                    ))
                })
                .collect()
        };
        if !parts.is_empty() {
            lines.push(format!("slept    {}", parts.join(", ")));
        }
    }
    if state == State::Asleep {
        let total: u64 = u.wakeups.values().map(|w| w.wakeups).sum();
        let mut top: Vec<(&String, u64)> = u
            .wakeups
            .iter()
            .map(|(n, w)| (n, w.wakeups))
            .filter(|(_, c)| *c > 0)
            .collect();
        top.sort_by_key(|t| std::cmp::Reverse(t.1));
        let names: Vec<String> = top
            .iter()
            .take(4)
            .map(|(n, c)| format!("{n} {c}"))
            .collect();
        let mut line = format!("woken    {total} times");
        if !names.is_empty() {
            let _ = write!(line, ": {}", names.join(", "));
        }
        lines.push(line);
    }
    if state == State::ScreenOff {
        let mut top: Vec<(&String, f64)> = u
            .wakeups
            .iter()
            .map(|(n, w)| (n, w.active))
            .filter(|(_, a)| *a >= 1.0)
            .collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1));
        if !top.is_empty() {
            let names: Vec<String> = top
                .iter()
                .take(4)
                .map(|(n, a)| format!("{n} {}", duration(*a)))
                .collect();
            lines.push(format!("held by  {}", names.join(", ")));
        }
    }
    let net: Vec<String> = u
        .net
        .iter()
        .filter(|(_, b)| mib(b.rx + b.tx) >= 0.05)
        .map(|(name, b)| format!("{name} {:.1} MB", mib(b.rx + b.tx)))
        .collect();
    if !net.is_empty() {
        lines.push(format!("network  {}", net.join(", ")));
    }
    lines
}

/// Totals per state over `buckets`, with what the hardware did.
pub fn summary(buckets: &[Bucket], title: &str) -> String {
    let mut out = String::new();
    let mut builds: Vec<&str> = buckets.iter().map(|b| b.build.as_str()).collect();
    builds.sort();
    builds.dedup();
    let _ = writeln!(out, "{title}");
    match builds.as_slice() {
        [] => {
            let _ = writeln!(out, "nothing booked yet");
            return out;
        }
        [one] => {
            let _ = writeln!(out, "build {one}");
        }
        many => {
            let latest = buckets
                .iter()
                .max_by_key(|b| b.hour)
                .map(|b| b.build.as_str());
            let _ = writeln!(
                out,
                "{} builds, latest {} (ledger builds compares them)",
                many.len(),
                latest.unwrap_or("")
            );
        }
    }
    if let Some((a, b)) = levels(buckets) {
        let _ = writeln!(out, "battery {a:.0}% → {b:.0}%");
    }
    let states = by_state(buckets);
    let used: f64 = states
        .iter()
        .filter(|(s, _)| **s != State::Charging)
        .map(|(_, b)| b.wh)
        .sum();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<12}{:>7}{:>11}{:>7}{:>10}",
        "", "time", "energy", "share", "average"
    );
    for (state, b) in &states {
        if *state == State::Charging {
            let _ = writeln!(
                out,
                "{:<12}{:>7}{:>11}",
                state.label(),
                duration(b.seconds),
                format!("{:.2} Wh in", -b.wh)
            );
        } else {
            let _ = writeln!(
                out,
                "{:<12}{:>7}{:>11}{:>7}{:>10}",
                state.label(),
                duration(b.seconds),
                format!("{:.2} Wh", b.wh),
                percent(b.wh, used),
                format!("{:.3} W", watts(b))
            );
        }
    }
    for (state, b) in &states {
        let lines = hardware(*state, b);
        if lines.is_empty() {
            continue;
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", state.label());
        for l in lines {
            let _ = writeln!(out, "  {l}");
        }
    }
    let gauge: f64 = states
        .iter()
        .filter(|(s, _)| **s != State::Charging)
        .map(|(_, b)| b.gauge_wh)
        .sum();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "gauge    booked {used:.2} Wh, the gauge's own count {gauge:.2} Wh"
    );
    out
}

/// A line per hour: the level and each state's time and average.
pub fn hours(buckets: &[Bucket]) -> String {
    let mut out = String::new();
    let mut by_hour: BTreeMap<i64, Vec<&Bucket>> = BTreeMap::new();
    for b in buckets {
        by_hour.entry(b.hour).or_default().push(b);
    }
    let _ = write!(out, "{:<12}{:<11}", "hour", "battery");
    for s in State::ALL {
        let _ = write!(out, "{:<17}", s.label());
    }
    let _ = writeln!(out);
    for (hour, list) in by_hour {
        let owned: Vec<Bucket> = list.iter().map(|b| (*b).clone()).collect();
        let level = levels(&owned)
            .map(|(a, b)| format!("{a:.0}→{b:.0}%"))
            .unwrap_or_default();
        let _ = write!(out, "{:<12}{:<11}", local(hour, "%a %H:%M"), level);
        let states = by_state(list);
        for s in State::ALL {
            let cell = match states.get(&s) {
                Some(b) if b.seconds >= 1.0 && s == State::Charging => {
                    format!("{} {:.1} Wh", duration(b.seconds), -b.wh)
                }
                Some(b) if b.seconds >= 1.0 => {
                    format!("{} {:.3} W", duration(b.seconds), watts(b))
                }
                _ => "–".into(),
            };
            let _ = write!(out, "{cell:<17}");
        }
        let _ = writeln!(out);
    }
    out
}

/// Each build's average per state, oldest build first.
pub fn builds(buckets: &[Bucket]) -> String {
    let mut out = String::new();
    let mut order: Vec<(&str, i64)> = Vec::new();
    for b in buckets {
        match order.iter_mut().find(|(name, _)| *name == b.build) {
            Some(entry) => entry.1 = entry.1.min(b.hour),
            None => order.push((&b.build, b.hour)),
        }
    }
    order.sort_by_key(|(_, first)| *first);
    for (build, first) in order {
        let states = by_state(buckets.iter().filter(|b| b.build == build));
        let _ = writeln!(out, "{build}");
        let _ = write!(out, "  from {:<14}", local(first, "%a %d %H:%M"));
        for s in [State::ScreenOn, State::ScreenOff, State::Asleep] {
            let cell = match states.get(&s) {
                Some(b) if b.seconds >= 1.0 => {
                    format!("{} {:.3} W {}", s.label(), watts(b), duration(b.seconds))
                }
                _ => format!("{} –", s.label()),
            };
            let _ = write!(out, "{cell:<28}");
        }
        let _ = writeln!(out);
    }
    if out.is_empty() {
        out.push_str("nothing booked yet\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(state: State, seconds: f64, wh: f64) -> Bucket {
        let mut b = Bucket::new(3600, state, "k1");
        b.seconds = seconds;
        b.wh = wh;
        b.gauge_wh = wh;
        b.level = Some([90.0, 80.0]);
        b
    }

    #[test]
    fn durations() {
        assert_eq!(duration(35.0), "35s");
        assert_eq!(duration(600.0), "10m");
        assert_eq!(duration(3.0 * 3600.0 + 12.0 * 60.0), "3h12");
    }

    #[test]
    fn summary_shares() {
        let text = summary(
            &[
                bucket(State::ScreenOn, 3600.0, 0.9),
                bucket(State::ScreenOff, 3600.0, 0.1),
            ],
            "test",
        );
        assert!(text.contains("build k1"), "{text}");
        assert!(text.contains("90%"), "{text}");
        assert!(text.contains("0.900 W"), "{text}");
        assert!(text.contains("battery 90% → 80%"), "{text}");
    }
}
