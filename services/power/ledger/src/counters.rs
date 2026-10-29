//! The kernel's running totals of what the hardware did: CPU time at each
//! frequency, idle time, how long devices were powered, DDR frequencies,
//! how long the Qualcomm subsystems slept, wake-up sources and network
//! bytes. Two readings make a [`Usage`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sysfs::{self, cpu_list, entries, number, read};

/// The rate of the Qualcomm sleep counters in `qcom_stats` and `ddr_stats`.
pub const QCOM_TICKS: f64 = 19_200_000.0;

const QCOM_STATS: &str = "/sys/kernel/debug/qcom_stats";
const ENERGY_MODEL: &str = "/sys/kernel/debug/energy_model";

/// A cpufreq policy and, from the kernel's energy model, what one of its
/// CPUs draws busy at each frequency.
#[derive(Debug)]
struct Policy {
    /// `cpu0-5`.
    name: String,
    path: PathBuf,
    cpus: Vec<u32>,
    /// kHz to µW.
    power: BTreeMap<u32, f64>,
}

/// Where the counters are on this phone, found once.
#[derive(Debug)]
pub struct Hardware {
    /// CPU number and its directory.
    cpus: Vec<(u32, PathBuf)>,
    policies: Vec<Policy>,
    /// A label and the device's `power/` directory.
    devices: Vec<(String, PathBuf)>,
    qcom_stats: bool,
}

/// One reading of every counter.
#[derive(Clone, Debug, Default)]
pub struct Counters {
    mono: f64,
    /// Microseconds idle, for each online CPU.
    idle: BTreeMap<u32, u64>,
    /// For each policy, kHz to time in 10 ms units.
    time_in_state: Vec<BTreeMap<u32, u64>>,
    /// Milliseconds active and suspended, for each device.
    devices: Vec<(u64, u64)>,
    /// MHz to ticks.
    ddr: BTreeMap<u32, u64>,
    /// Subsystem to ticks asleep.
    sleep: BTreeMap<String, u64>,
    /// Source to events, wake-ups and milliseconds active.
    wakeups: BTreeMap<String, (u64, u64, u64)>,
    /// Interface to bytes received and sent.
    net: BTreeMap<String, (u64, u64)>,
}

/// One CPU cluster's work.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    pub cpus: u32,
    /// CPU-seconds not idle.
    pub busy: f64,
    /// MHz to seconds at it.
    pub at: BTreeMap<u32, f64>,
    /// Joules the energy model puts on the busy time, where there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_j: Option<f64>,
}

/// Seconds a device was powered, out of the seconds counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OnTime {
    pub active: f64,
    pub total: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wake {
    pub events: u64,
    /// Events that woke the phone or stopped it sleeping.
    pub wakeups: u64,
    /// Seconds it held the phone awake.
    pub active: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bytes {
    pub rx: u64,
    pub tx: u64,
}

/// What the hardware did between two readings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cpu: BTreeMap<String, Cluster>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub on: BTreeMap<String, OnTime>,
    /// MHz to seconds.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ddr: BTreeMap<u32, f64>,
    /// Subsystem to seconds asleep.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sleep: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub wakeups: BTreeMap<String, Wake>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub net: BTreeMap<String, Bytes>,
}

/// A counter's growth; one that went back started over.
fn grew(before: u64, after: u64) -> u64 {
    after.checked_sub(before).unwrap_or(after)
}

impl Hardware {
    pub fn find() -> Hardware {
        let cpus = entries("/sys/devices/system/cpu")
            .into_iter()
            .filter_map(|p| {
                let id = sysfs::name(&p).strip_prefix("cpu")?.parse().ok()?;
                p.join("cpuidle").exists().then_some((id, p))
            })
            .collect();
        let models = energy_models();
        let policies = entries("/sys/devices/system/cpu/cpufreq")
            .into_iter()
            .filter(|p| p.join("stats/time_in_state").exists())
            .map(|path| {
                let cpus = cpu_list(&read(path.join("related_cpus")).unwrap_or_default());
                let name = match (cpus.first(), cpus.last()) {
                    (Some(a), Some(b)) if a != b => format!("cpu{a}-{b}"),
                    (Some(a), _) => format!("cpu{a}"),
                    _ => sysfs::name(&path),
                };
                let power = models
                    .iter()
                    .find(|(c, _)| cpus.first().is_some_and(|f| c.contains(f)))
                    .map(|(_, p)| p.clone())
                    .unwrap_or_default();
                Policy {
                    name,
                    path,
                    cpus,
                    power,
                }
            })
            .collect();
        Hardware {
            cpus,
            policies,
            devices: devices(),
            qcom_stats: Path::new(QCOM_STATS).exists(),
        }
    }

