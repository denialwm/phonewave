//! What runs around the two models: Pillow-exact resampling for their
//! inputs, and the lock mask built from their outputs. Ported from the
//! Python prototype (lock_mask.py) so masks stay comparable.

use anyhow::{Result, ensure};

use crate::tensor::{Tensor, par_rows};

#[derive(Clone, Copy)]
pub enum Filter {
    Bilinear,
    /// Pillow's bicubic, a = -0.5.
    Bicubic,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Bilinear => 1.0,
            Filter::Bicubic => 2.0,
        }
    }

    fn eval(self, x: f64) -> f64 {
        let x = x.abs();
        match self {
            Filter::Bilinear => (1.0 - x).max(0.0),
            Filter::Bicubic => {
                const A: f64 = -0.5;
                if x < 1.0 {
                    ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    (((x - 5.0) * x + 8.0) * x - 4.0) * A
                } else {
                    0.0
                }
            }
        }
    }
}

/// Taps of one resampled axis, as Pillow computes them: for every output
/// index the first input index and the normalised weights.
fn taps(len_in: usize, len_out: usize, filter: Filter) -> Vec<(usize, Vec<f64>)> {
    let scale = len_in as f64 / len_out as f64;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    (0..len_out)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = ((center + support + 0.5) as i64).min(len_in as i64) as usize - xmin;
            let ss = 1.0 / filterscale;
            let mut k: Vec<f64> = (0..xmax).map(|x| filter.eval((x as f64 + xmin as f64 - center + 0.5) * ss)).collect();
            let sum: f64 = k.iter().sum();
            if sum != 0.0 {
                k.iter_mut().for_each(|w| *w /= sum);
            }
            (xmin, k)
        })
        .collect()
}

/// Pillow's 8-bit resampling of interleaved pixels: two fixed-point passes,
/// horizontal first, each rounded back to 8 bits.
pub fn resize_u8(src: &[u8], w: usize, h: usize, ch: usize, ow: usize, oh: usize, filter: Filter) -> Vec<u8> {
    const BITS: u32 = 22;
    let fixed = |t: Vec<(usize, Vec<f64>)>| -> Vec<(usize, Vec<i64>)> {
        t.into_iter()
            .map(|(x0, k)| {
                (x0, k.iter().map(|&v| (v * f64::from(1u32 << BITS) + if v < 0.0 { -0.5 } else { 0.5 }) as i32 as i64).collect())
            })
            .collect()
    };
    let round = |ss: i64| (ss >> BITS).clamp(0, 255) as u8;
    let mut cur = src.to_vec();
    if ow != w {
        let tx = fixed(taps(w, ow, filter));
        let mut out = vec![0u8; ow * h * ch];
        par_rows(&mut out, ow * ch, |y, row| {
            let line = &cur[y * w * ch..];
            for (xx, (x0, k)) in tx.iter().enumerate() {
                for c in 0..ch {
                    let ss = k.iter().enumerate().fold(1i64 << (BITS - 1), |ss, (i, &kv)| ss + i64::from(line[(x0 + i) * ch + c]) * kv);
                    row[xx * ch + c] = round(ss);
                }
            }
        });
        cur = out;
    }
    if oh != h {
        let ty = fixed(taps(h, oh, filter));
        let stride = ow * ch;
        let mut out = vec![0u8; stride * oh];
        par_rows(&mut out, stride, |yy, row| {
            let (y0, k) = &ty[yy];
            for (x, v) in row.iter_mut().enumerate() {
                let ss = k.iter().enumerate().fold(1i64 << (BITS - 1), |ss, (i, &kv)| ss + i64::from(cur[(y0 + i) * stride + x]) * kv);
                *v = round(ss);
            }
        });
        cur = out;
    }
    cur
}

/// Pillow's resampling of a float ('F') image: double weights, float
/// pixels, horizontal pass first.
pub fn resize_f32(src: &[f32], w: usize, h: usize, ow: usize, oh: usize, filter: Filter) -> Vec<f32> {
    let mut cur = src.to_vec();
    if ow != w {
        let tx = taps(w, ow, filter);
        let mut out = vec![0f32; ow * h];
        par_rows(&mut out, ow, |y, row| {
            for (v, (x0, k)) in row.iter_mut().zip(&tx) {
                *v = k.iter().zip(&cur[y * w + x0..]).fold(0.0, |ss, (&kv, &p)| ss + f64::from(p) * kv) as f32;
            }
        });
        cur = out;
    }
    if oh != h {
        let ty = taps(h, oh, filter);
        let mut out = vec![0f32; ow * oh];
        par_rows(&mut out, ow, |yy, row| {
            let (y0, k) = &ty[yy];
            for (x, v) in row.iter_mut().enumerate() {
                *v = k.iter().enumerate().fold(0.0, |ss, (i, &kv)| ss + f64::from(cur[(y0 + i) * ow + x]) * kv) as f32;
            }
        });
        cur = out;
    }
    cur
}

