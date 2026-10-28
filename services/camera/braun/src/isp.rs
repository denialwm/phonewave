//! The live part of the pipeline, on every frame: a small upright preview
//! straight from the raw samples, and the statistics auto exposure, white
//! balance and focus run on. Methods as in Phonewave's libcamera patches for
//! the main camera: exposure meters the mean at 18% grey and lets at most 4%
//! of the frame near clipping; white balance is the grey of the zones near
//! the illuminant locus, projected onto it, which also gives the colour
//! temperature the matrix is chosen by. Lens shading is undone first, by
//! the stock tuning's gain maps.

use crate::camera::{Camera, Colour, SHADING_COLUMNS, SHADING_NODES, SHADING_ROWS, place, turned};

/// Linear luminance auto exposure aims the mean at.
const TARGET_MEAN: f32 = 0.18;
/// At most this share of the frame may sit over `HIGHLIGHT`.
const HIGHLIGHT_SHARE: f32 = 0.04;
const HIGHLIGHT: f32 = 0.9;
/// Exposure changes by the square root of what the meter asks, at most
/// this factor a step, and not at all within the dead band.
const AE_STEP: f32 = 1.6;
const AE_DEAD_BAND: f32 = 1.08;
/// White balance zones.
const ZONES_X: usize = 16;
const ZONES_Y: usize = 12;
/// A zone is grey when its colour is this close to the locus.
const LOCUS_BAND: f32 = 0.06;
/// A zone this dark (0 to 1, after black) says nothing about colour.
const ZONE_FLOOR: f32 = 4.0 / 1023.0;
/// A light cooler than this is shown neutral, as the RedMagic shows a cool
/// monitor scene; a warmer one is moved this much of the way toward it,
/// and keeps some warmth (a first guess).
const SHOWN_CT: f32 = 5500.0;
const KEEP_WARM: f32 = 0.4;
/// White balance follows its estimate this much each frame, in log space.
const AWB_SPEED: f32 = 0.25;
/// Raw codes below clipping over which a cell goes to white.
const CLIP_RAMP: f32 = 120.0;
/// A clipping cell is a light source, and goes to white, as its colour
/// nears grey: its weakest channel over its strongest, from the first to
/// the second. A saturated colour that clips (a lit blue wall) keeps it.
const NEUTRAL_FROM: f32 = 0.2;
const NEUTRAL_TO: f32 = 0.4;
/// Lens shading is undone fully, as the RedMagic does in a dim room, until
/// the scene is too dark for the sensor and digital gain takes over; then
/// less, to `SHADING_DIM` of its strength (as a power of the gains) at the
/// most digital gain, so the corners do not show the noise the gains lift:
/// the stock tuning dims it in low light to 0.6 and then 0.2.
const SHADING_DIM: f32 = 0.4;
/// The preview is at most about this wide, in source cells.
const PREVIEW_CELLS: usize = 1024;
/// The most digital gain auto exposure adds once time and analogue gain
/// are spent: a dark scene is brightened as the RedMagic brightens it.
const MAX_DIGITAL: f32 = 8.0;

/// How a frame is exposed: microseconds times analogue gain on the sensor,
/// and digital gain after it.
#[derive(Clone, Copy, Debug)]
pub struct Exposure {
    pub lines: u32,
    pub gain: f32,
    pub digital: f32,
}

impl Exposure {
    /// Microseconds x analogue gain x digital gain.
    pub fn total(&self, camera: &Camera) -> f32 {
        self.on_sensor(camera) * self.digital
    }

    /// Microseconds x analogue gain: what the sensor collects.
    pub fn on_sensor(&self, camera: &Camera) -> f32 {
        self.lines as f32 * camera.line_us * self.gain
    }

