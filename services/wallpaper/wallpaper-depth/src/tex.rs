//! GEMM-family kernels that read their operands from textures. On the
//! Adreno 740 the texture path feeds a register-blocked GEMM about three
//! times faster than storage buffers do, and rgba16float textures convert to
//! f32 in the texture unit for free.
//!
//! Two layouts. A row texture holds R rows of W texels; row r sits at
//! y = r % height, layer r / height. A channel texture holds an NCHW tensor
//! as 4-channel texels, one layer per batch and channel group, with a zero
//! border as wide as the convolution's padding.

use anyhow::{Result, ensure};

use crate::kernels::{Binder, Kernel, Lazy, View, bits, grid};
use crate::tensor::numel;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexFormat {
    F16,
    F32,
}

impl TexFormat {
    pub fn wgsl(self) -> &'static str {
        match self {
            TexFormat::F16 => "rgba16float",
            TexFormat::F32 => "rgba32float",
        }
    }

    pub fn texel_bytes(self) -> usize {
        match self {
            TexFormat::F16 => 8,
            TexFormat::F32 => 16,
        }
    }

    /// What a value becomes when stored: f16 saturates instead of turning
    /// into infinity.
    pub fn store(self, v: &str) -> String {
        match self {
            TexFormat::F16 => format!("clamp({v}, vec4(-65504.0), vec4(65504.0))"),
            TexFormat::F32 => v.to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexDesc {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub format: TexFormat,
}

impl TexDesc {
    /// Whether a texture of this description can stand in for `d`: kernels
    /// address their own logical extent, so any larger one works.
    pub fn contains(&self, d: &TexDesc) -> bool {
        self.format == d.format && self.width >= d.width && self.height >= d.height && self.layers >= d.layers
    }

    pub fn union(&self, d: &TexDesc) -> TexDesc {
        TexDesc {
            width: self.width.max(d.width),
            height: self.height.max(d.height),
            layers: self.layers.max(d.layers),
            format: self.format,
        }
    }

    pub fn bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * u64::from(self.layers) * self.format.texel_bytes() as u64
    }
}

/// Height of one layer of a row texture that needs more than one; about
/// the height of the channel textures, so scratch textures can share.
const LAYER_ROWS: usize = 1024;

pub fn row_desc(rows: usize, width: usize, format: TexFormat) -> TexDesc {
    let height = rows.clamp(1, LAYER_ROWS);
    TexDesc { width: width.max(1) as u32, height: height as u32, layers: rows.div_ceil(height).max(1) as u32, format }
}

/// WGSL loading texel `x` of row `row` (u32 expressions) of a row texture.
fn row_load(name: &str, d: &TexDesc, x: &str, row: &str) -> String {
    if d.layers == 1 {
        format!("textureLoad({name}, vec2<i32>(i32({x}), i32({row})), 0, 0)")
    } else {
        format!("textureLoad({name}, vec2<i32>(i32({x}), i32(({row}) % {h}u)), i32(({row}) / {h}u), 0)", h = d.height)
    }
}

/// Texture contents for a row texture: `batch` blocks of `rows` rows of the
/// row-major matrix `data[b][r][c]` (r < `used`, c < `cols`, zero beyond),
/// one vec4 over c per texel.
pub fn rows_bytes(data: &[f32], batch: usize, used: usize, rows: usize, cols: usize, format: TexFormat) -> (TexDesc, Vec<u8>) {
    let width = cols.div_ceil(4);
    let desc = row_desc(batch * rows, width, format);
    let total = desc.layers as usize * desc.height as usize * width * 4;
    let mut f = vec![0f32; total];
    for b in 0..batch {
        for r in 0..used {
            let src = &data[(b * used + r) * cols..][..cols];
            f[(b * rows + r) * width * 4..][..cols].copy_from_slice(src);
        }
    }
    let bytes = match format {
        TexFormat::F32 => bytemuck::cast_slice(&f).to_vec(),
        TexFormat::F16 => {
            use half::slice::HalfFloatSliceExt;
            f.iter_mut().for_each(|x| *x = x.clamp(-65504.0, 65504.0));
            let mut h = vec![half::f16::ZERO; f.len()];
            h.convert_from_f32_slice(&f);
            bytemuck::cast_slice(h.reinterpret_cast()).to_vec()
        }
    };
    (desc, bytes)
}

/// Convolution weights [CO, C, KH, KW] as the B operand of a GEMM, texel
/// over 4 output channels. Rows run over channels and taps, taps inner
/// (`conv` walks the taps of one channel group before the next) or outer
/// (the deformable samples are tap-major).
pub fn conv_weight_bytes(w: &[f32], co: usize, c: usize, taps: usize, taps_inner: bool, format: TexFormat) -> (TexDesc, Vec<u8>) {
    let cp = c.div_ceil(4) * 4;
    let mut t = vec![0f32; taps * cp * co];
    for o in 0..co {
        for ci in 0..c {
            for tap in 0..taps {
                let row = if taps_inner { ((ci / 4) * taps + tap) * 4 + ci % 4 } else { tap * cp + ci };
                t[row * co + o] = w[(o * c + ci) * taps + tap];
            }
        }
    }
    rows_bytes(&t, 1, taps * cp, taps * cp, co, format)
}

/// An expression over `[batch..., R, C]` packed into a row texture with
/// `rows` rows per batch, vec4 over C. Loads are read through their own
/// strides, so broadcasts, transposes and elementwise ops cost nothing.
pub fn pack_rows(label: String, x: &Lazy, rows: usize, tex: usize, d: &TexDesc) -> Result<Kernel> {
    let rank = x.shape.len();
    ensure!(rank >= 2, "packing a {rank}-d tensor");
    let (r, c) = (x.shape[rank - 2], x.shape[rank - 1]);
    let batch = &x.shape[..rank - 2];
    let nb = numel(batch);
    let width = c.div_ceil(4);
    let mut bd = Binder::new();
    let names = x.loads.iter().map(|v| bd.bind(v, false)).collect::<Result<Vec<_>>>()?;
    let tn = bd.bind_tex(tex, d.format, true);
    let total = nb * rows * width;
    let mut params = vec![total as u32];
    params.extend(batch.iter().map(|&s| s as u32));
    let mut walk = String::new();
    for i in (0..batch.len()).rev() {
        walk += &format!("    let c{i} = b % p[{0}u]; b = b / p[{0}u];\n", 1 + i);
    }
    let (mut bases, mut loads) = (String::new(), String::new());
    for (k, v) in x.loads.iter().enumerate() {
        let at = params.len();
        params.push(v.offset as u32);
        params.extend(v.strides[..rank - 2].iter().map(|&s| bits(s)));
        let mut o = format!("pi({at}u) + i32(rr) * {}", v.strides[rank - 2]);
        for i in 0..batch.len() {
            o += &format!(" + i32(c{i}) * pi({}u)", at + 1 + i);
        }
        bases += &format!("    let o{k} = {o};\n");
        loads += &format!("            let a{k} = ld_{}(o{k} + i32(col) * {});\n", names[k], v.strides[rank - 1]);
    }
    let expr = x.named(|k| format!("a{k}"));
    let store = if d.layers == 1 {
        format!("textureStore({tn}, vec2<i32>(i32(x), i32(row)), 0, {})", d.format.store("v"))
    } else {
        format!("textureStore({tn}, vec2<i32>(i32(x), i32(row % {h}u)), i32(row / {h}u), {})", d.format.store("v"), h = d.height)
    };
    let body = format!(
        "
const W = {width}u; const ROWS = {rows}u; const R = {r}u; const C = {c}u;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
    let i = gid.x + gid.y * nwg.x * 64u;
    if (i >= p[0]) {{ return; }}
    let x = i % W;
    let row = i / W;
    let rr = row % ROWS;
    var b = row / ROWS;
{walk}{bases}    var v = vec4<f32>(0.0);
    if (rr < R) {{
        for (var q = 0u; q < 4u; q++) {{
            let col = x * 4u + q;
            if (col < C) {{
{loads}                v[q] = f32({expr});
            }}
        }}
    }}
    {store};
}}
"
    );
    let mut k = bd.finish(format!("{label} [pack]"), &body, params, grid(total, 64));
    k.work = total as u64 * 16;
    Ok(k)
}

