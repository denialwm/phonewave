//! A camera as braun knows it. The kernel says most of it: the sensor's
//! format, line time, exposure and gain ranges, which way it faces and how
//! it is mounted, and its lens. The sensor's profile and the device's
//! calibration (`profile`) add the rest.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::media::{self, ENT_F_CAM_SENSOR, ENT_F_LENS, Entity, Graph};
use crate::profile::{self, Calibration, GainModel};
use crate::v4l2::{self, cid};

/// Without a sensor profile: the usual 10-bit black level, and clipping a
/// little under full scale.
const BLACK: f32 = 64.0;
const WHITE: f32 = 1000.0;
/// Auto exposure uses at most this much analogue gain.
const MAX_ANALOGUE: f32 = 16.0;
/// The graphs braun can route: one CSIPHY to CSID 0 to VFE 0's raw dump.
const CAMSS: &str = "qcom-camss";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    Front,
    Back,
    External,
}

impl Facing {
    pub fn name(self) -> &'static str {
        match self {
            Facing::Front => "front",
            Facing::Back => "back",
            Facing::External => "external",
        }
    }
}

/// What a camera is for among the phone's cameras.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Main,
    Ultrawide,
    Telephoto,
    /// Depth, macro or monochrome: not a camera to show on its own.
    Auxiliary,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Main => "main",
            Role::Ultrawide => "ultrawide",
            Role::Telephoto => "telephoto",
            Role::Auxiliary => "auxiliary",
        }
    }
}

/// The colour filter layout of the sensor's 2x2 cells.
#[derive(Clone, Copy, Debug)]
pub enum Bayer {
    Bggr,
    Gbrg,
    Grbg,
    Rggb,
}

impl Bayer {
    /// The 10-bit Bayer media bus codes.
    fn from_code(code: u32) -> Option<Self> {
        match code {
            0x3007 => Some(Bayer::Bggr),
            0x300e => Some(Bayer::Gbrg),
            0x300a => Some(Bayer::Grbg),
            0x300f => Some(Bayer::Rggb),
            _ => None,
        }
    }

    pub fn mbus_code(self) -> u32 {
        match self {
            Bayer::Bggr => 0x3007,
            Bayer::Gbrg => 0x300e,
            Bayer::Grbg => 0x300a,
            Bayer::Rggb => 0x300f,
        }
    }

    /// The packed 10-bit pixel format the video node writes.
    pub fn fourcc(self) -> u32 {
        let code = match self {
            Bayer::Bggr => *b"pBAA",
            Bayer::Gbrg => *b"pGAA",
            Bayer::Grbg => *b"pgAA",
            Bayer::Rggb => *b"pRAA",
        };
        u32::from_le_bytes(code)
    }

    /// Where red, the two greens and blue sit in a cell, as (x, y).
    pub fn cell(self) -> [(usize, usize); 4] {
        match self {
            Bayer::Bggr => [(1, 1), (1, 0), (0, 1), (0, 0)],
            Bayer::Gbrg => [(0, 1), (0, 0), (1, 1), (1, 0)],
            Bayer::Grbg => [(1, 0), (0, 0), (1, 1), (0, 1)],
            Bayer::Rggb => [(0, 0), (1, 0), (0, 1), (1, 1)],
        }
    }
}

impl GainModel {
    pub fn gain(self, code: u32) -> f32 {
        let code = code as f32;
        (self.m0 * code + self.c0) / (self.m1 * code + self.c1)
    }

    pub fn code(self, gain: f32) -> u32 {
        ((self.c1 * gain - self.c0) / (self.m0 - self.m1 * gain))
            .round()
            .max(0.0) as u32
    }
}

/// One point of the illuminant locus: the R/G and B/G of grey under a
/// light of `ct` kelvin.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocusPoint {
    pub ct: f32,
    pub rg: f32,
    pub bg: f32,
}

/// A colour correction matrix from a light of `ct` kelvin on.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ccm {
    pub ct: f32,
    pub m: [f32; 9],
}

/// Lens shading grid nodes across and down the sensor.
pub const SHADING_COLUMNS: usize = 17;
pub const SHADING_ROWS: usize = 13;
pub const SHADING_NODES: usize = SHADING_COLUMNS * SHADING_ROWS;