    pub fn read(&self, mono: f64) -> Counters {
        let mut idle = BTreeMap::new();
        for (id, dir) in &self.cpus {
            if read(dir.join("online")).as_deref() == Some("0") {
                continue;
            }
            let total = entries(dir.join("cpuidle"))
                .iter()
                .filter(|s| sysfs::name(s).starts_with("state"))
                .filter_map(|s| number::<u64>(s.join("time")))
                .sum();
            idle.insert(*id, total);
        }
        let time_in_state = self
            .policies
            .iter()
            .map(|p| {
                read(p.path.join("stats/time_in_state"))
                    .map(|s| pairs(&s))
                    .unwrap_or_default()
            })
            .collect();
        let devices = self
            .devices
            .iter()
            .map(|(_, power)| {
                (
                    number(power.join("runtime_active_time")).unwrap_or(0),
                    number(power.join("runtime_suspended_time")).unwrap_or(0),
                )
            })
            .collect();
        let (ddr, sleep) = if self.qcom_stats {
            (
                read(Path::new(QCOM_STATS).join("ddr_stats"))
                    .map(|s| ddr_stats(&s))
                    .unwrap_or_default(),
                qcom_sleep(),
            )
        } else {
            Default::default()
        };
        Counters {
            mono,
            idle,
            time_in_state,
            devices,
            ddr,
            sleep,
            wakeups: wakeups(),
            net: net(),
        }
    }

    /// What happened between `a` and `b`.
    pub fn usage(&self, a: &Counters, b: &Counters) -> Usage {
        let seconds = (b.mono - a.mono).max(0.0);
        let mut usage = Usage::default();
        for (i, policy) in self.policies.iter().enumerate() {
            let (Some(ta), Some(tb)) = (a.time_in_state.get(i), b.time_in_state.get(i)) else {
                continue;
            };
            let busy: f64 = policy
                .cpus
                .iter()
                .filter_map(|c| Some((a.idle.get(c)?, b.idle.get(c)?)))
                .map(|(ia, ib)| (seconds - grew(*ia, *ib) as f64 * 1e-6).max(0.0))
                .sum();
            let mut at = BTreeMap::new();
            for (khz, t) in tb {
                let spent = grew(ta.get(khz).copied().unwrap_or(0), *t) as f64 * 0.01;
                if spent > 0.0 {
                    *at.entry(khz / 1000).or_insert(0.0) += spent;
                }
            }
            let model_j = (!policy.power.is_empty()).then(|| {
                let total: f64 = at.values().sum();
                if total <= 0.0 {
                    return 0.0;
                }
                let mean_uw: f64 = tb
                    .iter()
                    .map(|(khz, t)| {
                        let share =
                            grew(ta.get(khz).copied().unwrap_or(0), *t) as f64 * 0.01 / total;
                        share * power_at(&policy.power, *khz)
                    })
                    .sum();
                busy * mean_uw * 1e-6
            });
            usage.cpu.insert(
                policy.name.clone(),
                Cluster {
                    cpus: policy.cpus.len() as u32,
                    busy,
                    at,
                    model_j,
                },
            );
        }
        for (i, (label, _)) in self.devices.iter().enumerate() {
            let (Some(da), Some(db)) = (a.devices.get(i), b.devices.get(i)) else {
                continue;
            };
            let active = grew(da.0, db.0) as f64 * 1e-3;
            let total = active + grew(da.1, db.1) as f64 * 1e-3;
            if total > 0.0 {
                usage.on.insert(label.clone(), OnTime { active, total });
            }
        }
        for (mhz, t) in &b.ddr {
            let spent = grew(a.ddr.get(mhz).copied().unwrap_or(0), *t) as f64 / QCOM_TICKS;
            if spent > 0.0 {
                usage.ddr.insert(*mhz, spent);
            }
        }
        for (name, t) in &b.sleep {
            let spent = grew(a.sleep.get(name).copied().unwrap_or(0), *t) as f64 / QCOM_TICKS;
            usage.sleep.insert(name.clone(), spent);
        }
        for (name, (events, wakeups, active)) in &b.wakeups {
            let before = a.wakeups.get(name).copied().unwrap_or_default();
            let wake = Wake {
                events: grew(before.0, *events),
                wakeups: grew(before.1, *wakeups),
                active: grew(before.2, *active) as f64 * 1e-3,
            };
            if wake.events > 0 || wake.wakeups > 0 || wake.active > 0.0 {
                usage.wakeups.insert(name.clone(), wake);
            }
        }
        for (name, (rx, tx)) in &b.net {
            let before = a.net.get(name).copied().unwrap_or_default();
            let bytes = Bytes {
                rx: grew(before.0, *rx),
                tx: grew(before.1, *tx),
            };
            if bytes.rx > 0 || bytes.tx > 0 {
                usage.net.insert(name.clone(), bytes);
            }
        }
        usage
    }
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        for (name, c) in &other.cpu {
            let mine = self.cpu.entry(name.clone()).or_default();
            mine.cpus = c.cpus;
            mine.busy += c.busy;
            for (mhz, s) in &c.at {
                *mine.at.entry(*mhz).or_insert(0.0) += s;
            }
            if let Some(j) = c.model_j {
                *mine.model_j.get_or_insert(0.0) += j;
            }
        }
        for (name, t) in &other.on {
            let mine = self.on.entry(name.clone()).or_default();
            mine.active += t.active;
            mine.total += t.total;
        }
        for (mhz, s) in &other.ddr {
            *self.ddr.entry(*mhz).or_insert(0.0) += s;
        }
        for (name, s) in &other.sleep {
            *self.sleep.entry(name.clone()).or_insert(0.0) += s;
        }
        for (name, w) in &other.wakeups {
            let mine = self.wakeups.entry(name.clone()).or_default();
            mine.events += w.events;
            mine.wakeups += w.wakeups;
            mine.active += w.active;
        }
        for (name, b) in &other.net {
            let mine = self.net.entry(name.clone()).or_default();
            mine.rx += b.rx;
            mine.tx += b.tx;
        }
    }
}