    /// The longest time first, then analogue gain, then digital gain, for
    /// `total` microseconds x gain.
    pub fn of(total: f32, camera: &Camera) -> Self {
        let longest = camera.max_lines as f32 * camera.line_us;
        let time = total.clamp(camera.min_lines as f32 * camera.line_us, longest);
        let step = camera.line_step;
        let lines = ((time / camera.line_us) as u32 / step * step)
            .clamp(camera.min_lines, camera.max_lines);
        let gain = (total / (lines as f32 * camera.line_us)).clamp(1.0, camera.max_gain);
        // The gain the sensor will really apply.
        let gain = camera.applied_gain(gain);
        let digital = (total / (lines as f32 * camera.line_us * gain)).clamp(1.0, MAX_DIGITAL);
        Self {
            lines,
            gain,
            digital,
        }
    }
}

/// Digital gain `digital` on scene light `light` (1 at clipping): shadows
/// and mid tones rise by it, and the light the sensor clipped stays where
/// it was, so lights keep their colour.
pub fn lift(light: f32, digital: f32) -> f32 {
    light * digital / (1.0 + (digital - 1.0) * light.max(0.0))
}

/// White balance and colour: gains on raw R and B (G is 1) and the matrix
/// after them, for a light of `light` kelvin shown as `ct` kelvin.
#[derive(Clone, Copy, Debug)]
pub struct Rendering {
    pub wb: [f32; 3],
    pub ccm: [f32; 9],
    /// The light white balance measured.
    pub light: f32,
    /// The light the gains and matrix are for: `light`, or for a warm
    /// light, `light` moved toward `SHOWN_CT`, so it stays a little warm.
    pub ct: f32,
    /// Lens shading gains for `light` at the exposure's digital gain, red,
    /// green and blue, on the grid of `Shading`.
    pub shading: [[f32; 3]; SHADING_NODES],
}

impl Rendering {
    pub fn neutral(colour: &Colour) -> Self {
        let ct = SHOWN_CT;
        Self {
            wb: [2.0, 1.0, 1.6],
            ccm: look(ccm_at(colour, ct)),
            light: ct,
            ct,
            shading: shading_at(colour, ct, 1.0),
        }
    }
}

/// The lens shading gains of `colour` under a light of `light` kelvin at
/// digital gain `digital`: the tuned maps interpolated in mired, dimmed as
/// digital gain rises.
fn shading_at(colour: &Colour, light: f32, digital: f32) -> [[f32; 3]; SHADING_NODES] {
    let maps = &colour.shading;
    if maps.is_empty() {
        return [[1.0; 3]; SHADING_NODES];
    }
    let upper = maps
        .iter()
        .position(|map| map.ct >= light)
        .unwrap_or(maps.len() - 1)
        .max(1)
        .min(maps.len() - 1);
    let lower = upper.saturating_sub(1);
    let (a, b) = (&maps[lower], &maps[upper]);
    let t = if lower == upper {
        0.0
    } else {
        ((1e6 / light - 1e6 / a.ct) / (1e6 / b.ct - 1e6 / a.ct)).clamp(0.0, 1.0)
    };
    let dim = (digital.ln() / MAX_DIGITAL.ln()).clamp(0.0, 1.0);
    let strength = 1.0 - (1.0 - SHADING_DIM) * dim;
    std::array::from_fn(|node| {
        std::array::from_fn(|channel| {
            let gain =
                a.gains[channel][node] + (b.gains[channel][node] - a.gains[channel][node]) * t;
            gain.powf(strength)
        })
    })
}

/// Lens shading gains at any cell of a frame `cells_x` cells wide, by
/// bilinear interpolation of the grid: one row at a time.
pub struct Shader {
    /// Per cell across: the grid column on its left, and how far past it.
    columns: Vec<(usize, f32)>,
    row: [[f32; 3]; SHADING_COLUMNS],
}

impl Shader {
    pub fn new(cells_x: usize) -> Self {
        let columns = (0..cells_x)
            .map(|x| {
                let u = (x as f32 + 0.5) / cells_x as f32 * (SHADING_COLUMNS - 1) as f32;
                let left = (u as usize).min(SHADING_COLUMNS - 2);
                (left, u - left as f32)
            })
            .collect();
        Self {
            columns,
            row: [[1.0; 3]; SHADING_COLUMNS],
        }
    }

