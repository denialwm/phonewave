pub mod backend;
pub mod engine;
pub mod service;

use serde::{Deserialize, Serialize};
use zbus::zvariant::Type;

pub const BUS_NAME: &str = "org.hapticd";
pub const OBJECT_PATH: &str = "/org/hapticd/Haptics";
pub const INTERFACE: &str = "org.hapticd.Haptics1";
pub const MAX_DURATION_MS: u32 = 30_000;
pub const MAX_STEPS: usize = 64;

/// Milliseconds and relative amplitude. Zero amplitude is a pause.
pub type Step = (u32, f64);

#[derive(Clone, Debug, Deserialize, Serialize, Type, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub intensity: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub default_device: String,
    /// Additional callers for headless systems. Root and active local sessions
    /// are already authorized; never accept a UID supplied by a bus client.
    #[serde(default)]
    pub allow_uids: Vec<u32>,
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub id: String,
    pub backend: String,
    pub path: std::path::PathBuf,
    #[serde(default)]
    pub gain: Option<String>,
    #[serde(default)]
    pub gain_max: Option<u32>,
}

pub fn validate_pattern(steps: &[Step]) -> Result<(), String> {
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(format!("a pattern requires 1..={MAX_STEPS} steps"));
    }
    let mut total = 0u64;
    for &(duration, intensity) in steps {
        if duration == 0 || !intensity.is_finite() || !(0.0..=1.0).contains(&intensity) {
            return Err(
                "duration must be positive and intensity must be finite, within 0..=1".into(),
            );
        }
        total += u64::from(duration);
    }
    if total > u64::from(MAX_DURATION_MS) {
        return Err(format!("a pattern may last at most {MAX_DURATION_MS} ms"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_reject_invalid_or_unbounded_requests() {
        for pattern in [
            vec![],
            vec![(0, 1.0)],
            vec![(1, f64::NAN)],
            vec![(1, f64::INFINITY)],
            vec![(1, -0.1)],
            vec![(1, 1.1)],
            vec![(u32::MAX, 1.0)],
            vec![(1, 0.5); 65],
            vec![(20_000, 1.0), (20_000, 0.0)],
        ] {
            assert!(validate_pattern(&pattern).is_err(), "{pattern:?}");
        }
        assert!(validate_pattern(&[(10, 0.5), (50, 0.0), (10, 1.0)]).is_ok());
    }
}