/// Lines of two numbers, as in `time_in_state`.
fn pairs(text: &str) -> BTreeMap<u32, u64> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect()
}

/// The power at a frequency, or at the nearest one the model has.
fn power_at(power: &BTreeMap<u32, f64>, khz: u32) -> f64 {
    if let Some(p) = power.get(&khz) {
        return *p;
    }
    power
        .iter()
        .min_by_key(|(f, _)| f.abs_diff(khz))
        .map(|(_, p)| *p)
        .unwrap_or(0.0)
}

/// The kernel's energy model: for each performance domain its CPUs and
/// what one of them draws busy at each frequency, when the model is in µW.
fn energy_models() -> Vec<(Vec<u32>, BTreeMap<u32, f64>)> {
    entries(ENERGY_MODEL)
        .into_iter()
        .filter_map(|domain| {
            // Bit 0: the model is in microwatts, not an abstract scale.
            let flags = read(domain.join("flags"))?;
            let flags = u64::from_str_radix(flags.trim_start_matches("0x"), 16).ok()?;
            if flags & 1 == 0 {
                return None;
            }
            let cpus = cpu_list(&read(domain.join("cpus"))?);
            let power = entries(&domain)
                .iter()
                .filter_map(|ps| {
                    let khz = sysfs::name(ps).strip_prefix("ps:")?.parse().ok()?;
                    Some((khz, number::<f64>(ps.join("power"))?))
                })
                .collect();
            Some((cpus, power))
        })
        .collect()
}

/// The devices whose powered time is worth counting: the GPU and storage
/// (by their devfreq), the display controller and the Wi-Fi card.
fn devices() -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for d in entries("/sys/class/devfreq") {
        let name = sysfs::name(&d);
        let label = name.rsplit('.').next().unwrap_or(&name).to_owned();
        found.push((label, d.join("device/power")));
    }
    for card in entries("/sys/class/drm") {
        let name = sysfs::name(&card);
        if name.starts_with("card") && !name.contains('-') {
            found.push(("display".to_owned(), card.join("device/power")));
        }
    }
    for net in entries("/sys/class/net") {
        if net.join("wireless").exists() || net.join("phy80211").exists() {
            found.push(("wifi".to_owned(), net.join("device/power")));
        }
    }
    // One entry per device, and only those with runtime power management.
    let mut seen = Vec::new();
    found.retain(|(_, power)| {
        let real = power.canonicalize().ok();
        let fresh = real.is_some() && !seen.contains(&real);
        seen.push(real);
        fresh && power.join("runtime_active_time").exists()
    });
    found
}