/// Geometry of a channel texture: border before and after along y and x.
#[derive(Clone, Copy)]
pub struct Border {
    pub top: usize,
    pub left: usize,
    pub bottom: usize,
    pub right: usize,
}

pub fn channel_desc(n: usize, c: usize, h: usize, w: usize, b: Border, format: TexFormat) -> TexDesc {
    TexDesc { width: (w + b.left + b.right) as u32, height: (h + b.top + b.bottom) as u32, layers: (n * c.div_ceil(4)) as u32, format }
}

/// An NCHW expression as a channel texture with a zero border.
pub fn pack_channels(label: String, parts: &[Lazy], b: Border, tex: usize, d: &TexDesc) -> Result<Kernel> {
    ensure!(parts.iter().all(|x| x.shape.len() == 4), "channel texture of {:?}", parts.iter().map(|x| &x.shape).collect::<Vec<_>>());
    let (h, w) = (parts[0].shape[2], parts[0].shape[3]);
    let c: usize = parts.iter().map(|x| x.shape[1]).sum();
    let mut bd = Binder::new();
    let plane = (d.width * d.height) as usize;
    let total = plane * d.layers as usize;
    let mut params = vec![total as u32];
    // Each part covers the channels after the previous one's, and computes
    // them from its own loads.
    let (mut bases, mut branches) = (String::new(), String::new());
    let (mut k, mut start) = (0, 0);
    for x in parts {
        let names = x.loads.iter().map(|v| bd.bind(v, false)).collect::<Result<Vec<_>>>()?;
        let first = k;
        let mut loads = String::new();
        for (v, name) in x.loads.iter().zip(&names) {
            let st = &v.strides;
            bases += &format!("        let o{k} = pi({}u) + i32(nb) * {} + iy * {} + ix * {};\n", params.len(), st[0], st[2], st[3]);
            loads += &format!("                let a{k} = ld_{name}(o{k} + i32(c - {start}u) * {});\n", st[1]);
            params.push(v.offset as u32);
            k += 1;
        }
        let end = start + x.shape[1];
        let expr = x.named(|j| format!("a{}", first + j));
        branches += &format!(
            "{}if (c < {end}u) {{\n{loads}                v[q] = f32({expr});\n            }}",
            if start == 0 { "" } else { " else " }
        );
        start = end;
    }
    let body = format!(
        "
const TW = {tw}u; const PLANE = {plane}u; const C4 = {c4}u; const H = {h}; const W = {w};
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
    let i = gid.x + gid.y * nwg.x * 64u;
    if (i >= p[0]) {{ return; }}
    let layer = i / PLANE;
    let tx = i % TW;
    let ty = (i % PLANE) / TW;
    let iy = i32(ty) - {top};
    let ix = i32(tx) - {left};
    var v = vec4<f32>(0.0);
    if (iy >= 0 && iy < H && ix >= 0 && ix < W) {{
        let nb = layer / C4;
        let c0 = (layer % C4) * 4u;
{bases}        for (var q = 0u; q < 4u; q++) {{
            let c = c0 + q;
            {branches}
        }}
    }}
    textureStore({tn}, vec2<i32>(i32(tx), i32(ty)), i32(layer), {store});
}}
",
        tn = bd.bind_tex(tex, d.format, true),
        tw = d.width,
        c4 = c.div_ceil(4),
        top = b.top,
        left = b.left,
        store = d.format.store("v"),
    );
    let mut k = bd.finish(format!("{label} [pack]"), &body, params, grid(total, 64));
    k.work = total as u64 * 16;
    Ok(k)
}