    /// Moves to cell row `y` of `cells_y`.
    pub fn row(&mut self, grid: &[[f32; 3]; SHADING_NODES], y: usize, cells_y: usize) {
        let v = (y as f32 + 0.5) / cells_y as f32 * (SHADING_ROWS - 1) as f32;
        let top = (v as usize).min(SHADING_ROWS - 2);
        let t = v - top as f32;
        for column in 0..SHADING_COLUMNS {
            let (a, b) = (
                grid[top * SHADING_COLUMNS + column],
                grid[(top + 1) * SHADING_COLUMNS + column],
            );
            self.row[column] =
                std::array::from_fn(|channel| a[channel] + (b[channel] - a[channel]) * t);
        }
    }

    /// The gains at cell `x` of the current row.
    pub fn at(&self, x: usize) -> [f32; 3] {
        let (left, t) = self.columns[x];
        let (a, b) = (self.row[left], self.row[left + 1]);
        std::array::from_fn(|channel| a[channel] + (b[channel] - a[channel]) * t)
    }
}

/// What one frame says about the scene.
pub struct Stats {
    /// Mean linear luminance, white balanced, after digital gain, 0 to 1.
    pub mean: f32,
    /// The 96th percentile of each cell's brightest channel, 0 to 1.
    pub high: f32,
    /// Colour sums per zone over unclipped cells, and the cells.
    zones: Vec<[f64; 3]>,
    counts: Vec<u32>,
}

/// An upright preview, BGRA.
pub struct Preview {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

/// Scene light to display, by a curve learned from the RedMagic 11 Pro+
/// on a normally exposed scene (a video on a monitor, 2026-10-22): each
/// channel goes through it, as a camera's gamma does, so a saturated
/// colour stays saturated and a bright one turns toward white as its
/// strongest channel tops out. A filmic S: the shadows go deep, the mid
/// tones keep their contrast and the highlights roll off; light past the
/// sensor's clipping (from a photo's short exposures, or a light source)
/// reaches white. Then sRGB encoding.
pub struct Tone {
    /// sRGB code by the square root of scene light over `TONE_RANGE`.
    lut: Vec<u8>,
}

/// The curve through (scene light, display light): scene light 1 is the
/// sensor's clipping at the preview's exposure. Monotone cubic in log-log
/// between the points, a straight gain below the first.
const TONE_POINTS: [(f32, f32); 14] = [
    (0.003, 0.0008),
    (0.01, 0.0014),
    (0.02, 0.003),
    (0.04, 0.0095),
    (0.07, 0.029),
    (0.1, 0.057),
    (0.15, 0.10),
    (0.22, 0.16),
    (0.35, 0.25),
    (0.55, 0.38),
    (0.8, 0.58),
    (1.1, 0.75),
    (1.6, 0.88),
    (TONE_RANGE, 1.0),
];
/// Scene light shown as white.
pub const TONE_RANGE: f32 = 4.0;
const TONE_STEPS: usize = 4096;

impl Default for Tone {
    fn default() -> Self {
        Self::new()
    }
}

impl Tone {
    pub fn new() -> Self {
        let last = (TONE_STEPS - 1) as f32;
        let lut = (0..TONE_STEPS)
            .map(|step| {
                (srgb(tone_curve((step as f32 / last).powi(2) * TONE_RANGE)) * 255.0 + 0.5) as u8
            })
            .collect();
        Self { lut }
    }

