//! WGSL kernels. Each builder returns a shader specialised on rank, types
//! and expression, its parameters and its workgroup grid. Every operand binds
//! a whole buffer and finds its elements through offsets and strides in the
//! parameters, so views (transposes, slices, broadcasts) cost nothing.

use anyhow::{Result, bail, ensure};

use crate::tensor::{DType, numel, strides};

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub buf: usize,
    /// Size of the whole logical buffer, in 4-byte words.
    pub words: usize,
    pub offset: usize,
    pub shape: Vec<usize>,
    pub strides: Vec<isize>,
    pub dtype: DType,
}

impl View {
    pub fn contiguous(buf: usize, words: usize, shape: &[usize], dtype: DType) -> View {
        View { buf, words, offset: 0, shape: shape.to_vec(), strides: crate::tensor::signed(&strides(shape)), dtype }
    }

    pub fn numel(&self) -> usize {
        numel(&self.shape)
    }

    pub fn is_contiguous(&self) -> bool {
        let st = strides(&self.shape);
        self.shape.iter().zip(&self.strides).zip(&st).all(|((&d, &s), &c)| d == 1 || s == c as isize)
    }

    pub fn ty(&self) -> &'static str {
        wgsl_type(self.dtype)
    }
}

/// An elementwise expression not computed yet: `expr` over `loads`, each
/// load a view of `shape`. In `expr`, `$k$` stands for load k.
#[derive(Clone, Debug)]
pub struct Lazy {
    pub shape: Vec<usize>,
    pub dtype: DType,
    pub loads: Vec<View>,
    pub expr: String,
}

impl Lazy {
    pub fn of(v: &View) -> Lazy {
        Lazy { shape: v.shape.clone(), dtype: v.dtype, loads: vec![v.clone()], expr: "$0$".into() }
    }

    /// The expression with load k written as `name(k)`.
    pub fn named(&self, name: impl Fn(usize) -> String) -> String {
        replace_loads(&self.expr, name)
    }
}

/// `expr` with every `$k$` replaced by `name(k)`.
pub fn replace_loads(expr: &str, name: impl Fn(usize) -> String) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut rest = expr;
    while let Some(i) = rest.find('$') {
        out += &rest[..i];
        let after = &rest[i + 1..];
        let j = after.find('$').expect("unterminated load");
        out += &name(after[..j].parse().expect("load index"));
        rest = &after[j + 1..];
    }
    out + rest
}

pub fn wgsl_type(d: DType) -> &'static str {
    match d {
        DType::F32 => "f32",
        DType::I64 | DType::Bool => "i32",
    }
}

pub struct Kernel {
    pub label: String,
    pub wgsl: String,
    pub params: Vec<u32>,
    pub groups: [u32; 3],
    /// Logical buffers in binding order from 1; true when written.
    pub bufs: Vec<(usize, bool)>,
    /// Textures in binding order of group 1; true when written.
    pub texs: Vec<(usize, bool)>,
    /// For kernels that may be cut into chunks along a grid axis: the
    /// parameter holding the first workgroup along it, and the axis.
    pub split: Option<(usize, usize)>,
    /// Estimated work in multiply-adds; elementwise kernels count about 32
    /// per element, which is what they cost on the Adreno 740.
    pub work: u64,
}

impl Kernel {
    /// Cuts the grid's x range into chunks of at most `budget` work each.
    /// The GPU's hang check resets a dispatch that runs too long.
    pub fn chunks(self, budget: u64) -> Vec<Kernel> {
        let Some((at, axis)) = self.split else { return vec![self] };
        let parts = (self.work.div_ceil(budget) as u32).clamp(1, self.groups[axis]);
        if parts == 1 {
            return vec![self];
        }
        let per = self.groups[axis].div_ceil(parts);
        (0..self.groups[axis])
            .step_by(per as usize)
            .map(|g0| {
                let mut params = self.params.clone();
                params[at] = g0;
                let mut groups = self.groups;
                groups[axis] = per.min(self.groups[axis] - g0);
                Kernel {
                    label: self.label.clone(),
                    wgsl: self.wgsl.clone(),
                    params,
                    groups,
                    bufs: self.bufs.clone(),
                    texs: self.texs.clone(),
                    split: None,
                    work: self.work / u64::from(parts),
                }
            })
            .collect()
    }
}

const PRELUDE: &str = "
fn pi(i: u32) -> i32 { return bitcast<i32>(p[i]); }
fn pf(i: u32) -> f32 { return bitcast<f32>(p[i]); }
fn erf_(x: f32) -> f32 {
    let z = abs(x);
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t * exp(-z * z - 1.26551223 + t * (1.00002368 + t * (0.37409196 + t * (0.09678418 + t * (-0.18628806
        + t * (0.27886807 + t * (-1.13520398 + t * (1.48851587 + t * (-0.82215223 + t * 0.17087277)))))))));
    return select(r - 1.0, 1.0 - r, x >= 0.0);
}
";

/// Elements one binding reaches: 128 MiB of 4-byte words, Turnip's
/// maxStorageBufferRange. Larger buffers are bound as several windows.
pub const WINDOW: usize = 1 << 25;

pub fn windows(words: usize) -> usize {
    words.div_ceil(WINDOW).max(1)
}

pub(crate) struct Binder {
    decls: String,
    bufs: Vec<(usize, bool)>,
    types: Vec<&'static str>,
    next: usize,
    texs: Vec<(usize, bool)>,
}

impl Binder {
    pub(crate) fn new() -> Self {
        Binder {
            decls: "@group(0) @binding(0) var<storage, read> p: array<u32>;\n".into(),
            bufs: Vec::new(),
            types: Vec::new(),
            next: 1,
            texs: Vec::new(),
        }
    }

    /// Binds a texture in group 1, as a sampled array or, when written, a
    /// storage array of its format. Returns its WGSL name.
    pub(crate) fn bind_tex(&mut self, tex: usize, format: crate::tex::TexFormat, write: bool) -> String {
        let name = format!("t{}", self.texs.len());
        let ty = if write { format!("texture_storage_2d_array<{}, write>", format.wgsl()) } else { "texture_2d_array<f32>".into() };
        self.decls += &format!("@group(1) @binding({}) var {name}: {ty};\n", self.texs.len());
        self.texs.push((tex, write));
        name
    }

