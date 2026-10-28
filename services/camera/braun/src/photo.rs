//! A photo from a bracketed raw burst at the sensor's full output: every
//! frame aligned to one reference, merged by exposure in linear radiance on
//! the raw mosaic with clipped cells left out, lens shading undone,
//! demosaiced, white balanced and colour corrected, turned upright, and shown through the preview's tone (`isp::Tone`) at the
//! preview's exposure, so the photo looks like the preview with less noise
//! and more detail.

use std::fs::File;
use std::io::Write;

use crate::camera::{Camera, place, turned};
use crate::isp::{self, Rendering, Shader, TONE_RANGE, Tone, lift};

/// Frames are aligned by up to this many cells (2x2 sensor pixels).
const MAX_SHIFT: i32 = 64;
const QUALITY: u8 = 92;

/// Frames taken at one exposure.
pub struct Group {
    /// Microseconds x gain.
    pub exposure: f32,
    pub frames: Vec<Vec<u8>>,
}

/// One channel of an image.
#[derive(Clone)]
struct Plane {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl Plane {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }
}

type Rgb = [Plane; 3];

/// The 10-bit sample at `x` of a packed RAW10 row.
fn sample(row: &[u8], x: usize) -> f32 {
    let group = x / 4 * 5;
    let low = (row[group + 4] >> (2 * (x % 4))) & 3;
    ((row[group + x % 4] as u32) << 2 | low as u32) as f32
}

/// A frame's green per cell over black, which frames are aligned on, and
/// the cells where any sample clipped.
fn green(data: &[u8], stride: usize, camera: &Camera) -> (Plane, Vec<bool>) {
    let (width, height) = (camera.width as usize / 2, camera.height as usize / 2);
    let mut plane = Plane::new(width, height);
    let mut clipped = vec![false; width * height];
    let cell = camera.bayer.cell();
    for y in 0..height {
        let rows = [
            &data[2 * y * stride..][..stride],
            &data[(2 * y + 1) * stride..][..stride],
        ];
        for x in 0..width {
            let raw = cell.map(|(dx, dy)| sample(rows[dy], 2 * x + dx));
            let at = y * width + x;
            clipped[at] = raw.iter().any(|&value| value >= camera.clip);
            plane.data[at] = (raw[1] + raw[2]) * 0.5 - camera.black;
        }
    }
    (plane, clipped)
}

/// Log radiance of the green.
fn guide(green: &Plane, exposure: f32) -> Plane {
    let mut plane = Plane::new(green.width, green.height);
    for (out, &value) in plane.data.iter_mut().zip(&green.data) {
        *out = (1.0 + value.max(0.0) / exposure * 1e4).ln();
    }
    plane
}

fn halve(plane: &Plane) -> Plane {
    let mut out = Plane::new(plane.width / 2, plane.height / 2);
    for y in 0..out.height {
        for x in 0..out.width {
            out.data[y * out.width + x] = 0.25
                * (plane.at(2 * x, 2 * y)
                    + plane.at(2 * x + 1, 2 * y)
                    + plane.at(2 * x, 2 * y + 1)
                    + plane.at(2 * x + 1, 2 * y + 1));
        }
    }
    out
}

fn pyramid(plane: Plane, levels: usize) -> Vec<Plane> {
    let mut out = vec![plane];
    for _ in 1..levels {
        let next = halve(out.last().unwrap());
        out.push(next);
    }
    out
}

/// Mean absolute difference of `a` and `b` shifted by (`dx`, `dy`), over
/// the middle of the image.
fn difference(a: &Plane, b: &Plane, dx: i32, dy: i32, margin: usize) -> f32 {
    let mut sum = 0f32;
    let step = (a.width / 256).max(1);
    let mut count = 0;
    for y in (margin..a.height - margin).step_by(step) {
        let by = (y as i32 + dy) as usize;
        for x in (margin..a.width - margin).step_by(step) {
            let bx = (x as i32 + dx) as usize;
            sum += (a.at(x, y) - b.at(bx, by)).abs();
            count += 1;
        }
    }
    sum / count.max(1) as f32
}

/// The shift in cells that lays `frame` on `reference`, coarse to fine.
fn align(reference: &[Plane], frame: &[Plane]) -> (i32, i32) {
    let levels = reference.len();
    let (mut dx, mut dy) = (0, 0);
    for level in (0..levels).rev() {
        dx *= 2;
        dy *= 2;
        let search = if level == levels - 1 {
            MAX_SHIFT >> level
        } else {
            1
        };
        let margin = (MAX_SHIFT >> level) as usize + 2;
        let (a, b) = (&reference[level], &frame[level]);
        if a.width <= 2 * margin || a.height <= 2 * margin {
            continue;
        }
        let mut best = (f32::MAX, dx, dy);
        for oy in -search..=search {
            for ox in -search..=search {
                let (cx, cy) = (dx + ox, dy + oy);
                if cx.unsigned_abs() as usize >= margin || cy.unsigned_abs() as usize >= margin {
                    continue;
                }
                let score = difference(a, b, cx, cy, margin);
                if score < best.0 {
                    best = (score, cx, cy);
                }
            }
        }
        (dx, dy) = (best.1, best.2);
    }
    (dx, dy)
}