    /// The display colour of scene-linear `rgb`, sRGB encoded.
    pub fn display(&self, rgb: [f32; 3]) -> [u8; 3] {
        let last = (TONE_STEPS - 1) as f32;
        rgb.map(|value| {
            self.lut[((value / TONE_RANGE).clamp(0.0, 1.0).sqrt() * last + 0.5) as usize]
        })
    }
}

/// `TONE_POINTS` as a monotone cubic (Fritsch-Carlson) in log-log.
fn tone_curve(light: f32) -> f32 {
    let (first_light, first_shown) = TONE_POINTS[0];
    if light <= first_light {
        return light * first_shown / first_light;
    }
    let points = TONE_POINTS.map(|(light, shown)| (light.ln(), shown.ln()));
    let slopes: Vec<f32> = points
        .windows(2)
        .map(|pair| (pair[1].1 - pair[0].1) / (pair[1].0 - pair[0].0))
        .collect();
    let mut tangents = vec![0f32; points.len()];
    tangents[0] = slopes[0];
    tangents[points.len() - 1] = slopes[slopes.len() - 1];
    for index in 1..points.len() - 1 {
        let (before, after) = (slopes[index - 1], slopes[index]);
        tangents[index] = if before * after <= 0.0 {
            0.0
        } else {
            2.0 / (1.0 / before + 1.0 / after)
        };
    }
    let x = light.ln().min(points[points.len() - 1].0);
    let index = points
        .windows(2)
        .position(|pair| x <= pair[1].0)
        .unwrap_or(points.len() - 2);
    let ((x0, y0), (x1, y1)) = (points[index], points[index + 1]);
    let h = x1 - x0;
    let t = (x - x0) / h;
    let (t2, t3) = (t * t, t * t * t);
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
        + (t3 - 2.0 * t2 + t) * h * tangents[index]
        + (-2.0 * t3 + 3.0 * t2) * y1
        + (t3 - t2) * h * tangents[index + 1];
    y.exp()
}

/// The colour look, learned with the curve: hue turned 4 degrees toward
/// blue about the grey axis, after the sensor's matrix, saturation as it
/// is. Linear, so it folds into the matrix.
const LOOK_SATURATION: f32 = 1.0;
const LOOK_HUE_DEGREES: f32 = -4.0;

pub fn look(ccm: [f32; 9]) -> [f32; 9] {
    let luma = [0.2126f32, 0.7152, 0.0722];
    let (sin, cos) = LOOK_HUE_DEGREES.to_radians().sin_cos();
    let k = 1.0 / 3f32.sqrt();
    // Rodrigues: rotation about (1, 1, 1).
    let rotation = [
        cos + k * k * (1.0 - cos),
        k * k * (1.0 - cos) - k * sin,
        k * k * (1.0 - cos) + k * sin,
        k * k * (1.0 - cos) + k * sin,
        cos + k * k * (1.0 - cos),
        k * k * (1.0 - cos) - k * sin,
        k * k * (1.0 - cos) - k * sin,
        k * k * (1.0 - cos) + k * sin,
        cos + k * k * (1.0 - cos),
    ];
    // look = grey + saturation x rotation x (colour - grey), grey = luma . colour.
    let mut matrix = [0f32; 9];
    for row in 0..3 {
        for column in 0..3 {
            let chroma: f32 = (0..3)
                .map(|inner| {
                    rotation[row * 3 + inner]
                        * (if inner == column { 1.0 } else { 0.0 } - luma[column])
                })
                .sum();
            matrix[row * 3 + column] = luma[column] + LOOK_SATURATION * chroma;
        }
    }
    let mut out = [0f32; 9];
    for row in 0..3 {
        for column in 0..3 {
            out[row * 3 + column] = (0..3)
                .map(|inner| matrix[row * 3 + inner] * ccm[inner * 3 + column])
                .sum();
        }
    }
    out
}

pub fn srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// The 10-bit sample at `x` of a packed RAW10 row.
fn sample(row: &[u8], x: usize) -> f32 {
    let group = x / 4 * 5;
    let low = (row[group + 4] >> (2 * (x % 4))) & 3;
    ((row[group + x % 4] as u32) << 2 | low as u32) as f32
}

/// Renders frame `data` (RAW10, `stride` bytes a row) of `sensor` to an
/// upright preview with `rendering` and `digital` gain, and gathers its
/// statistics. `zoom` crops the middle of the frame: the preview and the
/// statistics see only what the photo will.
#[allow(clippy::too_many_arguments)]
pub fn preview(
    data: &[u8],
    stride: usize,
    camera: &Camera,
    rendering: &Rendering,
    digital: f32,
    zoom: f32,
    tone: &Tone,
) -> (Preview, Stats) {
    let cells_x = camera.width as usize / 2;
    let cells_y = camera.height as usize / 2;
    let (seen_x, seen_y) = cropped(cells_x, cells_y, zoom);
    let (first_x, first_y) = ((cells_x - seen_x) / 2, (cells_y - seen_y) / 2);
    let step = seen_x.div_ceil(PREVIEW_CELLS).max(1);
    let width = seen_x / step;
    let height = seen_y / step;
    let (out_width, out_height) = turned(width, height, camera.rotation);
    let mut bgra = vec![255u8; out_width * out_height * 4];
    let cell = camera.bayer.cell();
    let black = camera.black;
    let scale = 1.0 / (1023.0 - black);
    let clip = camera.clip;
    let knee = clip - CLIP_RAMP;
    let [wr, wg, wb] = rendering.wb;
    let m = rendering.ccm;
    let mut zones = vec![[0f64; 3]; ZONES_X * ZONES_Y];
    let mut counts = vec![0u32; ZONES_X * ZONES_Y];
    let mut histogram = [0u32; 256];
    let mut luminance = 0f64;
    let mut shader = Shader::new(cells_x);
    for y in 0..height {
        let cell_y = first_y + y * step;
        shader.row(&rendering.shading, cell_y, cells_y);
        let top = cell_y * 2;
        let rows = [
            &data[top * stride..][..stride],
            &data[(top + 1) * stride..][..stride],
        ];
        let zone_row = y * ZONES_Y / height * ZONES_X;
        let mut row_zones = [[0f32; 3]; ZONES_X];
        let mut row_counts = [0u32; ZONES_X];
        for x in 0..width {
            let cell_x = first_x + x * step;
            let left = cell_x * 2;
            let sample = |(dx, dy): (usize, usize)| sample(rows[dy], left + dx);
            let raw = [
                sample(cell[0]),
                sample(cell[1]),
                sample(cell[2]),
                sample(cell[3]),
            ];
            let brightest = raw.iter().fold(0f32, |a, &b| a.max(b));
            let [sr, sg, sb] = shader.at(cell_x);
            let r = (raw[0] - black).max(0.0) * scale * sr;
            let g = ((raw[1] + raw[2]) * 0.5 - black).max(0.0) * scale * sg;
            let b = (raw[3] - black).max(0.0) * scale * sb;
            if brightest < clip {
                let zone = x * ZONES_X / width;
                row_zones[zone][0] += r;
                row_zones[zone][1] += g;
                row_zones[zone][2] += b;
                row_counts[zone] += 1;
            }
            let mut rgb = [r * wr, g * wg, b * wb].map(|value| lift(value, digital));
            luminance += (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]) as f64;
            let clipping = ((brightest - knee) / CLIP_RAMP).clamp(0.0, 1.0);
            if clipping > 0.0 {
                let clipped = [
                    raw[0] >= knee,
                    raw[1] >= knee || raw[2] >= knee,
                    raw[3] >= knee,
                ];
                rgb = clipped_light(rgb, clipped, clipping);
            }
            let [r, g, b] = rgb;
            histogram[((brightest - black).max(0.0) * scale * 255.0).min(255.0) as usize] += 1;
            let (ox, oy) = place(x, y, width, height, camera.rotation, camera.mirror);
            let at = (oy * out_width + ox) * 4;
            let [red, green, blue] = tone.display([
                m[0] * r + m[1] * g + m[2] * b,
                m[3] * r + m[4] * g + m[5] * b,
                m[6] * r + m[7] * g + m[8] * b,
            ]);
            bgra[at] = blue;
            bgra[at + 1] = green;
            bgra[at + 2] = red;
        }
        for zone in 0..ZONES_X {
            for channel in 0..3 {
                zones[zone_row + zone][channel] += row_zones[zone][channel] as f64;
            }
            counts[zone_row + zone] += row_counts[zone];
        }
    }
    let cells = (width * height) as u32;
    let mut seen = 0;
    let mut high = 1.0;
    for (bin, count) in histogram.iter().enumerate() {
        seen += count;
        if seen as f32 >= cells as f32 * (1.0 - HIGHLIGHT_SHARE) {
            high = (bin as f32 + 1.0) / 256.0;
            break;
        }
    }
    let stats = Stats {
        mean: (luminance / cells as f64) as f32,
        high,
        zones,
        counts,
    };
    (
        Preview {
            width: out_width,
            height: out_height,
            bgra,
        },
        stats,
    )
}