/// Bilinear samples of a modulated deformable convolution, written as a
/// row texture with one row per output pixel and one texel per tap and
/// channel group (tap-major), so the convolution becomes a GEMM over it.
/// `x` is the input as a channel texture whose border makes its texel
/// coordinates the padded coordinates the positions are in; corners are
/// clipped to `bounds` and weighted by the unclipped fractions, as the
/// exported graph does.
pub struct DeformSample {
    pub c: usize,
    pub taps: usize,
    pub oh: usize,
    pub ow: usize,
    pub bounds: [i64; 2],
}

pub fn deform_desc(s: &DeformSample, format: TexFormat) -> TexDesc {
    row_desc(s.oh * s.ow, s.taps * s.c.div_ceil(4), format)
}

pub fn deform_sample(
    label: String,
    pos: &View,
    mask: &View,
    x: (usize, &TexDesc),
    out: (usize, &TexDesc),
    s: &DeformSample,
) -> Result<Kernel> {
    ensure!(pos.is_contiguous() && mask.is_contiguous(), "DeformConv positions and mask must be contiguous");
    let mut bd = Binder::new();
    let pn = bd.bind(pos, false)?;
    let mn = bd.bind(mask, false)?;
    let xn = bd.bind_tex(x.0, x.1.format, false);
    let on = bd.bind_tex(out.0, out.1.format, true);
    let od = out.1;
    let hw = s.oh * s.ow;
    let total = hw * s.taps;
    let at = if od.layers == 1 {
        "vec2<i32>(i32(tx), i32(px)), 0".to_string()
    } else {
        format!("vec2<i32>(i32(tx), i32(px % {h}u)), i32(px / {h}u)", h = od.height)
    };
    let body = format!(
        "
const HW = {hw}u; const C4 = {c4}u; const HY = {hy}; const HX = {hx};
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
    let i = gid.x + gid.y * nwg.x * 64u;
    if (i >= p[0]) {{ return; }}
    let px = i % HW;
    let tap = i / HW;
    let py = ld_{pn}(pi(1u) + i32(tap * 2u * HW + px));
    let pxx = ld_{pn}(pi(1u) + i32((tap * 2u + 1u) * HW + px));
    let m = ld_{mn}(pi(2u) + i32(tap * HW + px));
    let fy0 = floor(py);
    let fx0 = floor(pxx);
    let fy = py - fy0;
    let fx = pxx - fx0;
    let y0 = clamp(i32(fy0), 0, HY);
    let y1 = clamp(i32(fy0) + 1, 0, HY);
    let x0 = clamp(i32(fx0), 0, HX);
    let x1 = clamp(i32(fx0) + 1, 0, HX);
    let w00 = (1.0 - fy) * (1.0 - fx) * m;
    let w01 = (1.0 - fy) * fx * m;
    let w10 = fy * (1.0 - fx) * m;
    let w11 = fy * fx * m;
    for (var c4 = 0u; c4 < C4; c4++) {{
        let l = i32(c4);
        let v = textureLoad({xn}, vec2<i32>(x0, y0), l, 0) * w00 + textureLoad({xn}, vec2<i32>(x1, y0), l, 0) * w01
            + textureLoad({xn}, vec2<i32>(x0, y1), l, 0) * w10 + textureLoad({xn}, vec2<i32>(x1, y1), l, 0) * w11;
        let tx = tap * C4 + c4;
        textureStore({on}, {at}, {store});
    }}
}}
",
        c4 = s.c.div_ceil(4),
        hy = s.bounds[0],
        hx = s.bounds[1],
        store = od.format.store("v"),
    );
    let mut k = bd.finish(format!("{label} [sample]"), &body, vec![total as u32, pos.offset as u32, mask.offset as u32], grid(total, 64));
    k.work = (total * s.c.div_ceil(4)) as u64 * 32;
    Ok(k)
}