    /// Binds the buffer behind `v` and returns the name its accessors
    /// `ld_NAME(i)` and `st_NAME(i, v)` are generated under.
    pub(crate) fn bind(&mut self, v: &View, write: bool) -> Result<String> {
        let ty = v.ty();
        if let Some(i) = self.bufs.iter().position(|&(b, _)| b == v.buf) {
            ensure!(!write && !self.bufs[i].1 && self.types[i] == ty, "buffer {} bound twice", v.buf);
            return Ok(format!("b{}", i + 1));
        }
        self.bufs.push((v.buf, write));
        self.types.push(ty);
        let name = format!("b{}", self.bufs.len());
        let access = if write { "read_write" } else { "read" };
        let n = windows(v.words);
        for w in 0..n {
            self.decls += &format!("@group(0) @binding({}) var<storage, {access}> {name}_{w}: array<{ty}>;\n", self.next);
            self.next += 1;
        }
        if n == 1 {
            self.decls += &format!("fn ld_{name}(i: i32) -> {ty} {{ return {name}_0[i]; }}\n");
            if write {
                self.decls += &format!("fn st_{name}(i: i32, v: {ty}) {{ {name}_0[i] = v; }}\n");
            }
        } else {
            let mut ld = format!(
                "fn ld_{name}(i: i32) -> {ty} {{\n    let w = i >> {s}u;\n    let j = i & {m};\n",
                s = WINDOW.trailing_zeros(),
                m = WINDOW - 1
            );
            let mut st = format!(
                "fn st_{name}(i: i32, v: {ty}) {{\n    let w = i >> {s}u;\n    let j = i & {m};\n",
                s = WINDOW.trailing_zeros(),
                m = WINDOW - 1
            );
            for w in 0..n - 1 {
                ld += &format!("    if (w == {w}) {{ return {name}_{w}[j]; }}\n");
                st += &format!("    if (w == {w}) {{ {name}_{w}[j] = v; return; }}\n");
            }
            ld += &format!("    return {name}_{}[j];\n}}\n", n - 1);
            st += &format!("    {name}_{}[j] = v;\n}}\n", n - 1);
            self.decls += &ld;
            if write {
                self.decls += &st;
            }
        }
        Ok(name)
    }

    /// Binds an f32 buffer as vec4s for aligned whole-vector access through
    /// `ld4_NAME(i)` and `st4_NAME(i, v)`, `i` counting vec4s.
    pub(crate) fn bind_vec4(&mut self, v: &View, write: bool) -> Result<String> {
        ensure!(v.dtype == DType::F32, "vec4 binding of a {:?} buffer", v.dtype);
        ensure!(!self.bufs.iter().any(|&(b, _)| b == v.buf), "buffer {} bound twice", v.buf);
        self.bufs.push((v.buf, write));
        self.types.push("vec4");
        let name = format!("b{}", self.bufs.len());
        let access = if write { "read_write" } else { "read" };
        let n = windows(v.words);
        for w in 0..n {
            self.decls += &format!("@group(0) @binding({}) var<storage, {access}> {name}_{w}: array<vec4<f32>>;\n", self.next);
            self.next += 1;
        }
        let (shift, mask) = (WINDOW.trailing_zeros() - 2, WINDOW / 4 - 1);
        let mut ld = format!("fn ld4_{name}(i: i32) -> vec4<f32> {{\n    let w = i >> {shift}u;\n    let j = i & {mask};\n");
        let mut st = format!("fn st4_{name}(i: i32, v: vec4<f32>) {{\n    let w = i >> {shift}u;\n    let j = i & {mask};\n");
        for w in 0..n - 1 {
            ld += &format!("    if (w == {w}) {{ return {name}_{w}[j]; }}\n");
            st += &format!("    if (w == {w}) {{ {name}_{w}[j] = v; return; }}\n");
        }
        ld += &format!("    return {name}_{}[j];\n}}\n", n - 1);
        st += &format!("    {name}_{}[j] = v;\n}}\n", n - 1);
        self.decls += &ld;
        if write {
            self.decls += &st;
        }
        Ok(name)
    }

    pub(crate) fn finish(self, label: String, body: &str, params: Vec<u32>, groups: [u32; 3]) -> Kernel {
        let threads = groups.iter().map(|&g| u64::from(g)).product::<u64>() * 256;
        Kernel {
            label,
            wgsl: format!("{}{PRELUDE}{body}", self.decls),
            params,
            groups,
            bufs: self.bufs,
            texs: self.texs,
            split: None,
            work: threads * 32,
        }
    }
}

/// Workgroups for `n` invocations of `size` threads, spread over x and y.
pub(crate) fn grid(n: usize, size: usize) -> [u32; 3] {
    let groups = n.div_ceil(size).max(1);
    let x = groups.min(65535);
    [x as u32, groups.div_ceil(x) as u32, 1]
}

pub(crate) fn bits(v: isize) -> u32 {
    v as i32 as u32
}

/// Drops size-1 dims and merges dims every operand walks contiguously.
fn collapse(shape: &[usize], strides: &[Vec<isize>]) -> (Vec<usize>, Vec<Vec<isize>>) {
    let keep: Vec<usize> = (0..shape.len()).filter(|&d| shape[d] != 1).collect();
    let mut sh: Vec<usize> = keep.iter().map(|&d| shape[d]).collect();
    let mut st: Vec<Vec<isize>> = strides.iter().map(|s| keep.iter().map(|&d| s[d]).collect()).collect();
    let mut d = sh.len().saturating_sub(1);
    while d > 0 {
        if st.iter().all(|s| s[d - 1] == s[d] * sh[d] as isize) {
            sh[d - 1] *= sh[d];
            sh.remove(d);
            for s in st.iter_mut() {
                s[d - 1] = s[d];
                s.remove(d);
            }
        }
        d -= 1;
    }
    if sh.is_empty() {
        sh.push(1);
        st.iter_mut().for_each(|s| s.push(0));
    }
    (sh, st)
}