/// The most bytes a preview of `camera` takes, at any zoom: it is at most
/// `PREVIEW_CELLS` wide, in the frame's aspect.
pub fn largest_preview(camera: &Camera) -> usize {
    let cells_x = camera.width as usize / 2;
    let cells_y = camera.height as usize / 2;
    let width = cells_x.min(PREVIEW_CELLS);
    let height = width * cells_y / cells_x.max(1) + 2;
    width * height * 4
}

/// The middle `1 / zoom` of `width` x `height`, in whole units.
pub fn cropped(width: usize, height: usize, zoom: f32) -> (usize, usize) {
    let zoom = zoom.max(1.0);
    (
        (width as f32 / zoom) as usize,
        (height as f32 / zoom) as usize,
    )
}

/// A cell some of whose channels (`clipped`) are near clipping, `clipping`
/// of the way: such a channel saw at least as much light as the channels
/// that held, and the light past it is shown. A near grey cell is a light
/// source and goes toward white; a saturated colour keeps its colour.
fn clipped_light(mut rgb: [f32; 3], clipped: [bool; 3], clipping: f32) -> [f32; 3] {
    let held = (0..3)
        .filter(|&channel| !clipped[channel])
        .fold(0f32, |a, channel| a.max(rgb[channel]));
    for channel in 0..3 {
        if clipped[channel] {
            rgb[channel] = rgb[channel].max(held);
        }
    }
    let strongest = rgb[0].max(rgb[1]).max(rgb[2]);
    let weakest = rgb[0].min(rgb[1]).min(rgb[2]);
    let grey = ((weakest / strongest.max(1e-6) - NEUTRAL_FROM) / (NEUTRAL_TO - NEUTRAL_FROM))
        .clamp(0.0, 1.0);
    let white = clipping * grey * grey * (3.0 - 2.0 * grey);
    rgb.map(|value| value + (TONE_RANGE - value) * white)
}