/// The gains, red, green and blue, that undo the lens's fall-off under a
/// light of `ct` kelvin: row by row, corner to corner over the sensor.
pub struct Shading {
    pub ct: f32,
    pub gains: [[f32; SHADING_NODES]; 3],
}

pub struct Colour {
    /// Grey under the known illuminants, warm to cool; empty when the
    /// camera has no tuning, which leaves white balance to grey world.
    pub locus: Vec<LocusPoint>,
    /// By colour temperature, warm to cool; one matrix for all lights when
    /// there is one entry.
    pub ccms: Vec<Ccm>,
    /// Lens shading by colour temperature, warm to cool; empty for none.
    pub shading: Vec<Shading>,
}

impl Colour {
    /// No tuning: grey world white balance, the raw colours, no lens
    /// shading.
    pub fn untuned() -> Self {
        Self {
            locus: Vec::new(),
            ccms: vec![Ccm {
                ct: 5000.0,
                m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            }],
            shading: Vec::new(),
        }
    }
}

/// A focus motor.
pub struct Lens {
    /// Its media entity.
    pub entity: String,
    /// The positions the focus sweep covers.
    pub travel: RangeInclusive<u32>,
    /// Lens position before the first focus sweep ends.
    pub start: u32,
}

pub struct Camera {
    /// How clients name it: the sensor's driver, with its I2C address when
    /// the phone has two of the same sensor. Also its calibration's name.
    pub id: String,
    /// The sensor's driver: its profile's name.
    pub model: String,
    /// Shown in messages: the model in capitals.
    pub name: String,
    /// The sensor's media entity and the graph it is in.
    pub entity: String,
    pub media: PathBuf,
    /// The entity the sensor sends to: its CSIPHY.
    pub phy: String,
    pub facing: Facing,
    pub role: Option<Role>,
    /// The zoom at which the main camera shows what this one does; 1 for
    /// the main camera, none when the device does not say.
    pub zoom: Option<f32>,
    pub width: u32,
    pub height: u32,
    pub bayer: Bayer,
    /// In 10-bit codes.
    pub black: f32,
    /// A raw sample at or over this is clipped.
    pub clip: f32,
    /// Line time in microseconds: exposure lines to time.
    pub line_us: f32,
    pub min_lines: u32,
    pub max_lines: u32,
    /// Exposure lines are a multiple of this.
    pub line_step: u32,
    /// How gain codes map to gain; none when nothing says, and the gain
    /// stays as it is.
    pub gain: Option<GainModel>,
    pub gain_codes: RangeInclusive<u32>,
    /// The most analogue gain auto exposure uses.
    pub max_gain: f32,
    /// Clockwise turn that puts the image upright on the phone held
    /// upright.
    pub rotation: u32,
    /// The image is shown and kept mirrored, as a mirror would show it.
    pub mirror: bool,
    pub lens: Option<Lens>,
    pub colour: Colour,
    /// The calibration has its colour.
    pub tuned: bool,
    /// What was assumed for it, or left out.
    pub notes: Vec<String>,
}

impl Camera {
    /// The gain code for `gain`, within the sensor's range; none when the
    /// gain is not known.
    pub fn gain_code(&self, gain: f32) -> Option<u32> {
        self.gain.map(|model| {
            model
                .code(gain)
                .clamp(*self.gain_codes.start(), *self.gain_codes.end())
        })
    }

    /// The gain the sensor really applies when asked for `gain`.
    pub fn applied_gain(&self, gain: f32) -> f32 {
        match (self.gain, self.gain_code(gain)) {
            (Some(model), Some(code)) => model.gain(code),
            _ => 1.0,
        }
    }

    /// The frame's size once turned upright.
    pub fn upright(&self) -> (u32, u32) {
        let (width, height) = turned(self.width as usize, self.height as usize, self.rotation);
        (width as u32, height as u32)
    }
}

/// The cameras found, and the sensors that are not cameras braun can open,
/// with why.
pub struct Discovery {
    pub cameras: Vec<Camera>,
    pub skipped: Vec<String>,
}