/// `out[i] = expr(a0, a1, ...)` over the output's shape, `ak` read from
/// `loads[k]` through its own strides.
pub fn map(label: String, out: &View, loads: &[View], expr: &str) -> Result<Kernel> {
    let mut operands = vec![out];
    operands.extend(loads);
    for v in &operands[1..] {
        ensure!(v.shape == out.shape, "map operand {:?} for output {:?}", v.shape, out.shape);
    }
    let (shape, st) = collapse(&out.shape, &operands.iter().map(|v| v.strides.clone()).collect::<Vec<_>>());
    let rank = shape.len();
    let mut b = Binder::new();
    let names: Vec<String> = operands.iter().enumerate().map(|(j, v)| b.bind(v, j == 0)).collect::<Result<_>>()?;
    let mut params = vec![out.numel() as u32];
    params.extend(operands.iter().map(|v| v.offset as u32));
    let sh = params.len();
    params.extend(shape.iter().map(|&d| d as u32));
    let sb = params.len();
    for s in &st {
        params.extend(s.iter().map(|&v| bits(v)));
    }
    let mut body = String::from(
        "@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= p[0]) { return; }
    var r = i;
",
    );
    for j in 0..operands.len() {
        body += &format!("    var o{j} = pi({}u);\n", 1 + j);
    }
    for d in (0..rank).rev() {
        if d == 0 {
            body += "    let c0 = i32(r);\n";
        } else {
            body += &format!("    let c{d} = i32(r % p[{0}u]); r = r / p[{0}u];\n", sh + d);
        }
        for j in 0..operands.len() {
            body += &format!("    o{j} += c{d} * pi({}u);\n", sb + j * rank + d);
        }
    }
    for k in 0..loads.len() {
        body += &format!("    let a{k} = ld_{}(o{});\n", names[k + 1], k + 1);
    }
    body += &format!("    st_{}(o0, {expr});\n}}\n", names[0]);
    Ok(b.finish(label, &body, params, grid(out.numel(), 256)))
}