/// `index` folded back into 0..`length` by whole cells, so a sample keeps
/// its colour.
fn fold(index: isize, length: usize) -> usize {
    let length = length as isize;
    let folded = if index < 0 {
        index + 2 * ((1 - index) / 2)
    } else if index >= length {
        index - 2 * ((index - length) / 2 + 1)
    } else {
        index
    };
    folded.clamp(0, length - 1) as usize
}

/// Malvar-He-Cutler demosaicing of a linear mosaic whose cells are laid
/// out as `cell` (red, two greens, blue).
fn demosaic(mosaic: &Plane, cell: [(usize, usize); 4]) -> Rgb {
    let (width, height) = (mosaic.width, mosaic.height);
    let mut colour_of = [[1usize; 2]; 2];
    colour_of[cell[0].1][cell[0].0] = 0;
    colour_of[cell[3].1][cell[3].0] = 2;
    // The row of the cell red is on.
    let red_row = cell[0].1;
    let mut rgb: Rgb = [
        Plane::new(width, height),
        Plane::new(width, height),
        Plane::new(width, height),
    ];
    for y in 0..height {
        let rows: [usize; 5] =
            std::array::from_fn(|tap| fold(y as isize + tap as isize - 2, height) * width);
        for x in 0..width {
            let cols: [usize; 5] =
                std::array::from_fn(|tap| fold(x as isize + tap as isize - 2, width));
            let p = |dx: usize, dy: usize| mosaic.data[rows[dy] + cols[dx]];
            let c = p(2, 2);
            let cross = p(2, 1) + p(2, 3) + p(1, 2) + p(3, 2);
            let far = p(2, 0) + p(2, 4) + p(0, 2) + p(4, 2);
            let diagonal = p(1, 1) + p(3, 1) + p(1, 3) + p(3, 3);
            let across = p(1, 2) + p(3, 2);
            let down = p(2, 1) + p(2, 3);
            let far_across = p(0, 2) + p(4, 2);
            let far_down = p(2, 0) + p(2, 4);
            let at = y * width + x;
            let (r, g, b) = match colour_of[y % 2][x % 2] {
                0 | 2 => {
                    let green = (4.0 * c + 2.0 * cross - far) / 8.0;
                    let other = (6.0 * c + 2.0 * diagonal - 1.5 * far) / 8.0;
                    if colour_of[y % 2][x % 2] == 0 {
                        (c, green, other)
                    } else {
                        (other, green, c)
                    }
                }
                _ => {
                    // Green: the colour of the row's neighbours, and the
                    // colour of the column's.
                    let by_row =
                        (5.0 * c + 4.0 * across - diagonal - far_across + 0.5 * far_down) / 8.0;
                    let by_column =
                        (5.0 * c + 4.0 * down - diagonal - far_down + 0.5 * far_across) / 8.0;
                    if y % 2 == red_row {
                        (by_row, c, by_column)
                    } else {
                        (by_column, c, by_row)
                    }
                }
            };
            rgb[0].data[at] = r;
            rgb[1].data[at] = g;
            rgb[2].data[at] = b;
        }
    }
    rgb
}