/// Every camera the media graphs have.
pub fn discover() -> Discovery {
    let mut cameras = Vec::new();
    let mut skipped = Vec::new();
    for path in media::devices() {
        let graph = match Graph::open(&path) {
            Ok(graph) => graph,
            Err(error) => {
                skipped.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        for entity in graph
            .entities
            .iter()
            .filter(|entity| entity.function == ENT_F_CAM_SENSOR)
        {
            if graph.driver != CAMSS {
                skipped.push(format!(
                    "{}: braun cannot route {} graphs",
                    entity.name, graph.driver
                ));
                continue;
            }
            match probe(&graph, &path, entity) {
                Ok(camera) => cameras.push(camera),
                Err(error) => skipped.push(format!("{}: {error}", entity.name)),
            }
        }
    }
    // Two of the same sensor are told apart by their address.
    let models: Vec<String> = cameras.iter().map(|camera| camera.model.clone()).collect();
    for camera in &mut cameras {
        if models
            .iter()
            .filter(|model| **model == camera.model)
            .count()
            > 1
        {
            let address = camera.entity.rsplit('-').next().unwrap_or_default();
            camera.id = format!("{}-{address}", camera.model);
        }
        match profile::calibration(&camera.id) {
            Ok(Some(calibration)) => calibrate(camera, &calibration),
            Ok(None) => camera.notes.push("no calibration: untuned".into()),
            Err(error) => camera.notes.push(format!("{error}: untuned")),
        }
    }
    Discovery { cameras, skipped }
}

fn error(what: &'static str) -> impl Fn(std::io::Error) -> String {
    move |error| format!("{what}: {error}")
}

/// The kernel's facts about the sensor `entity` and its lens, with its
/// sensor profile.
fn probe(graph: &Graph, media: &Path, entity: &Entity) -> Result<Camera, String> {
    let node = entity.node.as_deref().ok_or("no device node")?;
    let sensor = v4l2::open(node).map_err(error("opening"))?;
    let format = v4l2::subdev_format(&sensor, 0).map_err(error("format"))?;
    let bayer = Bayer::from_code(format.code)
        .ok_or_else(|| format!("format 0x{:04x} is not 10-bit Bayer", format.code))?;
    let exposure = v4l2::range(&sensor, cid::EXPOSURE).map_err(error("exposure"))?;
    let hblank = v4l2::value(&sensor, cid::HBLANK).map_err(error("horizontal blanking"))?;
    let pixel_rate = v4l2::value(&sensor, cid::PIXEL_RATE).map_err(error("pixel rate"))?;
    if pixel_rate <= 0 {
        return Err("no pixel rate".into());
    }
    let mut notes = Vec::new();
    let facing = match v4l2::value(&sensor, cid::CAMERA_ORIENTATION) {
        Ok(0) => Facing::Front,
        Ok(2) => Facing::External,
        Ok(_) => Facing::Back,
        Err(_) => {
            notes.push("no orientation: back assumed".into());
            Facing::Back
        }
    };
    let rotation = v4l2::value(&sensor, cid::CAMERA_SENSOR_ROTATION).unwrap_or_else(|_| {
        notes.push("no rotation: none assumed".into());
        0
    });

    let model = entity
        .name
        .split(' ')
        .next()
        .unwrap_or(&entity.name)
        .to_string();
    let sensor_profile = match profile::sensor(&model) {
        Ok(Some(found)) => found,
        Ok(None) => {
            notes.push("no sensor profile".into());
            Default::default()
        }
        Err(error) => {
            notes.push(format!("{error}: ignored"));
            Default::default()
        }
    };
    let gain_range = v4l2::range(&sensor, cid::ANALOGUE_GAIN).ok();
    let gain_codes = gain_range.map_or(0..=0, |range| {
        range.min.max(0) as u32..=range.max.max(0) as u32
    });
    let gain = sensor_profile.analogue_gain.or_else(|| {
        // The usual linear code, with its least as 1x.
        let unit = *gain_codes.start();
        (gain_range.is_some() && unit > 0).then(|| {
            notes.push(format!("gain assumed code / {unit}"));
            GainModel {
                m0: 1.0,
                c0: 0.0,
                m1: 0.0,
                c1: unit as f32,
            }
        })
    });
    if gain.is_none() {
        notes.push("gain unknown: left as it is".into());
    }
    let max_gain = match gain {
        Some(model) => sensor_profile
            .max_gain
            .unwrap_or(MAX_ANALOGUE)
            .min(model.gain(*gain_codes.end()))
            .max(1.0),
        None => 1.0,
    };

    let lens = graph
        .ancillaries_of(entity)
        .into_iter()
        .find(|ancillary| ancillary.function == ENT_F_LENS)
        .and_then(|lens| {
            let file = graph.node(&lens.name).ok()?;
            let range = v4l2::range(&file, cid::FOCUS_ABSOLUTE).ok()?;
            let (min, max) = (range.min.max(0) as u32, range.max.max(0) as u32);
            Some(Lens {
                entity: lens.name.clone(),
                travel: min..=max,
                start: min + (max - min) / 2,
            })
        });
    let phy = graph
        .sinks_of(entity)
        .first()
        .map(|phy| phy.name.clone())
        .ok_or("not linked to a receiver")?;

    Ok(Camera {
        id: model.clone(),
        name: model.to_uppercase(),
        model,
        entity: entity.name.clone(),
        media: media.to_path_buf(),
        phy,
        facing,
        role: None,
        zoom: None,
        width: format.width,
        height: format.height,
        bayer,
        black: sensor_profile.black.unwrap_or(BLACK),
        clip: sensor_profile.white.unwrap_or(WHITE),
        line_us: (format.width as f32 + hblank as f32) / pixel_rate as f32 * 1e6,
        min_lines: exposure.min.max(1) as u32,
        max_lines: exposure.max.max(1) as u32,
        line_step: exposure.step.max(1) as u32,
        gain,
        gain_codes,
        max_gain,
        rotation: clockwise(rotation as u32),
        mirror: facing == Facing::Front,
        lens,
        colour: Colour::untuned(),
        tuned: false,
        notes,
    })
}

/// The kernel's rotation is the counter-clockwise correction.
fn clockwise(correction: u32) -> u32 {
    (360 - correction % 360) % 360
}

fn calibrate(camera: &mut Camera, calibration: &Calibration) {
    camera.role = calibration.role;
    camera.zoom = calibration
        .zoom
        .or((calibration.role == Some(Role::Main)).then_some(1.0));
    if let Some(rotation) = calibration.rotation {
        camera.rotation = clockwise(rotation);
    }
    if let (Some(lens), Some(focus)) = (&mut camera.lens, &calibration.focus) {
        let (min, max) = (*lens.travel.start(), *lens.travel.end());
        let [from, to] = focus.travel.map(|position| position.clamp(min, max));
        if from < to {
            lens.travel = from..=to;
            lens.start = focus
                .start
                .unwrap_or(from + (to - from) / 2)
                .clamp(from, to);
        } else {
            camera
                .notes
                .push("focus travel empty: the whole travel used".into());
        }
    }
    if calibration.has_colour() {
        match calibration.colour() {
            Ok(colour) => {
                camera.colour = colour;
                camera.tuned = true;
            }
            Err(error) => camera
                .notes
                .push(format!("calibration colour: {error}: untuned")),
        }
    } else {
        camera
            .notes
            .push("calibration has no colour: untuned".into());
    }
}

/// Where pixel (x, y) of a `width` x `height` image lands once turned
/// `rotation` degrees clockwise and mirrored if `mirror`, and the turned
/// image's size.
pub fn place(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    rotation: u32,
    mirror: bool,
) -> (usize, usize) {
    let (out_width, _) = turned(width, height, rotation);
    let (ox, oy) = match rotation {
        90 => (height - 1 - y, x),
        180 => (width - 1 - x, height - 1 - y),
        270 => (y, width - 1 - x),
        _ => (x, y),
    };
    if mirror {
        (out_width - 1 - ox, oy)
    } else {
        (ox, oy)
    }
}

pub fn turned(width: usize, height: usize, rotation: u32) -> (usize, usize) {
    if rotation % 180 == 90 {
        (height, width)
    } else {
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::{GainModel, clockwise};

    #[test]
    fn gain_models() {
        let sony = GainModel {
            m0: 0.0,
            c0: 16384.0,
            m1: -1.0,
            c1: 16384.0,
        };
        assert_eq!(sony.code(1.0), 0);
        assert_eq!(sony.code(16.0), 15360);
        assert!((sony.gain(15360) - 16.0).abs() < 1e-3);
        let linear = GainModel {
            m0: 1.0,
            c0: 0.0,
            m1: 0.0,
            c1: 1024.0,
        };
        assert_eq!(linear.code(16.0), 0x4000);
        assert!((linear.gain(0x0400) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rotations() {
        assert_eq!(clockwise(270), 90);
        assert_eq!(clockwise(90), 270);
        assert_eq!(clockwise(0), 0);
        assert_eq!(clockwise(180), 180);
    }
}