/// The 64x64 output tile loop shared by MatMul and Conv. The caller supplies
/// `setup(z)`, `prep_b(n)`, `load_a(m, k)`, `load_b(k, n)` and
/// `store(m, n, v)`; M, N and K are p[0], p[1] and p[2].
const GEMM: &str = "
var<workgroup> As: array<vec4<f32>, 256>;
var<workgroup> Bs: array<vec4<f32>, 256>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(local_invocation_index) li: u32) {
    let M = p[0];
    let N = p[1];
    let K = p[2];
    setup(wg.z);
    let m0 = wg.y * 64u;
    let n0 = (wg.x + p[XOFF]) * 64u;
    let ak = li % 16u;
    let bn = li % 64u;
    let nb = n0 + bn;
    prep_b(nb);
    var acc0 = vec4<f32>(0.0);
    var acc1 = vec4<f32>(0.0);
    var acc2 = vec4<f32>(0.0);
    var acc3 = vec4<f32>(0.0);
    for (var k0 = 0u; k0 < K; k0 += 16u) {
        for (var t = 0u; t < 4u; t++) {
            let am = li / 16u + t * 16u;
            var av = 0.0;
            if (m0 + am < M && k0 + ak < K) { av = load_a(m0 + am, k0 + ak); }
            As[ak * 16u + am / 4u][am % 4u] = av;
            let bk = li / 64u + t * 4u;
            var bv = 0.0;
            if (k0 + bk < K && nb < N) { bv = load_b(k0 + bk, nb); }
            Bs[bk * 16u + bn / 4u][bn % 4u] = bv;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {
            let a = As[kk * 16u + lid.y];
            let b = Bs[kk * 16u + lid.x];
            acc0 += a.x * b;
            acc1 += a.y * b;
            acc2 += a.z * b;
            acc3 += a.w * b;
        }
        workgroupBarrier();
    }
    let m = m0 + lid.y * 4u;
    let n = n0 + lid.x * 4u;
    let rows = array<vec4<f32>, 4>(acc0, acc1, acc2, acc3);
    for (var i = 0u; i < 4u; i++) {
        if (m + i >= M) { break; }
        for (var j = 0u; j < 4u; j++) {
            if (n + j < N) { store(m + i, n + j, rows[i][j]); }
        }
    }
}
";

/// Output transform after a GEMM: an optional per-column (MatMul) or
/// per-row (Conv) bias and an activation.
pub struct Epilogue {
    pub bias: Option<View>,
    pub act: Option<&'static str>,
}

impl Epilogue {
    pub const NONE: Epilogue = Epilogue { bias: None, act: None };

    fn expr(&self, bias: &Option<String>, index: &str) -> String {
        let mut v = "v".to_string();
        if let Some(b) = bias {
            v = format!("({v} + ld_{b}(pi(BIAS) + i32({index})))");
        }
        match self.act {
            Some(a) => a.replace('$', &v),
            None => v,
        }
    }
}

fn k_dim(a: &View) -> usize {
    a.shape[a.shape.len() - 1]
}

/// C[b, m, n] = A[b, m, k] B[b, k, n] with up to three broadcast batch dims.
pub fn matmul(label: String, c: &View, a: &View, b: &View, epi: &Epilogue) -> Result<Kernel> {
    let (ra, rb, rc) = (a.shape.len(), b.shape.len(), c.shape.len());
    let (m, k, n) = (a.shape[ra - 2], a.shape[ra - 1], b.shape[rb - 1]);
    ensure!(b.shape[rb - 2] == k && c.shape[rc - 2..] == [m, n] && c.is_contiguous(), "MatMul shapes");
    let batch = &c.shape[..rc - 2];
    let bstr = |v: &View, r: usize| -> Vec<isize> {
        let pad = batch.len() + 2 - r;
        (0..batch.len()).map(|d| if d < pad || v.shape[d - pad] == 1 { 0 } else { v.strides[d - pad] }).collect()
    };
    let (bshape, bst) = collapse(batch, &[bstr(a, ra), bstr(b, rb), c.strides[..rc - 2].to_vec()]);
    ensure!(bshape.len() <= 3, "MatMul with batch {batch:?}");
    let mut bd = Binder::new();
    let cn = bd.bind(c, true)?;
    let an = bd.bind(a, false)?;
    let bn = bd.bind(b, false)?;
    let bias = epi.bias.as_ref().map(|v| bd.bind(v, false)).transpose()?;
    let mut params = vec![m as u32, n as u32, k as u32, a.offset as u32, b.offset as u32, c.offset as u32];
    params.extend([a.strides[ra - 2], a.strides[ra - 1], b.strides[rb - 2], b.strides[rb - 1]].map(bits));
    let pad3 = |v: &[isize], fill: isize| -> Vec<isize> {
        let mut out = vec![fill; 3 - v.len()];
        out.extend(v);
        out
    };
    params.extend(pad3(&bshape.iter().map(|&d| d as isize).collect::<Vec<_>>(), 1).into_iter().map(bits));
    for s in &bst {
        params.extend(pad3(s, 0).into_iter().map(bits));
    }
    params.push(epi.bias.as_ref().map_or(0, |v| v.offset as u32));
    params.push(0);
    let store = epi.expr(&bias, "n");
    let body = format!(
        "
var<private> ao: i32;
var<private> bo: i32;
var<private> co: i32;
fn setup(z: u32) {{
    var r = z;
    ao = pi(3u);
    bo = pi(4u);
    co = pi(5u);
    for (var d = 2; d >= 0; d--) {{
        let size = p[10 + d];
        let c = i32(r % size);
        r = r / size;
        ao += c * pi(u32(13 + d));
        bo += c * pi(u32(16 + d));
        co += c * pi(u32(19 + d));
    }}
}}
fn prep_b(n: u32) {{}}
fn load_a(m: u32, k: u32) -> f32 {{ return ld_{an}(ao + i32(m) * pi(6u) + i32(k) * pi(7u)); }}
fn load_b(k: u32, n: u32) -> f32 {{ return ld_{bn}(bo + i32(k) * pi(8u) + i32(n) * pi(9u)); }}
fn store(m: u32, n: u32, v: f32) {{ st_{cn}(co + i32(m * p[1] + n), {store}); }}
const BIAS = 22u;
const XOFF = 23u;
{GEMM}"
    );
    let zs = numel(&bshape);
    ensure!(zs <= 65535, "MatMul batch {zs}");
    let mut k = bd.finish(label, &body, params, [n.div_ceil(64) as u32, m.div_ceil(64) as u32, zs as u32]);
    k.split = Some((23, 0));
    k.work = (m * n * k_dim(a)) as u64 * zs as u64;
    Ok(k)
}

pub struct ConvShape {
    pub n: usize,
    pub c: usize,
    pub h: usize,
    pub w: usize,
    pub co: usize,
    pub kh: usize,
    pub kw: usize,
    pub oh: usize,
    pub ow: usize,
    pub stride: [usize; 2],
    pub pads: [usize; 4],
    pub dilation: [usize; 2],
    pub group: usize,
}

/// Convolution as an implicit GEMM: M output channels of a group, N output
/// pixels, K input channels times taps. The geometry is compiled in.
pub fn conv(label: String, y: &View, x: &View, w: &View, epi: &Epilogue, s: &ConvShape) -> Result<Kernel> {
    ensure!(x.is_contiguous() && w.is_contiguous() && y.is_contiguous(), "Conv operands must be contiguous");
    let mg = s.co / s.group;
    let cg = s.c / s.group;
    let k = cg * s.kh * s.kw;
    let mut bd = Binder::new();
    let yn = bd.bind(y, true)?;
    let xn = bd.bind(x, false)?;
    let wn = bd.bind(w, false)?;
    let bias = epi.bias.as_ref().map(|v| bd.bind(v, false)).transpose()?;
    let params = vec![
        mg as u32,
        (s.oh * s.ow) as u32,
        k as u32,
        x.offset as u32,
        w.offset as u32,
        y.offset as u32,
        epi.bias.as_ref().map_or(0, |v| v.offset as u32),
        0,
    ];
    let store = epi.expr(&bias, "g * MG + m");
    let body = format!(
        "
const C = {c}u; const H = {h}; const W = {w}; const CO = {co}u; const OW = {ow}u; const OHW = {ohw}u;
const KH = {kh}u; const KW = {kw}u; const SH = {sh}u; const SW = {sw}u; const DH = {dh}u; const DW = {dw}u;
const PT = {pt}; const PL = {pl}; const G = {g}u; const MG = {mg}u; const CG = {cg}u;
const BIAS = 6u;
const XOFF = 7u;
var<private> ni: u32;
var<private> g: u32;
var<private> oy: u32;
var<private> ox: u32;
fn setup(z: u32) {{ ni = z / G; g = z % G; }}
fn prep_b(n: u32) {{ oy = n / OW; ox = n % OW; }}
fn load_a(m: u32, k: u32) -> f32 {{ return ld_{wn}(pi(4u) + i32((g * MG + m) * p[2] + k)); }}
fn load_b(k: u32, n: u32) -> f32 {{
    let ci = k / (KH * KW);
    let r = k % (KH * KW);
    let iy = i32(oy * SH + (r / KW) * DH) - PT;
    let ix = i32(ox * SW + (r % KW) * DW) - PL;
    if (iy < 0 || iy >= H || ix < 0 || ix >= W) {{ return 0.0; }}
    return ld_{xn}(pi(3u) + (i32((ni * C + g * CG + ci)) * H + iy) * W + ix);
}}
fn store(m: u32, n: u32, v: f32) {{ st_{yn}(pi(5u) + i32((ni * CO + g * MG + m) * OHW + n), {store}); }}
{GEMM}",
        c = s.c,
        h = s.h,
        w = s.w,
        co = s.co,
        ow = s.ow,
        ohw = s.oh * s.ow,
        kh = s.kh,
        kw = s.kw,
        sh = s.stride[0],
        sw = s.stride[1],
        dh = s.dilation[0],
        dw = s.dilation[1],
        pt = s.pads[0],
        pl = s.pads[1],
        g = s.group,
    );
    let zs = s.n * s.group;
    let mut kernel = bd.finish(label, &body, params, [(s.oh * s.ow).div_ceil(64) as u32, mg.div_ceil(64) as u32, zs as u32]);
    kernel.split = Some((7, 0));
    kernel.work = (mg * s.oh * s.ow * k * zs) as u64;
    Ok(kernel)
}

pub struct DeformShape {
    pub c: usize,
    pub h: usize,
    pub w: usize,
    pub co: usize,
    pub k: usize,
    pub oh: usize,
    pub ow: usize,
    pub pads: [i64; 2],
    pub bounds: [i64; 2],
}

/// Modulated deformable convolution as an implicit GEMM whose B loader
/// takes the bilinear sample at the tap's position (see
/// `reference::deform_conv` for the semantics).
pub fn deform_conv(
    label: String,
    y: &View,
    x: &View,
    pos: &View,
    mask: &View,
    w: &View,
    epi: &Epilogue,
    s: &DeformShape,
) -> Result<Kernel> {
    for v in [y, x, pos, mask, w] {
        ensure!(v.is_contiguous(), "DeformConv operands must be contiguous");
    }
    let mut bd = Binder::new();
    let yn = bd.bind(y, true)?;
    let xn = bd.bind(x, false)?;
    let pn = bd.bind(pos, false)?;
    let mn = bd.bind(mask, false)?;
    let wn = bd.bind(w, false)?;
    let bias = epi.bias.as_ref().map(|v| bd.bind(v, false)).transpose()?;
    let (k2, hw) = (s.k * s.k, s.oh * s.ow);
    let params = vec![
        s.co as u32,
        hw as u32,
        (s.c * k2) as u32,
        x.offset as u32,
        pos.offset as u32,
        mask.offset as u32,
        w.offset as u32,
        y.offset as u32,
        epi.bias.as_ref().map_or(0, |v| v.offset as u32),
        0,
    ];
    let store = epi.expr(&bias, "m");
    let body = format!(
        "
const XH = {h}; const XW = {w}; const HW = {hw}u; const K2 = {k2}u;
const PT = {pt}; const PL = {pl}; const HY = {hy}; const HX = {hx};
const BIAS = 8u;
const XOFF = 9u;
fn setup(z: u32) {{}}
fn prep_b(n: u32) {{}}
fn sample(ci: u32, yy: i32, xx: i32) -> f32 {{
    let iy = yy - PT;
    let ix = xx - PL;
    if (iy < 0 || iy >= XH || ix < 0 || ix >= XW) {{ return 0.0; }}
    return ld_{xn}(pi(3u) + (i32(ci) * XH + iy) * XW + ix);
}}
fn load_a(m: u32, k: u32) -> f32 {{ return ld_{wn}(pi(6u) + i32(m * p[2] + k)); }}
fn load_b(k: u32, n: u32) -> f32 {{
    let ci = k / K2;
    let t = k % K2;
    let py = ld_{pn}(pi(4u) + i32(t * 2u * HW + n));
    let px = ld_{pn}(pi(4u) + i32((t * 2u + 1u) * HW + n));
    let fy0 = floor(py);
    let fx0 = floor(px);
    let fy = py - fy0;
    let fx = px - fx0;
    let y0 = clamp(i32(fy0), 0, HY);
    let y1 = clamp(i32(fy0) + 1, 0, HY);
    let x0 = clamp(i32(fx0), 0, HX);
    let x1 = clamp(i32(fx0) + 1, 0, HX);
    let v = (sample(ci, y0, x0) * (1.0 - fx) + sample(ci, y0, x1) * fx) * (1.0 - fy)
        + (sample(ci, y1, x0) * (1.0 - fx) + sample(ci, y1, x1) * fx) * fy;
    return v * ld_{mn}(pi(5u) + i32(t * HW + n));
}}
fn store(m: u32, n: u32, v: f32) {{ st_{yn}(pi(7u) + i32(m * HW + n), {store}); }}
{GEMM}",
        h = s.h,
        w = s.w,
        pt = s.pads[0],
        pl = s.pads[1],
        hy = s.bounds[0],
        hx = s.bounds[1],
    );
    let mut kernel = bd.finish(label, &body, params, [hw.div_ceil(64) as u32, s.co.div_ceil(64) as u32, 1]);
    kernel.split = Some((9, 0));
    kernel.work = (s.co * hw * s.c * k2) as u64 * 8;
    Ok(kernel)
}

/// Transposed convolution, one output element per thread (Depth Anything
/// uses two small ones).
pub fn conv_transpose(label: String, y: &View, x: &View, w: &View, bias: Option<&View>, s: &ConvShape) -> Result<Kernel> {
    ensure!(x.is_contiguous() && w.is_contiguous() && y.is_contiguous() && s.group == 1, "ConvTranspose operands");
    let mut bd = Binder::new();
    let yn = bd.bind(y, true)?;
    let xn = bd.bind(x, false)?;
    let wn = bd.bind(w, false)?;
    let bn = bias.map(|v| bd.bind(v, false)).transpose()?;
    let total = y.numel();
    let params = vec![total as u32, x.offset as u32, w.offset as u32, y.offset as u32, bias.map_or(0, |v| v.offset as u32)];
    let init = bn.map_or("0.0".to_string(), |b| format!("ld_{b}(pi(4u) + i32(co))"));
    let body = format!(
        "
const C = {c}u; const H = {h}; const W = {w}; const CO = {co}u; const OH = {oh}u; const OW = {ow}u;
const KH = {kh}u; const KW = {kw}u; const SH = {sh}; const SW = {sw}; const DH = {dh}; const DW = {dw};
const PT = {pt}; const PL = {pl};
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= p[0]) {{ return; }}
    let ox = i32(i % OW);
    let oy = i32((i / OW) % OH);
    let co = (i / (OW * OH)) % CO;
    let ni = i / (OW * OH * CO);
    var acc = {init};
    for (var ky = 0u; ky < KH; ky++) {{
        let ty = oy + PT - i32(ky) * DH;
        if (ty < 0 || ty % SH != 0 || ty / SH >= H) {{ continue; }}
        for (var kx = 0u; kx < KW; kx++) {{
            let tx = ox + PL - i32(kx) * DW;
            if (tx < 0 || tx % SW != 0 || tx / SW >= W) {{ continue; }}
            for (var ci = 0u; ci < C; ci++) {{
                let xv = ld_{xn}(pi(1u) + (i32(ni * C + ci) * H + ty / SH) * W + tx / SW);
                acc += xv * ld_{wn}(pi(2u) + i32(((ci * CO + co) * KH + ky) * KW + kx));
            }}
        }}
    }}
    st_{yn}(pi(3u) + i32(i), acc);
}}
",
        c = s.c,
        h = s.h,
        w = s.w,
        co = s.co,
        oh = s.oh,
        ow = s.ow,
        kh = s.kh,
        kw = s.kw,
        sh = s.stride[0],
        sw = s.stride[1],
        dh = s.dilation[0],
        dw = s.dilation[1],
        pt = s.pads[0],
        pl = s.pads[1],
    );
    Ok(bd.finish(label, &body, params, grid(total, 256)))
}