/// Develops the burst into a JPEG written to `out`. `rendering` is the white
/// balance and matrix of the preview when the shutter was pressed, `base`
/// its exposure on the sensor (microseconds x analogue gain) and `digital`
/// its digital gain.
#[allow(clippy::too_many_arguments)]
pub fn develop(
    groups: &[Group],
    camera: &Camera,
    stride: usize,
    base: f32,
    digital: f32,
    zoom: f32,
    rendering: &Rendering,
    out: File,
) -> std::io::Result<()> {
    let started = std::time::Instant::now();
    let (width, height) = (camera.width as usize, camera.height as usize);
    let (cells_x, cells_y) = (width / 2, height / 2);
    let levels = 5;
    // The reference: the first frame of the middle group, taken in the
    // middle of the burst.
    let middle = &groups[groups.len() / 2];
    let reference = pyramid(
        guide(&green(&middle.frames[0], stride, camera).0, middle.exposure),
        levels,
    );

    // Radiance on the mosaic, and the weight of each cell.
    let mut numerator = Plane::new(width, height);
    let mut weights = vec![0f32; cells_x * cells_y];
    let mut shortest_clipped = vec![false; cells_x * cells_y];
    let shortest = groups
        .iter()
        .map(|group| group.exposure)
        .fold(f32::MAX, f32::min);
    for group in groups {
        let mut sum = Plane::new(width, height);
        let mut clipped = vec![false; cells_x * cells_y];
        for frame in &group.frames {
            let (frame_green, clip) = green(frame, stride, camera);
            let (dx, dy) = align(
                &reference,
                &pyramid(guide(&frame_green, group.exposure), levels),
            );
            for y in 0..height {
                let sy = fold(y as isize + 2 * dy as isize, height);
                let row = &frame[sy * stride..][..stride];
                let out = &mut sum.data[y * width..][..width];
                for (x, value) in out.iter_mut().enumerate() {
                    *value += sample(row, fold(x as isize + 2 * dx as isize, width)) - camera.black;
                }
            }
            for y in 0..cells_y {
                let sy = (y as i32 + dy).clamp(0, cells_y as i32 - 1) as usize;
                for x in 0..cells_x {
                    let sx = (x as i32 + dx).clamp(0, cells_x as i32 - 1) as usize;
                    clipped[y * cells_x + x] |= clip[sy * cells_x + sx];
                }
            }
        }
        // Each group in the same radiance units; longer exposures count
        // more where they hold (better signal to noise), none where clipped.
        let count = group.frames.len() as f32;
        let weight = group.exposure * count;
        for y in 0..height {
            for x in 0..width {
                if clipped[y / 2 * cells_x + x / 2] {
                    continue;
                }
                let at = y * width + x;
                numerator.data[at] += sum.data[at] / (count * group.exposure) * weight;
            }
        }
        for (cell, &clip) in weights.iter_mut().zip(&clipped) {
            if !clip {
                *cell += weight;
            }
        }
        if group.exposure == shortest {
            shortest_clipped = clipped;
        }
    }
    // Clipped in every group: at least the shortest group's clip level.
    // The rest is lifted by the lens shading of its colour.
    let level = (camera.clip - camera.black) / shortest;
    let cell = camera.bayer.cell();
    let mut channel_of = [[1usize; 2]; 2];
    channel_of[cell[0].1][cell[0].0] = 0;
    channel_of[cell[3].1][cell[3].0] = 2;
    let mut shader = Shader::new(cells_x);
    for y in 0..height {
        if y % 2 == 0 {
            shader.row(&rendering.shading, y / 2, cells_y);
        }
        for x in 0..width {
            let weight = weights[y / 2 * cells_x + x / 2];
            let value = &mut numerator.data[y * width + x];
            *value = if weight > 0.0 {
                *value / weight * shader.at(x / 2)[channel_of[y % 2][x % 2]]
            } else {
                level
            };
        }
    }
    let rgb = demosaic(&numerator, camera.bayer.cell());
    drop(numerator);
    let merged = started.elapsed();

    // White balance, the preview's digital gain, the matrix, the tone the
    // preview shows, and light sources held white. Radiance is raw codes
    // per exposure; the tone takes light relative to clipping at the
    // preview's exposure `base`.
    let [wr, wg, wb] = rendering.wb;
    let m = rendering.ccm;
    let scale = base / (1023.0 - camera.black);
    let tone = Tone::new();
    let (out_width, out_height) = turned(width, height, camera.rotation);
    let mut pixels = vec![0u8; out_width * out_height * 3];
    for y in 0..height {
        for x in 0..width {
            let at = y * width + x;
            let [r, g, b] = [
                rgb[0].data[at] * wr,
                rgb[1].data[at] * wg,
                rgb[2].data[at] * wb,
            ]
            .map(|value| lift(value * scale, digital));
            let colour = if shortest_clipped[y / 2 * cells_x + x / 2] {
                [TONE_RANGE; 3]
            } else {
                [
                    m[0] * r + m[1] * g + m[2] * b,
                    m[3] * r + m[4] * g + m[5] * b,
                    m[6] * r + m[7] * g + m[8] * b,
                ]
            };
            let (ox, oy) = place(x, y, width, height, camera.rotation, camera.mirror);
            let pixel = (oy * out_width + ox) * 3;
            pixels[pixel..pixel + 3].copy_from_slice(&tone.display(colour));
        }
    }
    drop(rgb);
    let (pixels, out_width, out_height) = crop(pixels, out_width, out_height, zoom);
    let mut file = std::io::BufWriter::new(out);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut file, QUALITY)
        .encode(
            &pixels,
            out_width as u32,
            out_height as u32,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(std::io::Error::other)?;
    file.flush()?;
    eprintln!(
        "braun: {} photo ({out_width}x{out_height}) from {} frames: merged {:.1} s, done {:.1} s",
        camera.name,
        groups.iter().map(|group| group.frames.len()).sum::<usize>(),
        merged.as_secs_f32(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// The middle `1 / zoom` of an RGB image, as the preview showed it.
fn crop(image: Vec<u8>, width: usize, height: usize, zoom: f32) -> (Vec<u8>, usize, usize) {
    let (seen_x, seen_y) = isp::cropped(width, height, zoom);
    if (seen_x, seen_y) == (width, height) {
        return (image, width, height);
    }
    let (first_x, first_y) = ((width - seen_x) / 2, (height - seen_y) / 2);
    let mut out = Vec::with_capacity(seen_x * seen_y * 3);
    for y in first_y..first_y + seen_y {
        out.extend_from_slice(&image[(y * width + first_x) * 3..][..seen_x * 3]);
    }
    (out, seen_x, seen_y)
}