/// The exposure the meter asks for after `stats` were taken at `current`.
pub fn meter(stats: &Stats, current: Exposure, camera: &Camera) -> Option<Exposure> {
    let by_mean = TARGET_MEAN / stats.mean.max(1e-4);
    let by_highlights = HIGHLIGHT / stats.high.max(1e-4);
    let wanted = by_mean.min(by_highlights);
    if wanted < AE_DEAD_BAND && wanted > 1.0 / AE_DEAD_BAND {
        return None;
    }
    let factor = wanted.sqrt().clamp(1.0 / AE_STEP, AE_STEP);
    let next = Exposure::of(current.total(camera) * factor, camera);
    let changed = next.lines != current.lines
        || (next.gain - current.gain).abs() > 0.01
        || (next.digital - current.digital).abs() > 0.01;
    changed.then_some(next)
}

/// Moves `rendering` toward the white balance `stats` show.
/// Also sets the lens shading for that light at digital gain `digital`.
pub fn balance(stats: &Stats, rendering: &mut Rendering, colour: &Colour, digital: f32) {
    rendering.shading = shading_at(colour, rendering.light, digital);
    let mut sum = [0f64; 3];
    for (zone, &count) in stats.zones.iter().zip(&stats.counts) {
        if count == 0 {
            continue;
        }
        let g = zone[1] / count as f64;
        if (g as f32) < ZONE_FLOOR {
            continue;
        }
        let (mut rg, mut bg) = ((zone[0] / zone[1]) as f32, (zone[2] / zone[1]) as f32);
        if colour.locus.is_empty() || project(colour, &mut rg, &mut bg).1 <= LOCUS_BAND {
            for channel in 0..3 {
                sum[channel] += zone[channel];
            }
        }
    }
    if sum[1] <= 0.0 {
        // No grey zone: the whole frame's grey world.
        for zone in &stats.zones {
            for channel in 0..3 {
                sum[channel] += zone[channel];
            }
        }
        if sum[1] <= 0.0 {
            return;
        }
    }
    let (mut rg, mut bg) = ((sum[0] / sum[1]) as f32, (sum[2] / sum[1]) as f32);
    if colour.locus.is_empty() {
        // No tuning: grey world, shown as measured.
        let target = [1.0 / rg.max(0.05), 1.0, 1.0 / bg.max(0.05)];
        for channel in [0, 2] {
            let now = rendering.wb[channel].ln();
            rendering.wb[channel] = (now + (target[channel].ln() - now) * AWB_SPEED).exp();
        }
        return;
    }
    let light = project(colour, &mut rg, &mut bg).0;
    let mired = 1e6 / rendering.light;
    rendering.light = 1e6 / (mired + (1e6 / light - mired) * AWB_SPEED);
    rendering.ct = shown_ct(rendering.light);
    let (rg, bg) = locus_at(colour, rendering.ct);
    rendering.wb = [1.0 / rg, 1.0, 1.0 / bg];
    rendering.ccm = look(ccm_at(colour, rendering.ct));
}