/// `ddr_stats`: lines like `DDR Freq 547Mhz:\tCP IDX:1\tcount:1\tDuration (ticks):314255872`.
fn ddr_stats(text: &str) -> BTreeMap<u32, u64> {
    text.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("DDR Freq ")?;
            let mhz = rest.split("Mhz").next()?.trim().parse().ok()?;
            let ticks = rest
                .rsplit("Duration (ticks):")
                .next()?
                .trim()
                .parse()
                .ok()?;
            Some((mhz, ticks))
        })
        .collect()
}

/// Each `qcom_stats` subsystem's `Accumulated Duration`.
fn qcom_sleep() -> BTreeMap<String, u64> {
    entries(QCOM_STATS)
        .into_iter()
        .filter_map(|p| {
            let name = sysfs::name(&p);
            if name == "ddr_stats" {
                return None;
            }
            Some((name, accumulated(&read(&p)?)?))
        })
        .collect()
}

fn accumulated(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("Accumulated Duration:"))
        .and_then(|v| v.trim().parse().ok())
}

fn wakeups() -> BTreeMap<String, (u64, u64, u64)> {
    let mut all = BTreeMap::new();
    for w in entries("/sys/class/wakeup") {
        let Some(name) = read(w.join("name")) else {
            continue;
        };
        let entry: &mut (u64, u64, u64) = all.entry(name).or_default();
        entry.0 += number::<u64>(w.join("event_count")).unwrap_or(0);
        entry.1 += number::<u64>(w.join("wakeup_count")).unwrap_or(0);
        entry.2 += number::<u64>(w.join("total_time_ms")).unwrap_or(0);
    }
    all
}

/// Bytes through the interfaces on hardware; virtual ones such as
/// Droidloom's carry the same traffic again.
fn net() -> BTreeMap<String, (u64, u64)> {
    entries("/sys/class/net")
        .into_iter()
        .filter(|p| p.join("device").exists())
        .map(|p| {
            (
                sysfs::name(&p),
                (
                    number(p.join("statistics/rx_bytes")).unwrap_or(0),
                    number(p.join("statistics/tx_bytes")).unwrap_or(0),
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ddr_stats() {
        let text = "DDR LPM Stat Name:0xd4\tcount:0\tDuration (ticks):0\n\
                    DDR Freq 547Mhz:\tCP IDX:1\tcount:1\tDuration (ticks):314255872\n\
                    DDR Freq 5333Mhz:\tCP IDX:11\tcount:1\tDuration (ticks):19142059520\n";
        let ddr = ddr_stats(text);
        assert_eq!(ddr.len(), 2);
        assert_eq!(ddr[&547], 314255872);
        assert_eq!(ddr[&5333], 19142059520);
    }

    #[test]
    fn reads_qcom_stats() {
        let text = "Count: 12068\nLast Entered At: 67533464296\nLast Exited At: 67533256691\n\
                    Accumulated Duration: 60114139596\n";
        assert_eq!(accumulated(text), Some(60114139596));
        assert_eq!(accumulated(""), None);
    }

    #[test]
    fn counters_that_restart() {
        assert_eq!(grew(10, 15), 5);
        assert_eq!(grew(10, 3), 3);
    }

    #[test]
    fn nearest_power() {
        let power = BTreeMap::from([(384000, 9754.0), (537600, 13655.0)]);
        assert_eq!(power_at(&power, 537600), 13655.0);
        assert_eq!(power_at(&power, 400000), 9754.0);
    }

    #[test]
    fn usage_adds_up() {
        let mut a = Usage::default();
        a.ddr.insert(547, 1.0);
        a.wakeups.insert(
            "rtc".into(),
            Wake {
                events: 2,
                wakeups: 1,
                active: 0.5,
            },
        );
        let b = a.clone();
        a.add(&b);
        assert_eq!(a.ddr[&547], 2.0);
        assert_eq!(a.wakeups["rtc"].events, 4);
    }
}