/// The register-blocked GEMM: each thread keeps an R x 4V block of C and
/// per k-step of 4 reads R texels of A (vec4 over k) and 4V texels of B
/// (vec4 over n). The caller supplies the fetches and the stores. Lanes run
/// along N, or along M when the stores want neighbouring rows.
struct Gemm<'a> {
    m: usize,
    n: usize,
    k4: usize,
    batch: usize,
    r: usize,
    v: usize,
    lanes_m: bool,
    /// Per-thread code once m, n and z are known.
    setup: String,
    /// Code at the top of each k-step (k is the step).
    kstep: String,
    /// Texel of A for row m + i.
    fetch_a: &'a dyn Fn(usize) -> String,
    /// Texel of B for row 4k + q and texel column nc{j}.
    fetch_b: &'a dyn Fn(usize, usize) -> String,
    /// Stores every c{i}_{j}: row m + i, texel column n + j.
    store: String,
}

/// Register block per thread: 4 x 8 for f16 operands, 8 x 8 for f32.
fn block(format: TexFormat) -> (usize, usize) {
    match format {
        TexFormat::F16 => (4, 2),
        TexFormat::F32 => (8, 2),
    }
}

fn gemm(label: String, bd: Binder, g: Gemm) -> Kernel {
    let n4 = g.n.div_ceil(4);
    let (r, v) = (g.r, g.v);
    let ncols = n4.div_ceil(v).clamp(1, 8);
    let (wx, wy) = if g.lanes_m { (64 / ncols, ncols) } else { (ncols, 64 / ncols) };
    let mut s = String::new();
    for j in 0..v {
        s += &format!("    let nc{j} = min(n + {j}u, N4 - 1u);\n");
    }
    s += &g.setup;
    for i in 0..r {
        for j in 0..v {
            s += &format!("    var c{i}_{j} = vec4<f32>(0.0);\n");
        }
    }
    s += "    for (var k = 0u; k < K4; k++) {\n";
    s += &g.kstep;
    for i in 0..r {
        s += &format!("        let a{i} = {};\n", (g.fetch_a)(i));
    }
    for q in 0..4 {
        for j in 0..v {
            s += &format!("        let b{q}_{j} = {};\n", (g.fetch_b)(q, j));
        }
    }
    for i in 0..r {
        for j in 0..v {
            s += &format!(
                "        c{i}_{j} = fma(vec4(a{i}.x), b0_{j}, fma(vec4(a{i}.y), b1_{j}, fma(vec4(a{i}.z), b2_{j}, fma(vec4(a{i}.w), b3_{j}, c{i}_{j}))));\n"
            );
        }
    }
    s += "    }\n";
    s += &g.store;
    let (mi, ni) = if g.lanes_m {
        (format!("((wg.x + p[0]) * {wx}u + lid.x) * {r}u"), format!("(wg.y * {wy}u + lid.y) * {v}u"))
    } else {
        (format!("((wg.y + p[0]) * {wy}u + lid.y) * {r}u"), format!("(wg.x * {wx}u + lid.x) * {v}u"))
    };
    let body = format!(
        "
const M = {m}u; const N = {n}u; const N4 = {n4}u; const K4 = {k4}u;
@compute @workgroup_size({wx}, {wy}, 1)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {{
    let m = {mi};
    let n = {ni};
    let z = wg.z;
{s}}}
",
        m = g.m,
        n = g.n,
        k4 = g.k4,
    );
    let (mg, ng) = (g.m.div_ceil(r * if g.lanes_m { wx } else { wy }) as u32, n4.div_ceil(v * if g.lanes_m { wy } else { wx }) as u32);
    let groups = if g.lanes_m { [mg, ng, g.batch as u32] } else { [ng, mg, g.batch as u32] };
    let mut k = bd.finish(format!("{label} [gemm]"), &body, vec![0], groups);
    k.split = Some((0, if g.lanes_m { 0 } else { 1 }));
    k.work = (g.m * g.n * g.k4 * 4 * g.batch) as u64;
    k
}