/// The light a scene lit by `light` kelvin is shown as: itself when it is
/// cooler than `SHOWN_CT`, else moved `KEEP_WARM` of the way toward it, in
/// mired.
fn shown_ct(light: f32) -> f32 {
    if light >= SHOWN_CT {
        return light;
    }
    let (from, to) = (1e6 / light, 1e6 / SHOWN_CT);
    1e6 / (from + (to - from) * KEEP_WARM)
}

/// The grey of the locus at `ct` kelvin, interpolated in mired.
fn locus_at(colour: &Colour, ct: f32) -> (f32, f32) {
    let locus = &colour.locus;
    let mired = 1e6 / ct;
    let upper = locus
        .iter()
        .position(|point| point.ct >= ct)
        .unwrap_or(locus.len() - 1)
        .max(1);
    let (a, b) = (&locus[upper - 1], &locus[upper]);
    let t = ((mired - 1e6 / a.ct) / (1e6 / b.ct - 1e6 / a.ct)).clamp(0.0, 1.0);
    (a.rg + (b.rg - a.rg) * t, a.bg + (b.bg - a.bg) * t)
}

/// Moves (`rg`, `bg`) onto the nearest point of the locus; returns that
/// point's colour temperature, interpolated in mired, and how far it moved.
fn project(colour: &Colour, rg: &mut f32, bg: &mut f32) -> (f32, f32) {
    let mut best = (f32::MAX, 0.0, 0.0, 0.0);
    for pair in colour.locus.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (dx, dy) = (b.rg - a.rg, b.bg - a.bg);
        let t = (((*rg - a.rg) * dx + (*bg - a.bg) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (px, py) = (a.rg + t * dx, a.bg + t * dy);
        let distance = ((*rg - px).powi(2) + (*bg - py).powi(2)).sqrt();
        if distance < best.0 {
            let mired = 1e6 / a.ct + t * (1e6 / b.ct - 1e6 / a.ct);
            best = (distance, px, py, 1e6 / mired);
        }
    }
    *rg = best.1;
    *bg = best.2;
    (best.3, best.0)
}

/// The colour matrix for a light of `ct` kelvin, interpolated between the
/// tuned ones.
pub fn ccm_at(colour: &Colour, ct: f32) -> [f32; 9] {
    let ccms = &colour.ccms;
    let Some(upper) = ccms.iter().position(|ccm| ccm.ct >= ct) else {
        return ccms[ccms.len() - 1].m;
    };
    if upper == 0 {
        return ccms[0].m;
    }
    let (a, b) = (&ccms[upper - 1], &ccms[upper]);
    let t = (ct - a.ct) / (b.ct - a.ct);
    std::array::from_fn(|index| a.m[index] + (b.m[index] - a.m[index]) * t)
}

/// How sharp `preview` is around (`x`, `y`), 0 to 1 across it: the green's
/// gradient over its mean, so exposure changes do not move it.
pub fn sharpness(preview: &Preview, x: f32, y: f32) -> f32 {
    let half = preview.width.min(preview.height) / 8;
    let cx = (x * preview.width as f32) as usize;
    let cy = (y * preview.height as f32) as usize;
    let left = cx.saturating_sub(half).min(preview.width - 2 * half - 1);
    let top = cy.saturating_sub(half).min(preview.height - 2 * half - 1);
    let green = |x: usize, y: usize| preview.bgra[(y * preview.width + x) * 4 + 1] as f32;
    let (mut gradient, mut sum) = (0f32, 0f32);
    for y in top..top + 2 * half {
        for x in left..left + 2 * half {
            let here = green(x, y);
            gradient += (green(x + 1, y) - here).powi(2) + (green(x, y + 1) - here).powi(2);
            sum += here;
        }
    }
    let mean = sum / (4 * half * half) as f32;
    gradient / (mean * mean).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::{TONE_POINTS, TONE_RANGE, Tone, clipped_light, lift, tone_curve};

    #[test]
    fn curve_goes_through_its_points() {
        for (light, shown) in TONE_POINTS {
            assert!(
                (tone_curve(light) / shown - 1.0).abs() < 1e-3,
                "{light} {}",
                tone_curve(light)
            );
        }
        let mut last = 0.0;
        for step in 1..4000 {
            let value = tone_curve(step as f32 / 1000.0);
            assert!(value >= last);
            last = value;
        }
    }

    #[test]
    fn black_stays_and_light_past_clipping_is_near_white() {
        let tone = Tone::new();
        assert_eq!(tone.display([0.0; 3]), [0; 3]);
        assert!(
            tone.display([TONE_RANGE; 3])
                .iter()
                .all(|&value| value >= 240)
        );
        // The brightest light the sensor holds is not white.
        let [red, ..] = tone.display([0.6; 3]);
        assert!(red < 220, "{red}");
        // A bright saturated blue keeps most of its colour.
        let [red, green, blue] = tone.display([0.2, 0.3, 3.0]);
        assert!(
            blue > 240 && red < 200 && green < 200,
            "{red} {green} {blue}"
        );
    }

    #[test]
    fn clipped_grey_goes_white_and_clipped_colour_stays() {
        // A light source whose green clipped first: no pink, white.
        let light = clipped_light([0.9, 1.0, 0.95], [false, true, false], 1.0);
        assert!(
            light.iter().all(|&value| (value - TONE_RANGE).abs() < 1e-3),
            "{light:?}"
        );
        // A lit blue wall whose blue clipped stays as it was.
        let wall = clipped_light([0.1, 0.4, 1.57], [false, false, true], 1.0);
        assert_eq!(wall, [0.1, 0.4, 1.57]);
    }

    #[test]
    fn digital_gain_lifts_shadows_and_keeps_clipping() {
        assert!((lift(0.01, 6.0) / 0.06 - 1.0).abs() < 0.06);
        assert!((lift(1.0, 6.0) - 1.0).abs() < 1e-6);
        assert_eq!(lift(0.3, 1.0), 0.3);
    }
}
