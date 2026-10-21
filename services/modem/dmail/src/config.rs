//! The device file, the APN override and the user's settings.

use std::path::Path;
use std::{fs, io};

use serde::Deserialize;

use crate::data::{ENDPOINT_EMBEDDED, Endpoint};

pub const DEVICE_FILE: &str = "/etc/dmail/device.toml";
pub const APN_FILE: &str = "/etc/dmail/apn.toml";
pub const STATE_DIR: &str = "/var/lib/dmail";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub data: DataConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataConfig {
    pub ipa_netdev: String,
    pub dpm: Dpm,
    pub qmap: String,
    pub dl_datagrams: u32,
    pub dl_datagram_size: u32,
    pub mtu_max: u32,
}

/// The hardware data port, as `qmicli --dpm-open-port` names its parts.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dpm {
    pub ep_type: String,
    pub iface: u32,
    pub rx: u32,
    pub tx: u32,
}

impl Device {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let device: Device =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        device.endpoint()?;
        if device.data.qmap != "v5" {
            return Err(format!(
                "{}: only qmap = \"v5\" is supported",
                path.display()
            ));
        }
        Ok(device)
    }

    pub fn endpoint(&self) -> Result<Endpoint, String> {
        let kind = match self.data.dpm.ep_type.as_str() {
            "embedded" => ENDPOINT_EMBEDDED,
            other => return Err(format!("unknown data endpoint type {other:?}")),
        };
        Ok(Endpoint {
            kind,
            iface: self.data.dpm.iface,
        })
    }
}

/// `apn = "name"` in the APN file, if there is one.
pub fn apn_override(path: &Path) -> Result<Option<String>, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Apn {
        apn: String,
    }
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Apn>(&text)
            .map(|a| Some(a.apn))
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub const DATA_ENABLED: &str = "data-enabled";
pub const DELIVERY_REPORTS: &str = "delivery-reports";
pub const SIM_SLOT: &str = "sim-slot";
/// The SIM slots a phone can have, 1-based.
pub const SIM_SLOTS: std::ops::RangeInclusive<u8> = 1..=2;

/// What the user chose. Each setting is a number in a file of its own in the
/// state directory; a missing or unreadable file means the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Mobile data: on by default, as on any phone.
    pub data_enabled: bool,
    /// Off by default: every report is an SMS from the network.
    pub delivery_reports: bool,
    /// The slot whose SIM carries SMS and data.
    pub sim_slot: u8,
}

impl Settings {
    pub fn load(dir: &Path) -> Self {
        let read = |name: &str| {
            fs::read_to_string(dir.join(name))
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok())
        };
        Self {
            data_enabled: read(DATA_ENABLED) != Some(0),
            delivery_reports: read(DELIVERY_REPORTS) == Some(1),
            sim_slot: read(SIM_SLOT)
                .filter(|s| SIM_SLOTS.contains(s))
                .unwrap_or(1),
        }
    }
}

/// Saves one setting, replacing its file whole.
pub fn save(dir: &Path, name: &str, value: u8) -> io::Result<()> {
    let tmp = dir.join(format!("{name}.new"));
    fs::write(&tmp, format!("{value}\n"))?;
    fs::rename(tmp, dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_design_example() {
        let text = r#"
[data]
ipa_netdev = "rmnet_ipa0"
dpm = { ep_type = "embedded", iface = 1, rx = 2, tx = 24 }
qmap = "v5"
dl_datagrams = 16
dl_datagram_size = 8192
mtu_max = 1492
"#;
        let device: Device = toml::from_str(text).unwrap();
        assert_eq!(device.data.dpm.tx, 24);
        assert_eq!(device.endpoint().unwrap().kind, ENDPOINT_EMBEDDED);
    }

    #[test]
    fn settings_default_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("dmail-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let defaults = Settings {
            data_enabled: true,
            delivery_reports: false,
            sim_slot: 1,
        };
        assert_eq!(Settings::load(&dir), defaults);
        save(&dir, DATA_ENABLED, 0).unwrap();
        save(&dir, DELIVERY_REPORTS, 1).unwrap();
        save(&dir, SIM_SLOT, 2).unwrap();
        let saved = Settings::load(&dir);
        assert_eq!(
            (saved.data_enabled, saved.delivery_reports, saved.sim_slot),
            (false, true, 2)
        );
        save(&dir, SIM_SLOT, 9).unwrap();
        assert_eq!(Settings::load(&dir).sim_slot, 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