/// A bias for the GEMM columns: (accessor name, offset, vec4-bound).
type Bias = Option<(String, usize, bool)>;

fn bind_bias(bd: &mut Binder, bias: Option<&View>, n: usize) -> Result<Bias> {
    bias.map(|b| {
        if n.is_multiple_of(4) && b.offset % 4 == 0 && b.is_contiguous() {
            bd.bind_vec4(b, false).map(|name| (name, b.offset / 4, true))
        } else {
            bd.bind(b, false).map(|name| (name, b.offset, false))
        }
    })
    .transpose()
}

/// Bias of output column `col` (u32 expression), as an f32 expression.
fn bias_at(bias: &Bias, col: &str) -> String {
    match bias {
        None => String::new(),
        Some((name, off, true)) => format!(" + ld4_{name}({off} + i32(({col}) / 4u))[({col}) % 4u]"),
        Some((name, off, false)) => format!(" + ld_{name}({off} + i32({col}))"),
    }
}

/// C[z, m, n] = A[z, m, :] B[z, :, n] (+ bias[n]) for row textures A (vec4
/// over k, `m` rows per batch) and B (vec4 over n, 4 * k4 rows per batch).
/// With `batch` 1, a batched A is just more rows.
#[allow(clippy::too_many_arguments)]
pub fn matmul(
    label: String,
    c: &View,
    a: (usize, &TexDesc),
    b: (usize, &TexDesc),
    m: usize,
    n: usize,
    k4: usize,
    batch: usize,
    bias: Option<&View>,
) -> Result<Kernel> {
    ensure!(c.is_contiguous(), "MatMul output");
    let mut bd = Binder::new();
    let whole = n.is_multiple_of(4) && c.offset.is_multiple_of(4);
    let cn = if whole { bd.bind_vec4(c, true)? } else { bd.bind(c, true)? };
    let bias = bind_bias(&mut bd, bias, n)?;
    let an = bd.bind_tex(a.0, a.1.format, false);
    let bn = bd.bind_tex(b.0, b.1.format, false);
    let (ad, bdsc) = (*a.1, *b.1);
    let (r, v) = block(a.1.format);
    let fetch_a = |i: usize| row_load(&an, &ad, "k", &format!("z * M + min(m + {i}u, M - 1u)"));
    let fetch_b = |q: usize, j: usize| row_load(&bn, &bdsc, &format!("nc{j}"), &format!("z * K4 * 4u + k * 4u + {q}u"));
    let off = c.offset;
    let mut store = String::new();
    for i in 0..r {
        for j in 0..v {
            let row = format!("(z * M + m + {i}u)");
            store += &format!("    if (m + {i}u < M && n + {j}u < N4) {{\n");
            if whole {
                let mut val = format!("c{i}_{j}");
                if let Some((name, boff, vec)) = &bias {
                    val += &if *vec {
                        format!(" + ld4_{name}({boff} + i32(n + {j}u))")
                    } else {
                        let col = format!("(n + {j}u) * 4u");
                        format!(
                            " + vec4({}, {}, {}, {})",
                            &bias_at(&bias, &col.to_string())[3..],
                            &bias_at(&bias, &format!("{col} + 1u"))[3..],
                            &bias_at(&bias, &format!("{col} + 2u"))[3..],
                            &bias_at(&bias, &format!("{col} + 3u"))[3..]
                        )
                    };
                    let _ = name;
                }
                store += &format!("        st4_{cn}({} + i32({row} * N4 + n + {j}u), {val});\n", off / 4);
            } else {
                for (q, comp) in ["x", "y", "z", "w"].iter().enumerate() {
                    let col = format!("(n + {j}u) * 4u + {q}u");
                    store += &format!(
                        "        if ({col} < N) {{ st_{cn}({off} + i32({row} * N + {col}), c{i}_{j}.{comp}{}); }}\n",
                        bias_at(&bias, &col)
                    );
                }
            }
            store += "    }\n";
        }
    }
    Ok(gemm(
        label,
        bd,
        Gemm {
            m,
            n,
            k4,
            batch,
            r,
            v,
            lanes_m: false,
            setup: String::new(),
            kstep: String::new(),
            fetch_a: &fetch_a,
            fetch_b: &fetch_b,
            store,
        },
    ))
}