pub enum RowOp {
    /// Scale and bias views over the row, epsilon.
    LayerNorm(Option<View>, Option<View>, f32),
    Softmax,
    Mean,
}

/// Where a row op writes.
pub enum RowsOut<'a> {
    /// A contiguous buffer: the rows, or one value per row for Mean.
    Buf(&'a View),
    /// A row texture: one texture row per op row, vec4 texels along it.
    Tex(usize, &'a crate::tex::TexDesc),
}

/// A row op along the last dim of `x`. Each row gets a power-of-two group
/// of threads, so short rows share a workgroup, and up to 16 texels per
/// thread stay in registers, so `x` is read once.
pub fn rows(label: String, out: RowsOut, x: &Lazy, op: &RowOp) -> Result<Kernel> {
    let rank = x.shape.len();
    ensure!(rank >= 1, "row op of a scalar");
    let cols = x.shape[rank - 1];
    let lead = &x.shape[..rank - 1];
    let rows = numel(lead);
    let t4 = cols.div_ceil(4);
    let tpr = t4.next_power_of_two().clamp(1, 64);
    let rpw = 64 / tpr;
    let nt = t4.div_ceil(tpr);
    let cached = nt <= 16;
    let mut bd = Binder::new();
    let names = x.loads.iter().map(|v| bd.bind(v, false)).collect::<Result<Vec<_>>>()?;
    let mut params = vec![rows as u32];
    params.extend(lead.iter().map(|&d| d as u32));
    let mut prologue = String::new();
    for i in (0..lead.len()).rev() {
        prologue += &format!("    let c{i} = b % p[{0}u]; b = b / p[{0}u];\n", 1 + i);
    }
    let mut steps = Vec::new();
    for (k, v) in x.loads.iter().enumerate() {
        let at = params.len();
        params.push(v.offset as u32);
        params.extend(v.strides[..rank - 1].iter().map(|&s| bits(s)));
        let mut o = format!("pi({at}u)");
        for i in 0..lead.len() {
            o += &format!(" + i32(c{i}) * pi({}u)", at + 1 + i);
        }
        prologue += &format!("    let o{k} = {o};\n");
        steps.push(v.strides[rank - 1]);
    }
    let elem = |j: &str| x.named(|k| format!("ld_{}(o{k} + i32({j}) * {})", names[k], steps[k]));
    let pad = if matches!(op, RowOp::Softmax) { "-3.4e38" } else { "0.0" };
    // Fills `v` with texel `t` of the row.
    let load = |v: &str, t: &str| {
        format!(
            "    if ({t} < T4) {{\n        for (var q = 0u; q < 4u; q++) {{\n            let j = {t} * 4u + q;\n            if (j < COLS) {{ {v}[q] = f32({}); }}\n        }}\n    }}\n",
            elem("j")
        )
    };
    let (scale, bias, eps) = match op {
        RowOp::LayerNorm(s, b, eps) => {
            let s = s.as_ref().map(|v| bd.bind(v, false).map(|n| (n, v.offset))).transpose()?;
            let b = b.as_ref().map(|v| bd.bind(v, false).map(|n| (n, v.offset))).transpose()?;
            (s, b, *eps)
        }
        _ => (None, None, 0.0),
    };
    // Stores texel `t` of the row from `v`.
    let store = match out {
        RowsOut::Tex(tex, d) => {
            let tn = bd.bind_tex(tex, d.format, true);
            let at = if d.layers == 1 {
                "vec2<i32>(i32(T), i32(row)), 0".to_string()
            } else {
                format!("vec2<i32>(i32(T), i32(row % {h}u)), i32(row / {h}u)", h = d.height)
            };
            format!("textureStore({tn}, {at}, {})", d.format.store("V"))
        }
        RowsOut::Buf(y) if matches!(op, RowOp::Mean) => {
            let yn = bd.bind(y, true)?;
            format!("st_{yn}({} + i32(row), V.x)", y.offset)
        }
        RowsOut::Buf(y) if cols.is_multiple_of(4) && y.offset % 4 == 0 => {
            ensure!(y.is_contiguous(), "row op output");
            let yn = bd.bind_vec4(y, true)?;
            format!("st4_{yn}({} + i32(row * T4 + T), V)", y.offset / 4)
        }
        RowsOut::Buf(y) => {
            ensure!(y.is_contiguous(), "row op output");
            let yn = bd.bind(y, true)?;
            format!(
                "for (var q = 0u; q < 4u; q++) {{ if (T * 4u + q < COLS) {{ st_{yn}({} + i32(row * COLS + T * 4u + q), V[q]); }} }}",
                y.offset
            )
        }
    };
    let affine = |v: &str, t: &str| {
        let mut s = String::new();
        if scale.is_some() || bias.is_some() {
            s += &format!("    for (var q = 0u; q < 4u; q++) {{\n        let j = min({t} * 4u + q, COLS - 1u);\n");
            if let Some((n, off)) = &scale {
                s += &format!("        {v}[q] *= ld_{n}({off} + i32(j));\n");
            }
            if let Some((n, off)) = &bias {
                s += &format!("        {v}[q] += ld_{n}({off} + i32(j));\n");
            }
            s += "    }\n";
        }
        s
    };
    let put = |v: &str, t: &str| format!("    if (live && {t} < T4) {{ let T = {t}; let V = {v}; {store}; }}\n");
    let mask =
        |t: &str| format!("select(vec4<f32>(0.0), vec4<f32>(1.0), vec4<u32>({t} * 4u) + vec4<u32>(0u, 1u, 2u, 3u) < vec4<u32>(COLS))");
    let sum4 = |v: &str| format!("({v}.x + {v}.y + {v}.z + {v}.w)");
    let mut main = String::new();
    if cached {
        for t in 0..nt {
            main += &format!("    let x{t} = lr + {t}u * TPR;\n    var v{t} = vec4<f32>({pad});\n");
            main += &load(&format!("v{t}"), &format!("x{t}"));
        }
        let each = |f: &dyn Fn(usize) -> String| (0..nt).map(f).collect::<String>();
        main += &match op {
            RowOp::Softmax => format!(
                "    var m = -3.4e38;\n{}    let mx = seg_max(m, li);\n    var s = 0.0;\n{}    let inv = 1.0 / seg_sum(s, li);\n{}",
                each(&|t| format!("    m = max(m, max(max(v{t}.x, v{t}.y), max(v{t}.z, v{t}.w)));\n")),
                each(&|t| format!("    v{t} = exp(v{t} - mx);\n    s += {};\n", sum4(&format!("v{t}")))),
                each(&|t| put(&format!("v{t} * inv"), &format!("x{t}"))),
            ),
            RowOp::LayerNorm(..) => format!(
                "    var s = 0.0;\n{}    let mean = seg_sum(s, li) / f32(COLS);\n    var q2 = 0.0;\n{}    let inv = inverseSqrt(seg_sum(q2, li) / f32(COLS) + {eps:e});\n{}",
                each(&|t| format!("    s += {};\n", sum4(&format!("v{t}")))),
                each(&|t| format!("    v{t} = (v{t} - mean) * {};\n    q2 += dot(v{t}, v{t});\n", mask(&format!("x{t}")))),
                each(&|t| format!(
                    "    v{t} *= inv;\n{}{}",
                    affine(&format!("v{t}"), &format!("x{t}")),
                    put(&format!("v{t}"), &format!("x{t}"))
                )),
            ),
            RowOp::Mean => format!(
                "    var s = 0.0;\n{}    let total = seg_sum(s, li) / f32(COLS);\n    if (lr == 0u) {{ {} }}\n",
                each(&|t| format!("    s += {};\n", sum4(&format!("v{t}")))),
                put("vec4<f32>(total)", "0u"),
            ),
        };
    } else {
        let walk = |body: &str| {
            format!("    for (var x = lr; x < T4; x += TPR) {{\n        var v = vec4<f32>({pad});\n{}{body}    }}\n", load("v", "x"))
        };
        main += &match op {
            RowOp::Softmax => format!(
                "    var m = -3.4e38;\n{}    let mx = seg_max(m, li);\n    var s = 0.0;\n{}    let inv = 1.0 / seg_sum(s, li);\n{}",
                walk("        m = max(m, max(max(v.x, v.y), max(v.z, v.w)));\n"),
                walk(&format!("        s += {};\n", sum4("exp(v - mx)"))),
                walk(&put("exp(v - mx) * inv", "x")),
            ),
            RowOp::LayerNorm(..) => format!(
                "    var s = 0.0;\n{}    let mean = seg_sum(s, li) / f32(COLS);\n    var q2 = 0.0;\n{}    let inv = inverseSqrt(seg_sum(q2, li) / f32(COLS) + {eps:e});\n{}",
                walk(&format!("        s += {};\n", sum4("v"))),
                walk(&format!("        let d = (v - mean) * {};\n        q2 += dot(d, d);\n", mask("x"))),
                walk(&format!("        v = (v - mean) * inv;\n{}{}", affine("v", "x"), put("v", "x"))),
            ),
            RowOp::Mean => format!(
                "    var s = 0.0;\n{}    let total = seg_sum(s, li) / f32(COLS);\n    if (lr == 0u) {{ {} }}\n",
                walk(&format!("        s += {};\n", sum4("v"))),
                put("vec4<f32>(total)", "0u"),
            ),
        };
    }
    let body = format!(
        "
const COLS = {cols}u; const T4 = {t4}u; const TPR = {tpr}u; const RPW = {rpw}u;
var<workgroup> red: array<f32, 64>;
fn seg_sum(v: f32, li: u32) -> f32 {{
    red[li] = v;
    workgroupBarrier();
    for (var s = TPR / 2u; s > 0u; s >>= 1u) {{
        if (li % TPR < s) {{ red[li] += red[li + s]; }}
        workgroupBarrier();
    }}
    let r = red[li - li % TPR];
    workgroupBarrier();
    return r;
}}
fn seg_max(v: f32, li: u32) -> f32 {{
    red[li] = v;
    workgroupBarrier();
    for (var s = TPR / 2u; s > 0u; s >>= 1u) {{
        if (li % TPR < s) {{ red[li] = max(red[li], red[li + s]); }}
        workgroupBarrier();
    }}
    let r = red[li - li % TPR];
    workgroupBarrier();
    return r;
}}
@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>,
        @builtin(local_invocation_index) li: u32) {{
    let first = (wg.x + wg.y * nwg.x) * RPW;
    let lr = li % TPR;
    let live = first + li / TPR < p[0];
    let row = min(first + li / TPR, p[0] - 1u);
    var b = row;
{prologue}{main}}}
"
    );
    let groups = rows.div_ceil(rpw).max(1);
    let gx = groups.min(65535);
    Ok(bd.finish(label, &body, params, [gx as u32, groups.div_ceil(gx) as u32, 1]))
}

