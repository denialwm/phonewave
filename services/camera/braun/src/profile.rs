//! What the kernel does not say about a camera, from two kinds of file.
//!
//! A sensor profile holds the facts of a sensor model on any phone: its
//! black and white level and how its analogue gain codes map to gain. They
//! come with braun, one file per driver name, in `/usr/share/braun/sensors`.
//!
//! A calibration file holds what one phone's module adds: its role among
//! the phone's cameras, the focus travel worth sweeping, the colour of its
//! light under known illuminants, its colour matrices and its lens shading.
//! It comes with the device, one file per camera, in `/etc/braun`. Without
//! one the camera still works, untuned.

use std::path::Path;

use serde::Deserialize;

use crate::camera::{Ccm, Colour, LocusPoint, Role, SHADING_NODES, Shading};

pub const SENSORS: &str = "/usr/share/braun/sensors";
pub const CALIBRATION: &str = "/etc/braun";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorProfile {
    /// The black level, in 10-bit codes.
    pub black: Option<f32>,
    /// A raw sample at or over this is clipped.
    pub white: Option<f32>,
    pub analogue_gain: Option<GainModel>,
    /// The most analogue gain auto exposure uses.
    pub max_gain: Option<f32>,
}

/// gain = (m0 x code + c0) / (m1 x code + c1), as SMIA sensors describe it:
/// a linear code has m1 = 0, a Sony one m0 = 0.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GainModel {
    pub m0: f32,
    pub c0: f32,
    pub m1: f32,
    pub c1: f32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub role: Option<Role>,
    /// Overrides the kernel's `camera_sensor_rotation` (counter-clockwise
    /// correction, as the kernel states it) where the device tree is wrong.
    pub rotation: Option<u32>,
    pub focus: Option<Focus>,
    #[serde(default)]
    pub locus: Vec<LocusPoint>,
    #[serde(default)]
    pub ccm: Vec<Ccm>,
    #[serde(default)]
    pub shading: Vec<ShadingMap>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Focus {
    /// The lens positions the focus sweep covers.
    pub travel: [u32; 2],
    /// Where the lens starts, before the first sweep ends.
    pub start: Option<u32>,
}

/// Lens shading gains under a light of `ct` kelvin, row by row over the
/// grid, corner to corner over the sensor.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadingMap {
    pub ct: f32,
    pub red: Vec<f32>,
    pub green: Vec<f32>,
    pub blue: Vec<f32>,
}

fn load<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// The profile of sensor model `model` (its driver's name), if braun has one.
pub fn sensor(model: &str) -> Result<Option<SensorProfile>, String> {
    load(&Path::new(SENSORS).join(model).with_extension("toml"))
}

/// The device's calibration of camera `id`, if it has one.
pub fn calibration(id: &str) -> Result<Option<Calibration>, String> {
    load(&Path::new(CALIBRATION).join(id).with_extension("toml"))
}

impl Calibration {
    /// Its colour, checked: every list warm to cool, every map the grid's size.
    pub fn colour(&self) -> Result<Colour, String> {
        let warm_to_cool = |cts: &mut dyn Iterator<Item = f32>| {
            let cts: Vec<f32> = cts.collect();
            cts.windows(2).all(|pair| pair[0] < pair[1])
        };
        if !warm_to_cool(&mut self.locus.iter().map(|point| point.ct))
            || !warm_to_cool(&mut self.ccm.iter().map(|ccm| ccm.ct))
            || !warm_to_cool(&mut self.shading.iter().map(|map| map.ct))
        {
            return Err("locus, ccm and shading go warm to cool".into());
        }
        if self.locus.len() == 1 {
            return Err("a locus needs two points or none".into());
        }
        let mut shading = Vec::with_capacity(self.shading.len());
        for map in &self.shading {
            let channel = |values: &[f32]| -> Result<[f32; SHADING_NODES], String> {
                values.try_into().map_err(|_| {
                    format!(
                        "shading at {} K: {} gains, not {SHADING_NODES}",
                        map.ct,
                        values.len()
                    )
                })
            };
            shading.push(Shading {
                ct: map.ct,
                gains: [
                    channel(&map.red)?,
                    channel(&map.green)?,
                    channel(&map.blue)?,
                ],
            });
        }
        let untuned = Colour::untuned();
        Ok(Colour {
            locus: self.locus.clone(),
            ccms: if self.ccm.is_empty() {
                untuned.ccms
            } else {
                self.ccm.clone()
            },
            shading,
        })
    }

    pub fn has_colour(&self) -> bool {
        !self.locus.is_empty() || !self.ccm.is_empty() || !self.shading.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Calibration, SensorProfile};

    #[test]
    fn sensor_profiles_parse() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/sensors");
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let profile: SensorProfile =
                toml::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            assert!(profile.analogue_gain.is_some(), "{}", path.display());
            seen += 1;
        }
        assert!(seen > 0);
    }

    #[test]
    fn device_calibrations_parse() {
        let devices = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../devices");
        let mut seen = 0;
        for device in std::fs::read_dir(devices).unwrap() {
            let Ok(entries) = std::fs::read_dir(device.unwrap().path().join("rootfs/etc/braun"))
            else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                let text = std::fs::read_to_string(&path).unwrap();
                let calibration: Calibration = toml::from_str(&text)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                calibration
                    .colour()
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                seen += 1;
            }
        }
        assert!(seen > 0);
    }

    #[test]
    fn short_shading_is_refused() {
        let calibration: Calibration =
            toml::from_str("[[shading]]\nct = 5000\nred = [1.0]\ngreen = [1.0]\nblue = [1.0]\n")
                .unwrap();
        assert!(calibration.colour().is_err());
    }
}