/// An RGB image resized for a model and normalised with the ImageNet mean
/// and deviation, as a 1x3xHxW tensor. The normalisation runs in doubles
/// like NumPy's does in the prototype.
pub fn model_input(rgb: &[u8], w: usize, h: usize, ow: usize, oh: usize, filter: Filter) -> Tensor {
    const MEAN: [f64; 3] = [0.485, 0.456, 0.406];
    const STD: [f64; 3] = [0.229, 0.224, 0.225];
    let px = resize_u8(rgb, w, h, 3, ow, oh, filter);
    let mut data = vec![0f32; 3 * ow * oh];
    for c in 0..3 {
        for i in 0..ow * oh {
            data[c * ow * oh + i] = ((f64::from(f32::from(px[i * 3 + c]) / 255.0) - MEAN[c]) / STD[c]) as f32;
        }
    }
    Tensor::f32(vec![1, 3, oh, ow], data)
}

/// Depth Anything's input size: short side 518, both sides rounded up to
/// multiples of its 14-pixel patches.
pub fn depth_size(w: usize, h: usize) -> (usize, usize) {
    let s = 518.0 / w.min(h) as f64;
    let up = |v: usize| ((v as f64 * s / 14.0).ceil() as usize) * 14;
    (up(w), up(h))
}

/// Mask size: 2048 high, the wallpaper's aspect.
pub fn mask_size(w: usize, h: usize) -> (usize, usize) {
    ((w as f64 * 2048.0 / h as f64).round_ties_even() as usize, 2048)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Depth gate: fractions of the subject's median nearness, and the
/// dilation in depth pixels.
pub const LOW: f32 = 0.15;
pub const HIGH: f32 = 0.30;
pub const GROW: usize = 7;

pub struct Mask {
    pub width: usize,
    pub height: usize,
    /// Subject probability at mask size, 8 bits.
    pub subject: Vec<u8>,
    /// Final alpha at mask size, 8 bits.
    pub alpha: Vec<u8>,
}

/// The lock mask from BiRefNet's logits (1x1x1024x1024) and Depth
/// Anything's relative inverse depth (1xHxW): the subject, minus what lies
/// far behind it.
pub fn lock_mask(logits: &Tensor, depth: &Tensor, size: (usize, usize)) -> Result<Mask> {
    let (mw, mh) = size;
    let n = logits.shape.len();
    let (lh, lw) = (logits.shape[n - 2], logits.shape[n - 1]);
    let probability: Vec<f32> = logits.as_f32()?.iter().map(|&x| 1.0 / (1.0 + (-x).exp())).collect();
    let probability = resize_f32(&probability, lw, lh, mw, mh, Filter::Bilinear);
    let subject: Vec<f32> = probability.iter().map(|&p| smoothstep((p - 0.1) / 0.8)).collect();

    let n = depth.shape.len();
    let (dh, dw) = (depth.shape[n - 2], depth.shape[n - 1]);
    let depth = depth.as_f32()?;
    let small = resize_f32(&subject, mw, mh, dw, dh, Filter::Bilinear);
    let mut core: Vec<f32> = depth.iter().zip(&small).filter(|(_, s)| **s > 0.9).map(|(d, _)| *d).collect();
    let near: Vec<u8> = if core.is_empty() {
        vec![255; dw * dh]
    } else {
        core.sort_by(f32::total_cmp);
        let m = core.len();
        let median = if m % 2 == 1 { core[m / 2] } else { (core[m / 2 - 1] + core[m / 2]) / 2.0 };
        depth.iter().map(|&d| (smoothstep((d / median - LOW) / (HIGH - LOW)) * 255.0 + 0.5) as u8).collect()
    };
    // Pillow's MaxFilter on an edge-extended image: a separable max.
    let r = GROW / 2;
    let mut rows = vec![0u8; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            rows[y * dw + x] = (x.saturating_sub(r)..=(x + r).min(dw - 1)).map(|i| near[y * dw + i]).max().unwrap();
        }
    }
    let mut gate = vec![0u8; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            gate[y * dw + x] = (y.saturating_sub(r)..=(y + r).min(dh - 1)).map(|j| rows[j * dw + x]).max().unwrap();
        }
    }
    let gate = resize_u8(&gate, dw, dh, 1, mw, mh, Filter::Bilinear);
    ensure!(gate.len() == subject.len(), "mask sizes differ");
    let alpha = subject.iter().zip(&gate).map(|(&s, &g)| (s * (f32::from(g) / 255.0) * 255.0 + 0.5) as u8).collect();
    let subject = probability.iter().map(|&p| (p * 255.0 + 0.5) as u8).collect();
    Ok(Mask { width: mw, height: mh, subject, alpha })
}