/// Linear Resize of the last two dims of a contiguous NCHW tensor.
pub fn resize_linear(label: String, y: &View, x: &View, ctm: &str, scale: [f32; 2]) -> Result<Kernel> {
    ensure!(x.is_contiguous() && y.is_contiguous() && x.shape.len() == 4, "Resize operands");
    let (h, w, oh, ow) = (x.shape[2], x.shape[3], y.shape[2], y.shape[3]);
    let coord = |len_in: usize, len_out: usize, s: f32, v: &str| -> Result<String> {
        Ok(match ctm {
            "align_corners" if len_out == 1 => "0.0".into(),
            "align_corners" => format!("f32({v}) * {:?}", (len_in as f32 - 1.0) / (len_out as f32 - 1.0)),
            "half_pixel" => format!("(f32({v}) + 0.5) / {s:?} - 0.5"),
            "pytorch_half_pixel" if len_out > 1 => format!("(f32({v}) + 0.5) / {s:?} - 0.5"),
            "pytorch_half_pixel" => "0.0".into(),
            "asymmetric" => format!("f32({v}) / {s:?}"),
            m => bail!("coordinate_transformation_mode {m}"),
        })
    };
    let (cy, cx) = (coord(h, oh, scale[0], "oy")?, coord(w, ow, scale[1], "ox")?);
    let mut bd = Binder::new();
    let yn = bd.bind(y, true)?;
    let xn = bd.bind(x, false)?;
    let total = y.numel();
    let params = vec![total as u32, x.offset as u32, y.offset as u32];
    let body = format!(
        "
const H = {h}u; const W = {w}u; const OH = {oh}u; const OW = {ow}u;
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= p[0]) {{ return; }}
    let ox = i % OW;
    let oy = (i / OW) % OH;
    let plane = i / (OW * OH);
    let fy = clamp({cy}, 0.0, f32(H - 1u));
    let fx = clamp({cx}, 0.0, f32(W - 1u));
    let y0 = u32(floor(fy));
    let x0 = u32(floor(fx));
    let y1 = min(y0 + 1u, H - 1u);
    let x1 = min(x0 + 1u, W - 1u);
    let dy = fy - f32(y0);
    let dx = fx - f32(x0);
    let base = pi(1u) + i32(plane * H * W);
    let v00 = ld_{xn}(base + i32(y0 * W + x0));
    let v01 = ld_{xn}(base + i32(y0 * W + x1));
    let v10 = ld_{xn}(base + i32(y1 * W + x0));
    let v11 = ld_{xn}(base + i32(y1 * W + x1));
    st_{yn}(pi(2u) + i32(i), (v00 * (1.0 - dx) + v01 * dx) * (1.0 - dy) + (v10 * (1.0 - dx) + v11 * dx) * dy);
}}
"
    );
    Ok(bd.finish(label, &body, params, grid(total, 256)))
}