/// Stores for a GEMM whose rows are output pixels and columns output
/// channels of an NCHW tensor of `co` channels and `hw` pixels. `pixel` is
/// the u32 expression of row m's flat pixel index and rows m.. m + r - 1 are
/// consecutive pixels; `valid(i)` says whether row m + i is stored. With
/// `quads`, four rows go out as one vec4 (the caller guarantees alignment
/// and that a quad is all valid or all not).
fn planar_store(
    y: &str,
    off: usize,
    co: usize,
    hw: usize,
    r: usize,
    v: usize,
    pixel: &str,
    valid: &dyn Fn(usize) -> String,
    bias: &Bias,
    quads: bool,
) -> String {
    let mut s = String::new();
    for j in 0..v {
        for (q, comp) in ["x", "y", "z", "w"].iter().enumerate() {
            let col = format!("(n + {j}u) * 4u + {q}u");
            let b = bias_at(bias, &col);
            s += &format!("    if ({col} < N) {{\n        let plane = (z * {co}u + {col}) * {hw}u + {pixel};\n");
            if quads {
                for g in 0..r / 4 {
                    let i0 = 4 * g;
                    let val = format!(
                        "vec4(c{i0}_{j}.{comp}, c{}_{j}.{comp}, c{}_{j}.{comp}, c{}_{j}.{comp}){}",
                        i0 + 1,
                        i0 + 2,
                        i0 + 3,
                        if b.is_empty() { String::new() } else { format!(" + vec4({})", &b[3..]) }
                    );
                    s += &format!("        if ({}) {{ st4_{y}({} + i32((plane + {i0}u) / 4u), {val}); }}\n", valid(i0), off / 4);
                }
            } else {
                for i in 0..r {
                    s += &format!("        if ({}) {{ st_{y}({off} + i32(plane + {i}u), c{i}_{j}.{comp}{b}); }}\n", valid(i));
                }
            }
            s += "    }\n";
        }
    }
    s
}

