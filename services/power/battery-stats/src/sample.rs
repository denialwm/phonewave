//! What the phone is doing right now: the clocks, the battery and the
//! screen. Cheap enough to read every few seconds.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sysfs::{self, entries, number, read};

/// What the phone is doing, as far as the battery is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    ScreenOn,
    ScreenOff,
    /// Suspended. Never read from a sample: the clocks show it afterwards.
    Asleep,
    /// On a charger, whatever the screen does.
    Charging,
}

impl State {
    pub const ALL: [State; 4] = [
        State::ScreenOn,
        State::ScreenOff,
        State::Asleep,
        State::Charging,
    ];

    pub fn label(self) -> &'static str {
        match self {
            State::ScreenOn => "screen on",
            State::ScreenOff => "screen off",
            State::Asleep => "asleep",
            State::Charging => "charging",
        }
    }
}

/// Seconds on a clock.
pub fn clock(id: libc::clockid_t) -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a valid timespec to write into.
    unsafe { libc::clock_gettime(id, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

/// The phone's battery and the supplies that can charge it.
#[derive(Debug)]
pub struct Power {
    battery: PathBuf,
    chargers: Vec<PathBuf>,
}

impl Power {
    /// The first system battery that reports its current and voltage.
    pub fn find() -> Option<Power> {
        let supplies = entries("/sys/class/power_supply");
        let battery = supplies
            .iter()
            .find(|p| {
                read(p.join("type")).as_deref() == Some("Battery")
                    && read(p.join("scope")).as_deref() != Some("Device")
                    && p.join("current_now").exists()
                    && p.join("voltage_now").exists()
            })?
            .clone();
        let chargers = supplies
            .into_iter()
            .filter(|p| {
                matches!(
                    read(p.join("type")).as_deref(),
                    Some("Mains" | "USB" | "Wireless")
                ) && p.join("online").exists()
            })
            .collect();
        Some(Power { battery, chargers })
    }

    pub fn battery(&self) -> &Path {
        &self.battery
    }
}

/// The display connectors and backlights.
#[derive(Debug)]
pub struct Screen {
    connectors: Vec<PathBuf>,
    backlights: Vec<PathBuf>,
}

impl Screen {
    pub fn find() -> Screen {
        let connectors = entries("/sys/class/drm")
            .into_iter()
            .filter(|p| {
                let name = sysfs::name(p);
                name.starts_with("card") && name.contains('-') && !name.contains("Writeback")
            })
            .filter(|p| p.join("dpms").exists())
            .collect();
        Screen {
            connectors,
            backlights: entries("/sys/class/backlight"),
        }
    }

    /// Whether anything lights the screen, and the first backlight's level
    /// as a fraction of its range.
    fn read(&self) -> (bool, Option<f64>) {
        let connected: Vec<&PathBuf> = self
            .connectors
            .iter()
            .filter(|c| read(c.join("status")).as_deref() == Some("connected"))
            .collect();
        let shown = connected.is_empty()
            || connected.iter().any(|c| {
                read(c.join("enabled")).as_deref() == Some("enabled")
                    && read(c.join("dpms")).as_deref() == Some("On")
            });
        let mut brightness = None;
        let mut lit = self.backlights.is_empty();
        for b in &self.backlights {
            let level: f64 = number(b.join("brightness")).unwrap_or(0.0);
            let max: f64 = number(b.join("max_brightness")).unwrap_or(0.0);
            let power: u32 = number(b.join("bl_power")).unwrap_or(0);
            if brightness.is_none() && max > 0.0 {
                brightness = Some(level / max);
            }
            lit |= level > 0.0 && power == 0;
        }
        (shown && lit, brightness)
    }
}

#[derive(Clone, Debug)]
pub struct Sample {
    /// CLOCK_MONOTONIC, which stops while the phone sleeps.
    pub mono: f64,
    /// CLOCK_BOOTTIME, which does not.
    pub boot: f64,
    /// Unix time.
    pub real: f64,
    /// Out of the battery; negative while it charges.
    pub watts: f64,
    pub volts: f64,
    /// What the gauge says is left: watt-hours if it counts energy...
    pub energy_wh: Option<f64>,
    /// ...or else amp-hours.
    pub charge_ah: Option<f64>,
    /// Percent.
    pub level: Option<f64>,
    pub plugged: bool,
    pub screen: bool,
    pub brightness: Option<f64>,
}

impl Sample {
    pub fn read(power: &Power, screen: &Screen) -> Sample {
        let mono = clock(libc::CLOCK_MONOTONIC);
        let boot = clock(libc::CLOCK_BOOTTIME);
        let real = clock(libc::CLOCK_REALTIME);
        let b = &power.battery;
        let amps = number::<f64>(b.join("current_now")).unwrap_or(0.0) * 1e-6;
        let volts = number::<f64>(b.join("voltage_now")).unwrap_or(0.0) * 1e-6;
        let status = read(b.join("status")).unwrap_or_default();
        let magnitude = (amps * volts).abs();
        let watts = match status.as_str() {
            "Charging" => -magnitude,
            "Discharging" => magnitude,
            // The common sign: negative current leaves the battery.
            _ => -amps * volts,
        };
        let charger = power
            .chargers
            .iter()
            .any(|c| number::<u32>(c.join("online")) == Some(1));
        let (screen_on, brightness) = screen.read();
        Sample {
            mono,
            boot,
            real,
            watts,
            volts,
            energy_wh: number::<f64>(b.join("energy_now")).map(|v| v * 1e-6),
            charge_ah: number::<f64>(b.join("charge_counter"))
                .or_else(|| number(b.join("charge_now")))
                .map(|v| v * 1e-6),
            level: number(b.join("capacity")),
            plugged: charger || matches!(status.as_str(), "Charging" | "Full"),
            screen: screen_on,
            brightness,
        }
    }

    pub fn state(&self) -> State {
        if self.plugged {
            State::Charging
        } else if self.screen {
            State::ScreenOn
        } else {
            State::ScreenOff
        }
    }

    /// Seconds the phone slept since `before`.
    pub fn slept_since(&self, before: &Sample) -> f64 {
        ((self.boot - before.boot) - (self.mono - before.mono)).max(0.0)
    }

    /// The energy the gauge says left the battery since `before`, if it
    /// counts.
    pub fn gauge_wh_since(&self, before: &Sample) -> Option<f64> {
        if let (Some(a), Some(b)) = (before.energy_wh, self.energy_wh) {
            return Some(a - b);
        }
        let (a, b) = (before.charge_ah?, self.charge_ah?);
        Some((a - b) * (before.volts + self.volts) / 2.0)
    }
}