/// Constant padding of a strided input into a contiguous output.
pub fn pad(label: String, y: &View, x: &View, before: &[isize], value: &str) -> Result<Kernel> {
    ensure!(y.is_contiguous(), "Pad output");
    let rank = y.shape.len();
    let mut bd = Binder::new();
    let yn = bd.bind(y, true)?;
    let xn = bd.bind(x, false)?;
    let mut params = vec![y.numel() as u32, x.offset as u32, y.offset as u32];
    params.extend(y.shape.iter().map(|&d| d as u32));
    params.extend(x.shape.iter().map(|&d| d as u32));
    params.extend(x.strides.iter().map(|&s| bits(s)));
    params.extend(before.iter().map(|&b| bits(b)));
    let mut body = String::from(
        "@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= p[0]) { return; }
    var r = i;
    var o = pi(1u);
    var inside = true;
",
    );
    for d in (0..rank).rev() {
        let (ys, xs, st, bf) = (3 + d, 3 + rank + d, 3 + 2 * rank + d, 3 + 3 * rank + d);
        body += &format!(
            "    {{ let c = i32(r % p[{ys}u]); r = r / p[{ys}u]; let s = c - pi({bf}u);
      inside = inside && s >= 0 && s < pi({xs}u); o += s * pi({st}u); }}\n"
        );
    }
    body += &format!("    var v = {value};\n    if (inside) {{ v = ld_{xn}(o); }}\n    st_{yn}(pi(2u) + i32(i), v);\n}}\n");
    Ok(bd.finish(label, &body, params, grid(y.numel(), 256)))
}