pub struct ConvGeom {
    pub n: usize,
    pub c: usize,
    pub co: usize,
    pub kh: usize,
    pub kw: usize,
    pub oh: usize,
    pub ow: usize,
    pub stride: [usize; 2],
    pub dilation: [usize; 2],
}

/// Convolution of a channel texture whose border is the padding, as a GEMM
/// over output pixels (M), output channels (N) and input channel groups x
/// taps (K, taps inner, so neighbouring steps read neighbouring texels).
/// Each thread's pixels share an output row: rows are computed `OWP` wide,
/// a multiple of the block, and the extra columns are dropped.
pub fn conv(label: String, y: &View, x: (usize, &TexDesc), w: (usize, &TexDesc), bias: Option<&View>, g: &ConvGeom) -> Result<Kernel> {
    ensure!(y.is_contiguous(), "Conv output");
    let (r, v) = block(x.1.format);
    let quads = g.ow.is_multiple_of(4) && y.offset.is_multiple_of(4);
    let mut bd = Binder::new();
    let yn = if quads { bd.bind_vec4(y, true)? } else { bd.bind(y, true)? };
    let bias = bind_bias(&mut bd, bias, g.co)?;
    let xn = bd.bind_tex(x.0, x.1.format, false);
    let wn = bd.bind_tex(w.0, w.1.format, false);
    let wd = *w.1;
    let owp = g.ow.div_ceil(r) * r;
    let c4 = g.c.div_ceil(4);
    let taps = g.kh * g.kw;
    let setup =
        format!("    let mm = min(m, M - 1u);\n    let oy = mm / {owp}u;\n    let ox = mm % {owp}u;\n    let lbase = i32(z * {c4}u);\n");
    let (sh, sw, dh, dw) = (g.stride[0], g.stride[1], g.dilation[0], g.dilation[1]);
    let kstep = if taps == 1 {
        format!("        let xb = i32(ox * {sw}u);\n        let yb = i32(oy * {sh}u);\n        let layer = lbase + i32(k);\n")
    } else {
        format!(
            "        let tap = k % {taps}u;\n        let layer = lbase + i32(k / {taps}u);\n        let xb = i32(ox * {sw}u + (tap % {kw}u) * {dw}u);\n        let yb = i32(oy * {sh}u + (tap / {kw}u) * {dh}u);\n",
            kw = g.kw
        )
    };
    let fetch_a = |i: usize| format!("textureLoad({xn}, vec2<i32>(xb + {}, yb), layer, 0)", i * sw);
    let fetch_b = |q: usize, j: usize| row_load(&wn, &wd, &format!("nc{j}"), &format!("k * 4u + {q}u"));
    let ow = g.ow;
    let valid = |i: usize| format!("m + {i}u < M && ox + {i}u < {ow}u");
    let store = planar_store(&yn, y.offset, g.co, g.oh * g.ow, r, v, &format!("oy * {ow}u + ox"), &valid, &bias, quads);
    Ok(gemm(
        label,
        bd,
        Gemm {
            m: g.oh * owp,
            n: g.co,
            k4: taps * c4,
            batch: g.n,
            r,
            v,
            lanes_m: true,
            setup,
            kstep,
            fetch_a: &fetch_a,
            fetch_b: &fetch_b,
            store,
        },
    ))
}