/// Gather along `axis` of a contiguous tensor with int indices.
pub fn gather(label: String, y: &View, x: &View, ind: &View, axis: usize) -> Result<Kernel> {
    ensure!(x.is_contiguous() && ind.is_contiguous() && y.is_contiguous(), "Gather operands");
    let outer = numel(&x.shape[..axis]);
    let inner = numel(&x.shape[axis + 1..]);
    let (n, k) = (x.shape[axis], ind.numel());
    let mut bd = Binder::new();
    let yn = bd.bind(y, true)?;
    let xn = bd.bind(x, false)?;
    let iname = bd.bind(ind, false)?;
    let params = vec![y.numel() as u32, x.offset as u32, ind.offset as u32, y.offset as u32];
    let _ = outer;
    let body = format!(
        "
const N = {n}; const K = {k}u; const INNER = {inner}u;
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= p[0]) {{ return; }}
    let j = i % INNER;
    let q = (i / INNER) % K;
    let o = i / (INNER * K);
    var g = ld_{iname}(pi(2u) + i32(q));
    if (g < 0) {{ g += N; }}
    st_{yn}(pi(3u) + i32(i), ld_{xn}(pi(1u) + (i32(o) * N + g) * i32(INNER) + i32(j)));
}}
"
    );
    Ok(bd.finish(label, &body, params, grid(y.numel(), 256)))
}

/// GatherND with batch dims over contiguous operands.
pub fn gather_nd(label: String, y: &View, x: &View, ind: &View, b: usize) -> Result<Kernel> {
    ensure!(x.is_contiguous() && ind.is_contiguous() && y.is_contiguous(), "GatherND operands");
    let q = ind.shape.len();
    let k = ind.shape[q - 1];
    let m = numel(&ind.shape[b..q - 1]);
    let inner = numel(&x.shape[b + k..]);
    let block = numel(&x.shape[b..]);
    let st = strides(&x.shape[b..]);
    let mut bd = Binder::new();
    let yn = bd.bind(y, true)?;
    let xn = bd.bind(x, false)?;
    let iname = bd.bind(ind, false)?;
    let params = vec![y.numel() as u32, x.offset as u32, ind.offset as u32, y.offset as u32];
    let mut sum = String::new();
    for (j, s) in st.iter().take(k).enumerate() {
        sum += &format!("    {{ var v = ld_{iname}(t + {j}); if (v < 0) {{ v += {d}; }} off += v * {s}; }}\n", d = x.shape[b + j]);
    }
    let body = format!(
        "
const M = {m}u; const INNER = {inner}u; const BLOCK = {block};
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= p[0]) {{ return; }}
    let j = i % INNER;
    let row = i / INNER;
    let bi = row / M;
    let t = pi(2u) + i32(row * {k}u);
    var off = i32(bi) * BLOCK;
{sum}    st_{yn}(pi(3u) + i32(i), ld_{xn}(pi(1u) + off + i32(j)));
}}
"
    );
    Ok(bd.finish(label, &body, params, grid(y.numel(), 256)))
}