/// The deformable convolution's GEMM over its samples (`deform_sample`):
/// output pixels (M) by output channels (N) by taps x channel groups (K).
pub fn deform_gemm(
    label: String,
    y: &View,
    a: (usize, &TexDesc),
    w: (usize, &TexDesc),
    bias: Option<&View>,
    s: &DeformSample,
    co: usize,
) -> Result<Kernel> {
    ensure!(y.is_contiguous(), "DeformConv output");
    let hw = s.oh * s.ow;
    let (r, v) = block(a.1.format);
    let quads = hw.is_multiple_of(4) && y.offset.is_multiple_of(4);
    let mut bd = Binder::new();
    let yn = if quads { bd.bind_vec4(y, true)? } else { bd.bind(y, true)? };
    let bias = bind_bias(&mut bd, bias, co)?;
    let an = bd.bind_tex(a.0, a.1.format, false);
    let wn = bd.bind_tex(w.0, w.1.format, false);
    let (ad, wd) = (*a.1, *w.1);
    let fetch_a = |i: usize| row_load(&an, &ad, "k", &format!("min(m + {i}u, M - 1u)"));
    let fetch_b = |q: usize, j: usize| row_load(&wn, &wd, &format!("nc{j}"), &format!("k * 4u + {q}u"));
    let valid = |i: usize| format!("m + {i}u < M");
    let store = planar_store(&yn, y.offset, co, hw, r, v, "m", &valid, &bias, quads);
    let k4 = s.taps * s.c.div_ceil(4);
    Ok(gemm(
        label,
        bd,
        Gemm {
            m: hw,
            n: co,
            k4,
            batch: 1,
            r,
            v,
            lanes_m: true,
            setup: String::new(),
            kstep: String::new(),
            fetch_a: &fetch_a,
            fetch_b: &fetch_b,
            store,
        },
    ))
}
