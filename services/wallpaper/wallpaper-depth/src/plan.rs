//! Lowers an ONNX graph for one fixed input shape. Constants and the shape
//! graph fold on the CPU; reshapes, transposes, slices and broadcasts become
//! views; everything else becomes GPU dispatches.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use anyhow::{Context, Result, bail, ensure};

use crate::kernels::{self, ConvShape, Epilogue, Kernel, Lazy, RowOp, RowsOut, View, replace_loads};
use crate::onnx::{Model, Node};
use crate::reference;
use crate::tensor::{DType, Data, Tensor, broadcast_shape, numel, signed, strides};
use crate::tex::{self, TexDesc, TexFormat};

#[derive(Clone)]
pub enum Val {
    Const(Rc<Tensor>),
    Gpu(View),
    /// Elementwise ops not computed yet: their consumer evaluates them,
    /// inside its own kernel when it can.
    Lazy(Rc<Lazy>),
    /// A row op not computed yet: a MatMul has it write its operand
    /// texture, anything else has it write memory.
    Rows(Rc<Rows>),
    /// A concatenation not copied yet: a convolution packs its parts
    /// straight into its input texture, anything else has them copied.
    Cat(Rc<Cat>),
}

/// Values joined along `axis` into `shape`.
pub struct Cat {
    pub axis: usize,
    pub parts: Vec<Val>,
    pub shape: Vec<usize>,
    pub dtype: DType,
}

/// A row op along the last dim of `x`, whose result has `shape`.
pub struct Rows {
    pub x: Lazy,
    pub op: RowOp,
    pub shape: Vec<usize>,
}

impl Val {
    pub fn shape(&self) -> &[usize] {
        match self {
            Val::Const(t) => &t.shape,
            Val::Gpu(v) => &v.shape,
            Val::Lazy(l) => &l.shape,
            Val::Rows(r) => &r.shape,
            Val::Cat(c) => &c.shape,
        }
    }

    pub fn dtype(&self) -> DType {
        match self {
            Val::Const(t) => t.dtype(),
            Val::Gpu(v) => v.dtype,
            Val::Lazy(l) => l.dtype,
            Val::Rows(_) => DType::F32,
            Val::Cat(c) => c.dtype,
        }
    }
}

/// A logical buffer. Weights carry their contents and live for the whole
/// run; scratch buffers share physical memory once they are dead.
pub struct Buf {
    pub words: usize,
    pub init: Option<Vec<u32>>,
}

/// A texture; weight textures carry their contents.
pub struct Tex {
    pub desc: TexDesc,
    pub init: Option<Vec<u8>>,
}

/// What the planner may use on the device.
#[derive(Clone)]
pub struct Options {
    /// Multiply-adds one dispatch may do, if the device limits it.
    pub budget: Option<u64>,
    /// Texture format for GEMM operands, or None for the buffer kernels.
    pub tex: Option<TexFormat>,
    pub max_tex_dim: u32,
    pub max_tex_layers: u32,
    /// Whether elementwise ops wait for their consumers (`Val::Lazy`).
    pub fuse: bool,
}

/// The dispatches one graph node became, with the values it read and wrote.
pub struct Step {
    pub node: usize,
    pub dispatches: Range<usize>,
    pub inputs: Vec<Option<Val>>,
    pub outputs: Vec<Val>,
}

pub struct Program {
    pub opset: i64,
    pub nodes: Vec<Node>,
    pub bufs: Vec<Buf>,
    pub textures: Vec<Tex>,
    pub kernels: Vec<Kernel>,
    pub steps: Vec<Step>,
    pub input: View,
    pub outputs: Vec<Val>,
}

struct Planner {
    opts: Options,
    bufs: Vec<Buf>,
    textures: Vec<Tex>,
    kernels: Vec<Kernel>,
    // The caches below are keyed by address and hold on to the value, so an
    // address is never reused for another one.
    uploaded: HashMap<*const Tensor, (Rc<Tensor>, View)>,
    /// Weight textures by tensor and layout.
    packed: HashMap<(*const Tensor, &'static str), (Rc<Tensor>, usize, TexDesc)>,
    /// Deferred values some consumer needed in memory, by address.
    materialized: HashMap<usize, (Val, View)>,
    label: String,
}

pub fn plan(model: Model, input_shape: &[usize], opts: &Options) -> Result<Program> {
    let Model { opset, nodes, initializers, inputs, outputs } = model;
    ensure!(inputs.len() == 1, "models with {} inputs", inputs.len());
    let mut p = Planner {
        opts: opts.clone(),
        bufs: Vec::new(),
        textures: Vec::new(),
        kernels: Vec::new(),
        uploaded: HashMap::new(),
        packed: HashMap::new(),
        materialized: HashMap::new(),
        label: String::new(),
    };
    let mut values: HashMap<String, Val> = initializers.into_iter().map(|(n, t)| (n, Val::Const(Rc::new(t)))).collect();
    let input = p.alloc(input_shape, DType::F32);
    values.insert(inputs[0].clone(), Val::Gpu(input.clone()));
    let mut steps = Vec::new();
    let verbose = std::env::var_os("WALLPAPER_DEPTH_VERBOSE").is_some();
    let mut costs: HashMap<String, std::time::Duration> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let started = std::time::Instant::now();
        let ins: Vec<Option<Val>> = node
            .inputs
            .iter()
            .map(|n| match n.as_str() {
                "" => Ok(None),
                n => values.get(n).cloned().map(Some).with_context(|| format!("{} reads unknown {n}", node.label())),
            })
            .collect::<Result<_>>()?;
        let outs: Vec<Val> = if node.op == "Identity" {
            vec![ins[0].clone().context("Identity of nothing")?]
        } else if node.op == "Shape" {
            let shape = ins[0].as_ref().context("Shape of nothing")?.shape().to_vec();
            vec![Val::Const(Rc::new(reference::shape_of(node, &shape)))]
        } else if ins.iter().flatten().all(|v| matches!(v, Val::Const(_))) {
            let consts: Vec<Option<&Tensor>> = ins
                .iter()
                .map(|v| match v {
                    Some(Val::Const(t)) => Some(&**t),
                    _ => None,
                })
                .collect();
            reference::eval(node, &consts, opset)
                .with_context(|| format!("folding {}", node.label()))?
                .into_iter()
                .map(|t| Val::Const(Rc::new(t)))
                .collect()
        } else {
            let start = p.kernels.len();
            p.label = node.label();
            let outs = p.lower(node, &ins, opset).with_context(|| format!("lowering {}", node.label()))?;
            steps.push(Step { node: i, dispatches: start..p.kernels.len(), inputs: ins, outputs: outs.clone() });
            outs
        };
        if verbose {
            let folded = matches!(outs.first(), Some(Val::Const(_)));
            if started.elapsed().as_millis() >= 3 {
                eprintln!("  slow {} {:?}: {:.2?}", node.label(), outs.first().map(|v| v.shape().to_vec()), started.elapsed());
            }
            *costs.entry(format!("{} {}", if folded { "fold" } else { "lower" }, node.op)).or_default() += started.elapsed();
        }
        ensure!(outs.len() >= node.outputs.iter().filter(|n| !n.is_empty()).count(), "{} lost outputs", node.label());
        for (name, v) in node.outputs.iter().zip(outs) {
            if !name.is_empty() {
                values.insert(name.clone(), v);
            }
        }
    }
    if verbose {
        let mut c: Vec<_> = costs.into_iter().collect();
        c.sort_by_key(|a| std::cmp::Reverse(a.1));
        for (op, t) in c.iter().take(8) {
            eprintln!("  plan {op}: {t:.2?}");
        }
    }
    p.label = "output".into();
    let outputs = outputs
        .iter()
        .map(|n| match values.get(n).with_context(|| format!("output {n} never computed"))? {
            v @ (Val::Lazy(_) | Val::Rows(_) | Val::Cat(_)) => Ok(Val::Gpu(p.gpu_val(v)?)),
            v => Ok(v.clone()),
        })
        .collect::<Result<_>>()?;
    Ok(Program { opset, nodes, bufs: p.bufs, textures: p.textures, kernels: p.kernels, steps, input, outputs })
}

/// A scalar constant as a WGSL expression.
fn literal(t: &Tensor) -> String {
    match &t.data {
        Data::F32(v) => format!("bitcast<f32>({}u)", v[0].to_bits()),
        Data::I64(v) => format!("i32({})", v[0].clamp(i64::from(i32::MIN) + 1, i64::from(i32::MAX))),
        Data::Bool(v) => format!("i32({})", i32::from(v[0])),
    }
}

/// `v` read as if broadcast to `shape`.
fn broadcast_view(v: &View, shape: &[usize]) -> Result<View> {
    ensure!(shape.len() >= v.shape.len(), "broadcasting {:?} to {shape:?}", v.shape);
    let pad = shape.len() - v.shape.len();
    let strides = (0..shape.len())
        .map(|i| {
            if i < pad {
                return Ok(0);
            }
            let d = v.shape[i - pad];
            if d == shape[i] {
                Ok(v.strides[i - pad])
            } else if d == 1 {
                Ok(0)
            } else {
                bail!("broadcasting {:?} to {shape:?}", v.shape)
            }
        })
        .collect::<Result<_>>()?;
    Ok(View { shape: shape.to_vec(), strides, ..v.clone() })
}

/// `v` reshaped without copying, when its strides allow it.
fn reshape_view(v: &View, shape: &[usize]) -> Option<View> {
    if numel(shape) != v.numel() || v.numel() == 0 {
        return None;
    }
    let old: Vec<(usize, isize)> = v.shape.iter().zip(&v.strides).filter(|(d, _)| **d != 1).map(|(&d, &s)| (d, s)).collect();
    let new: Vec<usize> = shape.iter().copied().filter(|&d| d != 1).collect();
    let mut st = vec![0isize; new.len()];
    let (mut oi, mut ni) = (0, 0);
    while oi < old.len() && ni < new.len() {
        let (mut oj, mut nj) = (oi + 1, ni + 1);
        let (mut op, mut np) = (old[oi].0, new[ni]);
        while op != np {
            if np < op {
                np *= *new.get(nj)?;
                nj += 1;
            } else {
                op *= old.get(oj)?.0;
                oj += 1;
            }
        }
        if (oi..oj - 1).any(|k| old[k].1 != old[k + 1].1 * old[k + 1].0 as isize) {
            return None;
        }
        st[nj - 1] = old[oj - 1].1;
        for k in (ni..nj - 1).rev() {
            st[k] = st[k + 1] * new[k + 1] as isize;
        }
        (oi, ni) = (oj, nj);
    }
    let mut it = st.into_iter();
    let strides = shape.iter().map(|&d| if d == 1 { 0 } else { it.next().unwrap() }).collect();
    Some(View { shape: shape.to_vec(), strides, ..v.clone() })
}

/// `template` with every operand name `aN` replaced by `args[N]`.
fn subst_args(template: &str, args: &[String]) -> String {
    let b = template.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'a' && (i == 0 || !word(b[i - 1])) {
            let j = i + 1 + b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            if j > i + 1 && (j == b.len() || !word(b[j])) {
                out += &args[template[i + 1..j].parse::<usize>().expect("operand index")];
                i = j;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// The address of a deferred value, which keys its materialized copy.
fn address(v: &Val) -> usize {
    match v {
        Val::Lazy(l) => Rc::as_ptr(l) as *const () as usize,
        Val::Rows(r) => Rc::as_ptr(r) as *const () as usize,
        Val::Cat(c) => Rc::as_ptr(c) as *const () as usize,
        _ => 0,
    }
}

/// Whether a row op result of `shape` is a whole `[nb, rows, cols]` MatMul
/// operand, so it can write the operand texture itself.
fn row_operand(shape: &[usize], nb: usize, rows: usize, cols: usize) -> bool {
    let r = shape.len();
    r >= 2 && shape[r - 1] == cols && shape[r - 2] == rows && numel(&shape[..r - 2]) == nb
}

/// The last two dims of `v`, for an operand whose batch dims are all 1.
fn last2(v: &View) -> View {
    let r = v.shape.len();
    View { shape: v.shape[r - 2..].to_vec(), strides: v.strides[r - 2..].to_vec(), ..v.clone() }
}

fn ints(v: &Option<Val>) -> Result<Vec<i64>> {
    match v {
        Some(Val::Const(t)) => t.ints(),
        Some(_) => bail!("a shape operand is computed on the GPU"),
        None => bail!("a shape operand is missing"),
    }
}

fn konst(v: &Option<Val>) -> Option<&Tensor> {
    match v {
        Some(Val::Const(t)) => Some(t),
        _ => None,
    }
}

impl Planner {
    fn alloc(&mut self, shape: &[usize], dtype: DType) -> View {
        let words = numel(shape).max(1);
        self.bufs.push(Buf { words, init: None });
        View::contiguous(self.bufs.len() - 1, words, shape, dtype)
    }

    fn upload(&mut self, t: &Rc<Tensor>) -> View {
        if let Some((_, v)) = self.uploaded.get(&Rc::as_ptr(t)) {
            return v.clone();
        }
        let words: Vec<u32> = match &t.data {
            Data::F32(v) => v.iter().map(|x| x.to_bits()).collect(),
            Data::I64(v) => v.iter().map(|&x| x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32 as u32).collect(),
            Data::Bool(v) => v.iter().map(|&x| u32::from(x)).collect(),
        };
        let n = words.len().max(1);
        self.bufs.push(Buf { words: n, init: Some(words) });
        let v = View::contiguous(self.bufs.len() - 1, n, &t.shape, t.dtype());
        self.uploaded.insert(Rc::as_ptr(t), (t.clone(), v.clone()));
        v
    }

    fn gpu(&mut self, v: &Option<Val>) -> Result<View> {
        match v {
            Some(v) => self.gpu_val(v),
            None => bail!("missing operand"),
        }
    }

    fn gpu_val(&mut self, v: &Val) -> Result<View> {
        match v {
            Val::Gpu(v) => Ok(v.clone()),
            Val::Const(t) => Ok(self.upload(t)),
            Val::Lazy(_) | Val::Rows(_) | Val::Cat(_) => self.materialize(v),
        }
    }

    fn pending<'l>(&self, v: &'l Val) -> Option<&'l Rc<Lazy>> {
        match v {
            Val::Lazy(l) if !self.materialized.contains_key(&address(v)) => Some(l),
            _ => None,
        }
    }

    /// A deferred value computed into memory, once.
    fn materialize(&mut self, v: &Val) -> Result<View> {
        if let Some((_, m)) = self.materialized.get(&address(v)) {
            return Ok(m.clone());
        }
        let out = self.alloc(v.shape(), v.dtype());
        match v {
            Val::Lazy(l) => self.store(&out, l.loads.clone(), &l.expr)?,
            Val::Rows(r) => {
                let label = self.label.clone();
                self.emit(kernels::rows(label, RowsOut::Buf(&out), &r.x, &r.op))?;
            }
            Val::Cat(c) => {
                let rank = c.shape.len();
                let mut start = vec![0; rank];
                for part in &c.parts {
                    let s = part.shape().to_vec();
                    let region = slice_view(&out, &start, &vec![1; rank], &s);
                    let (loads, expr) = self.operand(&Some(part.clone()), &s)?;
                    self.store(&region, loads, &expr)?;
                    start[c.axis] += s[c.axis];
                }
            }
            _ => bail!("materializing a value that is not deferred"),
        }
        self.materialized.insert(address(v), (v.clone(), out.clone()));
        Ok(out)
    }

    /// `x` as an expression of shape `shape` (same elements, row-major),
    /// for the row ops; copied first when its strides do not allow that.
    fn rows_source(&mut self, x: &Option<Val>, shape: &[usize]) -> Result<Lazy> {
        let xv = x.as_ref().context("row op of nothing")?;
        let src = self.source(xv)?;
        if let Some(loads) = src.loads.iter().map(|v| reshape_view(v, shape)).collect::<Option<Vec<_>>>() {
            return Ok(Lazy { shape: shape.to_vec(), loads, ..src });
        }
        let v = self.gpu(x)?;
        let v = self.contiguous(v)?;
        Ok(Lazy::of(&View { shape: shape.to_vec(), strides: signed(&strides(shape)), ..v }))
    }

    /// A row op, left for its consumer unless fusing is off.
    fn row_op(&mut self, x: Lazy, op: RowOp, shape: Vec<usize>) -> Result<Vec<Val>> {
        let v = Val::Rows(Rc::new(Rows { x, op, shape }));
        if self.opts.fuse {
            return Ok(vec![v]);
        }
        Ok(vec![Val::Gpu(self.materialize(&v)?)])
    }

    /// `out = expr(loads)`, every load a view of out's shape.
    fn store(&mut self, out: &View, loads: Vec<View>, expr: &str) -> Result<()> {
        self.map(out, &loads, &replace_loads(expr, |k| format!("a{k}")))
    }

    /// A value as an expression for a kernel that reads its operand
    /// elementwise (the packs).
    fn source(&mut self, v: &Val) -> Result<Lazy> {
        match self.pending(v) {
            Some(l) => Ok((**l).clone()),
            None => Ok(Lazy::of(&self.gpu_val(v)?)),
        }
    }

    /// An NCHW value as expressions whose channels follow one another: the
    /// parts of a channel concatenation not copied yet, else the value.
    fn channel_parts(&mut self, v: &Val) -> Result<Vec<Lazy>> {
        if let Val::Cat(c) = v
            && c.axis == 1
            && !self.materialized.contains_key(&address(v))
        {
            let parts = c.parts.iter().map(|p| self.source(p)).collect::<Result<Vec<_>>>()?;
            if parts.iter().map(|p| p.loads.len()).sum::<usize>() <= 8 {
                return Ok(parts);
            }
        }
        Ok(vec![self.source(v)?])
    }

    /// `v` broadcast to `shape`, as loads and an expression over them.
    fn operand(&mut self, v: &Option<Val>, shape: &[usize]) -> Result<(Vec<View>, String)> {
        match v {
            Some(Val::Const(t)) if t.numel() == 1 => Ok((vec![], literal(t))),
            Some(x) => match self.pending(x) {
                Some(l) => Ok((l.loads.iter().map(|v| broadcast_view(v, shape)).collect::<Result<_>>()?, l.expr.clone())),
                None => Ok((vec![broadcast_view(&self.gpu_val(x)?, shape)?], "$0$".into())),
            },
            None => bail!("missing operand"),
        }
    }

    /// A view op (transpose, slice, broadcast...) applied to a value; on a
    /// lazy value it applies to every load.
    fn view(&mut self, x: &Option<Val>, f: impl Fn(&View) -> Result<View>) -> Result<Val> {
        let x = x.as_ref().context("missing operand")?;
        if let Some(l) = self.pending(x) {
            let shape = f(&View::contiguous(usize::MAX, numel(&l.shape).max(1), &l.shape, l.dtype))?.shape;
            let loads = l.loads.iter().map(&f).collect::<Result<_>>()?;
            return Ok(Val::Lazy(Rc::new(Lazy { shape, dtype: l.dtype, loads, expr: l.expr.clone() })));
        }
        Ok(Val::Gpu(f(&self.gpu_val(x)?)?))
    }

    fn fits(&self, d: &TexDesc) -> bool {
        d.width <= self.opts.max_tex_dim && d.height <= self.opts.max_tex_dim && d.layers <= self.opts.max_tex_layers
    }

    fn texture(&mut self, desc: TexDesc, init: Option<Vec<u8>>) -> usize {
        self.textures.push(Tex { desc, init });
        self.textures.len() - 1
    }

    /// A weight texture made once per tensor and layout.
    fn weight_texture(
        &mut self,
        t: &Rc<Tensor>,
        layout: &'static str,
        make: impl FnOnce(&[f32]) -> (TexDesc, Vec<u8>),
    ) -> Result<(usize, TexDesc)> {
        let key = (Rc::as_ptr(t), layout);
        if let Some(&(_, id, desc)) = self.packed.get(&key) {
            return Ok((id, desc));
        }
        let (desc, bytes) = make(t.as_f32()?);
        let id = self.texture(desc, Some(bytes));
        self.packed.insert(key, (t.clone(), id, desc));
        Ok((id, desc))
    }

    /// An operand `[batch..., rows, cols]` (broadcast to `batch`) as a row
    /// texture of `per` rows per batch, vec4 over cols.
    fn rows_texture(
        &mut self,
        v: &Val,
        batch: &[usize],
        rows: usize,
        per: usize,
        cols: usize,
        format: TexFormat,
    ) -> Result<Option<(usize, TexDesc)>> {
        let nb = numel(batch);
        let desc = tex::row_desc(nb * per, cols.div_ceil(4), format);
        if !self.fits(&desc) {
            return Ok(None);
        }
        match v {
            Val::Const(t) => {
                let own = numel(&t.shape[..t.shape.len() - 2]);
                if own != nb && own != 1 {
                    return Ok(None);
                }
                let layout = if per == rows { "rows" } else { "rows4" };
                if nb != 1 && own == 1 {
                    // Rare: a shared constant operand of a batched product.
                    let data = t.as_f32()?.repeat(nb);
                    let (desc, bytes) = tex::rows_bytes(&data, nb, rows, per, cols, format);
                    return Ok(Some((self.texture(desc, Some(bytes)), desc)));
                }
                Ok(Some(self.weight_texture(t, layout, |d| tex::rows_bytes(d, nb, rows, per, cols, format))?))
            }
            Val::Rows(r) if per == rows && !self.materialized.contains_key(&address(v)) && row_operand(&r.shape, nb, rows, cols) => {
                let id = self.texture(desc, None);
                let label = self.label.clone();
                self.emit(kernels::rows(label, RowsOut::Tex(id, &desc), &r.x, &r.op))?;
                Ok(Some((id, desc)))
            }
            _ => {
                let src = self.source(v)?;
                let shape = [batch, &[rows, cols][..]].concat();
                let loads = src.loads.iter().map(|l| broadcast_view(l, &shape)).collect::<Result<_>>()?;
                let src = Lazy { shape, loads, ..src };
                let id = self.texture(desc, None);
                let label = self.label.clone();
                self.emit(tex::pack_rows(label, &src, per, id, &desc))?;
                Ok(Some((id, desc)))
            }
        }
    }

    /// MatMul through textures, or None when it does not fit them.
    fn matmul_tex(&mut self, a: &Val, b: &Val, format: TexFormat) -> Result<Option<View>> {
        let (ra, rb) = (a.shape().len(), b.shape().len());
        let (m, k, n) = (a.shape()[ra - 2], a.shape()[ra - 1], b.shape()[rb - 1]);
        let batch = broadcast_shape(&[&a.shape()[..ra - 2], &b.shape()[..rb - 2]])?;
        let nb = numel(&batch);
        let k4 = k.div_ceil(4);
        let shared_b = numel(&b.shape()[..rb - 2]) == 1;
        let Some(ta) = self.rows_texture(a, &batch, m, m, k, format)? else { return Ok(None) };
        let bbatch: &[usize] = if shared_b { &[] } else { &batch };
        let bv = if shared_b {
            match b {
                Val::Const(_) => b.clone(),
                _ => self.view(&Some(b.clone()), |v| Ok(last2(v)))?,
            }
        } else {
            b.clone()
        };
        let Some(tb) = self.rows_texture(&bv, bbatch, k, 4 * k4, n, format)? else { return Ok(None) };
        let out = self.alloc(&[&batch[..], &[m, n]].concat(), DType::F32);
        let (rows, z) = if shared_b { (nb * m, 1) } else { (m, nb) };
        let label = self.label.clone();
        self.emit(tex::matmul(label, &out, (ta.0, &ta.1), (tb.0, &tb.1), rows, n, k4, z, None))?;
        Ok(Some(out))
    }

    fn emit(&mut self, k: Result<Kernel>) -> Result<()> {
        let k = k?;
        match self.opts.budget {
            Some(b) => self.kernels.extend(k.chunks(b)),
            None => self.kernels.push(k),
        }
        Ok(())
    }

    fn map(&mut self, out: &View, loads: &[View], expr: &str) -> Result<()> {
        if out.numel() > 0 {
            let label = self.label.clone();
            self.emit(kernels::map(label, out, loads, expr))?;
        }
        Ok(())
    }

    fn contiguous(&mut self, v: View) -> Result<View> {
        if v.is_contiguous() {
            return Ok(v);
        }
        let out = self.alloc(&v.shape, v.dtype);
        self.map(&out, &[v], "a0")?;
        Ok(out)
    }

    fn reshape(&mut self, v: View, shape: &[usize]) -> Result<View> {
        if let Some(r) = reshape_view(&v, shape) {
            return Ok(r);
        }
        let v = self.contiguous(v)?;
        Ok(View { shape: shape.to_vec(), strides: signed(&strides(shape)), ..v })
    }

    /// An elementwise op: `expr` over the broadcast operands, left lazy
    /// for its consumer unless fusing is off or the expression grows large.
    fn elementwise(&mut self, ins: &[&Option<Val>], dtype: DType, expr: &str) -> Result<Vec<Val>> {
        let shapes: Vec<&[usize]> = ins.iter().map(|v| v.as_ref().map_or(&[][..], Val::shape)).collect();
        let shape = broadcast_shape(&shapes)?;
        let mut loads: Vec<View> = Vec::new();
        let mut parts = Vec::new();
        for v in ins {
            let (ls, e) = self.operand(v, &shape)?;
            let mut at = Vec::new();
            for l in ls {
                at.push(loads.iter().position(|x| *x == l).unwrap_or_else(|| {
                    loads.push(l);
                    loads.len() - 1
                }));
            }
            parts.push(format!("({})", replace_loads(&e, |k| format!("${}$", at[k]))));
        }
        let lazy = Lazy { shape, dtype, loads, expr: subst_args(expr, &parts) };
        if self.opts.fuse && lazy.loads.len() <= 8 && lazy.expr.len() <= 2000 {
            return Ok(vec![Val::Lazy(Rc::new(lazy))]);
        }
        let out = self.alloc(&lazy.shape, dtype);
        if out.numel() > 0 {
            self.store(&out, lazy.loads, &lazy.expr)?;
        }
        Ok(vec![Val::Gpu(out)])
    }

    fn lower(&mut self, node: &Node, ins: &[Option<Val>], opset: i64) -> Result<Vec<Val>> {
        let x = ins.first().context("no inputs")?;
        let dtype = x.as_ref().map_or(DType::F32, Val::dtype);
        let zero = if dtype == DType::F32 { "0.0" } else { "0" };
        let gpu = |v: View| Ok(vec![Val::Gpu(v)]);
        match node.op.as_str() {
            "Identity" => Ok(vec![x.clone().context("Identity of nothing")?]),
            "Reshape" => {
                let xs = x.as_ref().context("Reshape of nothing")?;
                let shape = reference::reshape_shape(xs.shape(), &ints(&ins[1])?, node.int("allowzero", 0))?;
                if let Some(l) = self.pending(xs)
                    && let Some(loads) = l.loads.iter().map(|v| reshape_view(v, &shape)).collect::<Option<Vec<_>>>()
                {
                    return Ok(vec![Val::Lazy(Rc::new(Lazy { shape, dtype: l.dtype, loads, expr: l.expr.clone() }))]);
                }
                let v = self.gpu(x)?;
                gpu(self.reshape(v, &shape)?)
            }
            "Unsqueeze" | "Squeeze" => {
                let xs = x.as_ref().context("Squeeze of nothing")?.shape().to_vec();
                let axes = reference::axes(node, konst(ins.get(1).unwrap_or(&None)))?;
                let shape = if node.op == "Unsqueeze" {
                    reference::unsqueeze_shape(&xs, &axes.context("Unsqueeze without axes")?)?
                } else {
                    reference::squeeze_shape(&xs, axes.as_deref())?
                };
                // Size-1 dims carry no stride, so the remaining ones keep theirs.
                Ok(vec![self.view(x, |v| {
                    let mut kept = v.shape.iter().zip(&v.strides).filter(|(d, _)| **d != 1).map(|(_, s)| *s);
                    let strides = shape.iter().map(|&d| if d == 1 { 0 } else { kept.next().unwrap() }).collect();
                    Ok(View { shape: shape.clone(), strides, ..v.clone() })
                })?])
            }
            "Transpose" => {
                let rank = x.as_ref().context("Transpose of nothing")?.shape().len();
                let perm = reference::perm(node, rank)?;
                Ok(vec![self.view(x, |v| {
                    Ok(View {
                        shape: perm.iter().map(|&p| v.shape[p]).collect(),
                        strides: perm.iter().map(|&p| v.strides[p]).collect(),
                        ..v.clone()
                    })
                })?])
            }
            "Slice" => {
                let xs = x.as_ref().context("Slice of nothing")?.shape().to_vec();
                let opt = |i: usize| ins.get(i).filter(|v| v.is_some()).map(ints).transpose();
                let spec = reference::slice_spec(&xs, &ints(&ins[1])?, &ints(&ins[2])?, opt(3)?, opt(4)?)?;
                Ok(vec![self.view(x, |v| Ok(slice_view(v, &spec.start, &spec.step, &spec.count)))?])
            }
            "Split" => {
                let xs = x.as_ref().context("Split of nothing")?.shape().to_vec();
                let ax = reference::axis(node.int("axis", 0), xs.len())?;
                let sizes = reference::split_sizes(node, xs[ax], konst(ins.get(1).unwrap_or(&None)), opset)?;
                let mut start = vec![0; xs.len()];
                let mut outs = Vec::new();
                for n in sizes {
                    let mut count = xs.clone();
                    count[ax] = n;
                    outs.push(self.view(x, |v| Ok(slice_view(v, &start, &vec![1; xs.len()], &count)))?);
                    start[ax] += n;
                }
                Ok(outs)
            }
            "Expand" => {
                let xs = x.as_ref().context("Expand of nothing")?.shape().to_vec();
                let target: Vec<usize> = ints(&ins[1])?.iter().map(|&d| d as usize).collect();
                let shape = broadcast_shape(&[&xs, &target])?;
                Ok(vec![self.view(x, |v| broadcast_view(v, &shape))?])
            }
            "Gather" => {
                let xs = x.as_ref().context("Gather of nothing")?.shape().to_vec();
                let ax = reference::axis(node.int("axis", 0), xs.len())?;
                if let Some(t) = konst(&ins[1]).filter(|t| t.shape.is_empty()) {
                    let n = xs[ax] as i64;
                    let i = t.ints()?[0];
                    let i = if i < 0 { i + n } else { i };
                    ensure!((0..n).contains(&i), "Gather index {i} outside {n}");
                    let mut start = vec![0; xs.len()];
                    start[ax] = i as usize;
                    let mut count = xs.clone();
                    count[ax] = 1;
                    return Ok(vec![self.view(x, |v| {
                        let s = slice_view(v, &start, &vec![1; xs.len()], &count);
                        let (mut shape, mut strides) = (s.shape.clone(), s.strides.clone());
                        shape.remove(ax);
                        strides.remove(ax);
                        Ok(View { shape, strides, ..s })
                    })?]);
                }
                let v = self.gpu(x)?;
                let data = self.contiguous(v)?;
                let ind = self.gpu(&ins[1])?;
                let ind = self.contiguous(ind)?;
                let shape = [&data.shape[..ax], &ind.shape[..], &data.shape[ax + 1..]].concat();
                let out = self.alloc(&shape, data.dtype);
                let label = self.label.clone();
                self.emit(kernels::gather(label, &out, &data, &ind, ax))?;
                gpu(out)
            }
            "GatherND" => {
                let data = self.gpu(x)?;
                let data = self.contiguous(data)?;
                let ind = self.gpu(&ins[1])?;
                let ind = self.contiguous(ind)?;
                let b = node.int("batch_dims", 0) as usize;
                let q = ind.shape.len();
                let k = ind.shape[q - 1];
                let shape = [&ind.shape[..q - 1], &data.shape[b + k..]].concat();
                let out = self.alloc(&shape, data.dtype);
                let label = self.label.clone();
                self.emit(kernels::gather_nd(label, &out, &data, &ind, b))?;
                gpu(out)
            }
            "Concat" => {
                let shapes: Vec<&[usize]> = ins.iter().map(|v| v.as_ref().map_or(&[][..], Val::shape)).collect();
                let ax = reference::axis(node.int("axis", 0), shapes[0].len())?;
                let mut shape = shapes[0].to_vec();
                shape[ax] = shapes.iter().map(|s| s[ax]).sum();
                let parts: Vec<Val> = ins
                    .iter()
                    .zip(&shapes)
                    .filter(|(_, s)| s[ax] > 0)
                    .map(|(v, _)| v.clone().context("Concat of nothing"))
                    .collect::<Result<_>>()?;
                let v = Val::Cat(Rc::new(Cat { axis: ax, parts, shape, dtype }));
                if self.opts.fuse {
                    return Ok(vec![v]);
                }
                Ok(vec![Val::Gpu(self.materialize(&v)?)])
            }
            "Add" | "Sub" | "Mul" | "Div" => {
                let sym = match node.op.as_str() {
                    "Add" => "+",
                    "Sub" => "-",
                    "Mul" => "*",
                    _ => "/",
                };
                self.elementwise(&[x, &ins[1]], dtype, &format!("a0 {sym} a1"))
            }
            "Sum" => {
                let refs: Vec<&Option<Val>> = ins.iter().collect();
                let expr = (0..ins.len()).map(|i| format!("a{i}")).collect::<Vec<_>>().join(" + ");
                self.elementwise(&refs, dtype, &expr)
            }
            "Pow" => {
                let e = konst(&ins[1]).filter(|t| t.numel() == 1).map(|t| t.to_f32()[0]);
                let expr = match e {
                    Some(1.0) => "a0",
                    Some(2.0) => "a0 * a0",
                    Some(3.0) => "a0 * a0 * a0",
                    Some(0.5) => "sqrt(a0)",
                    _ => "pow(a0, f32(a1))",
                };
                self.elementwise(&[x, &ins[1]], DType::F32, expr)
            }
            "Mod" => {
                let expr = if node.int("fmod", 0) == 1 {
                    if dtype == DType::F32 { "a0 - a1 * trunc(a0 / a1)" } else { "a0 % a1" }
                } else {
                    "select(a0 % a1, a0 % a1 + a1, (a0 % a1 != 0) && ((a0 % a1 < 0) != (a1 < 0)))"
                };
                self.elementwise(&[x, &ins[1]], dtype, expr)
            }
            "Equal" => self.elementwise(&[x, &ins[1]], DType::Bool, "select(0, 1, a0 == a1)"),
            "Where" => {
                let dt = ins[1].as_ref().context("Where without x")?.dtype();
                self.elementwise(&[x, &ins[1], &ins[2]], dt, "select(a2, a1, a0 != 0)")
            }
            "Not" => self.elementwise(&[x], DType::Bool, "1 - a0"),
            "Sqrt" => self.elementwise(&[x], DType::F32, "sqrt(a0)"),
            "Erf" => self.elementwise(&[x], DType::F32, "erf_(a0)"),
            "Relu" => self.elementwise(&[x], dtype, &format!("max(a0, {zero})")),
            "Sigmoid" => self.elementwise(&[x], DType::F32, "1.0 / (1.0 + exp(-a0))"),
            "Floor" => self.elementwise(&[x], DType::F32, "floor(a0)"),
            "Cast" => {
                let (to, expr) = match (node.int("to", 0), dtype) {
                    (1, DType::F32) => (DType::F32, "a0"),
                    (1, _) => (DType::F32, "f32(a0)"),
                    (6 | 7, DType::F32) => (DType::I64, "i32(a0)"),
                    (6 | 7, _) => (DType::I64, "a0"),
                    (9, DType::F32) => (DType::Bool, "select(0, 1, a0 != 0.0)"),
                    (9, _) => (DType::Bool, "select(0, 1, a0 != 0)"),
                    (to, _) => bail!("Cast to {to}"),
                };
                self.elementwise(&[x], to, expr)
            }
            "Clip" => {
                let bound = |i: usize| -> Result<Option<String>> {
                    match ins.get(i) {
                        None | Some(None) => Ok(None),
                        Some(Some(Val::Const(t))) if t.numel() == 1 => Ok(Some(literal(t))),
                        _ => bail!("Clip bounds computed on the GPU"),
                    }
                };
                let mut expr = "a0".to_string();
                if let Some(lo) = bound(1)? {
                    expr = format!("max({expr}, {lo})");
                }
                if let Some(hi) = bound(2)? {
                    expr = format!("min({expr}, {hi})");
                }
                self.elementwise(&[x], dtype, &expr)
            }
            "MatMul" if self.opts.tex.is_some() && x.as_ref().is_some_and(|v| v.shape().len() >= 2) => {
                let a = x.clone().context("MatMul without A")?;
                let b = ins[1].clone().context("MatMul without B")?;
                let format = self.opts.tex.unwrap();
                if let Some(out) = self.matmul_tex(&a, &b, format)? {
                    return gpu(out);
                }
                let a = self.gpu(x)?;
                let b = self.gpu(&ins[1])?;
                let (ra, rb) = (a.shape.len(), b.shape.len());
                let batch = broadcast_shape(&[&a.shape[..ra - 2], &b.shape[..rb - 2]])?;
                let out = self.alloc(&[&batch[..], &[a.shape[ra - 2], b.shape[rb - 1]]].concat(), DType::F32);
                let label = self.label.clone();
                self.emit(kernels::matmul(label, &out, &a, &b, &Epilogue::NONE))?;
                gpu(out)
            }
            "MatMul" => {
                let a = self.gpu(x)?;
                let b = self.gpu(&ins[1])?;
                let (ra, rb) = (a.shape.len(), b.shape.len());
                ensure!(ra >= 2 && rb >= 2, "MatMul of {:?} and {:?}", a.shape, b.shape);
                let batch = broadcast_shape(&[&a.shape[..ra - 2], &b.shape[..rb - 2]])?;
                let out = self.alloc(&[&batch[..], &[a.shape[ra - 2], b.shape[rb - 1]]].concat(), DType::F32);
                let label = self.label.clone();
                self.emit(kernels::matmul(label, &out, &a, &b, &Epilogue::NONE))?;
                gpu(out)
            }
            "Conv" | "ConvTranspose" => {
                let xval = x.as_ref().context("Conv of nothing")?;
                let xs = xval.shape().to_vec();
                let ws = ins[1].as_ref().context("Conv without weights")?.shape().to_vec();
                let bias = ins.get(2).filter(|v| v.is_some()).map(|v| self.gpu(v)).transpose()?;
                let g = reference::conv_geometry(node)?;
                ensure!(xs.len() == 4 && ws.len() == 4, "only 2-D convolutions");
                let (kh, kw) = (ws[2], ws[3]);
                let (h, wd) = (xs[2], xs[3]);
                let transposed = node.op == "ConvTranspose";
                let co = if transposed { ws[1] * g.group } else { ws[0] };
                let (oh, ow) = if transposed {
                    ensure!(node.ints("output_padding").is_none(), "ConvTranspose output_padding");
                    (
                        (h - 1) * g.stride[0] + g.dilation[0] * (kh - 1) + 1 - g.pads[0] - g.pads[2],
                        (wd - 1) * g.stride[1] + g.dilation[1] * (kw - 1) + 1 - g.pads[1] - g.pads[3],
                    )
                } else {
                    (
                        reference::conv_out(h, kh, g.stride[0], g.pads[0], g.pads[2], g.dilation[0]),
                        reference::conv_out(wd, kw, g.stride[1], g.pads[1], g.pads[3], g.dilation[1]),
                    )
                };
                let shape = ConvShape {
                    n: xs[0],
                    c: xs[1],
                    h,
                    w: wd,
                    co,
                    kh,
                    kw,
                    oh,
                    ow,
                    stride: g.stride,
                    pads: g.pads,
                    dilation: g.dilation,
                    group: g.group,
                };
                let out = self.alloc(&[shape.n, co, oh, ow], DType::F32);
                if let (Some(format), false, 1, Some(Val::Const(wt))) = (self.opts.tex, transposed, g.group, &ins[1]) {
                    let border = tex::Border { top: g.pads[0], left: g.pads[1], bottom: g.pads[2], right: g.pads[3] };
                    let xd = tex::channel_desc(shape.n, shape.c, h, wd, border, format);
                    let (c, taps) = (shape.c, kh * kw);
                    let probe = tex::row_desc(taps * c.div_ceil(4) * 4, co.div_ceil(4), format);
                    if self.fits(&xd) && self.fits(&probe) {
                        let wt = wt.clone();
                        let (wid, wdesc) = self.weight_texture(&wt, "conv", |d| tex::conv_weight_bytes(d, co, c, taps, true, format))?;
                        let xid = self.texture(xd, None);
                        let label = self.label.clone();
                        let parts = self.channel_parts(xval)?;
                        self.emit(tex::pack_channels(label.clone(), &parts, border, xid, &xd))?;
                        let geom = tex::ConvGeom { n: shape.n, c, co, kh, kw, oh, ow, stride: g.stride, dilation: g.dilation };
                        self.emit(tex::conv(label, &out, (xid, &xd), (wid, &wdesc), bias.as_ref(), &geom))?;
                        return gpu(out);
                    }
                }
                // A transposed convolution whose taps never overlap is a GEMM
                // of pixels by input channels and weights [C, CO * KH * KW],
                // each pixel's row then scattered into its KH x KW block.
                if let (Some(format), true, 1, Some(Val::Const(wt))) = (self.opts.tex, transposed, g.group, &ins[1])
                    && g.stride == [kh, kw]
                    && g.dilation == [1, 1]
                    && g.pads.iter().all(|&p| p == 0)
                    && shape.n == 1
                {
                    let (c, hw, n) = (shape.c, h * wd, co * kh * kw);
                    let v = self.gpu(x)?;
                    let xv = self.contiguous(v)?;
                    let a = View { shape: vec![hw, c], strides: vec![1, hw as isize], ..xv };
                    let b = Val::Const(Rc::new(wt.reshaped(vec![c, n])));
                    if let Some(rows) = self.matmul_tex(&Val::Gpu(a), &b, format)? {
                        let blocks = [co, h, kh, wd, kw];
                        let src = View { shape: blocks.to_vec(), strides: signed(&[kh * kw, wd * n, kw, n, 1]), ..rows };
                        let dst = View { shape: blocks.to_vec(), strides: signed(&[oh * ow, kh * ow, ow, kw, 1]), ..out.clone() };
                        let mut loads = vec![src];
                        let mut expr = "$0$".to_string();
                        if let Some(b) = &bias {
                            loads.push(View { shape: blocks.to_vec(), strides: vec![b.strides[0], 0, 0, 0, 0], ..b.clone() });
                            expr += " + $1$";
                        }
                        self.store(&dst, loads, &expr)?;
                        return gpu(out);
                    }
                }
                let v = self.gpu(x)?;
                let xv = self.contiguous(v)?;
                let w = self.gpu(&ins[1])?;
                let label = self.label.clone();
                if transposed {
                    self.emit(kernels::conv_transpose(label, &out, &xv, &w, bias.as_ref(), &shape))?;
                } else {
                    self.emit(kernels::conv(label, &out, &xv, &w, &Epilogue { bias, act: None }, &shape))?;
                }
                gpu(out)
            }
            "DeformConv" => {
                let xval = x.as_ref().context("DeformConv of nothing")?;
                let xs = xval.shape().to_vec();
                let ws = ins[3].as_ref().context("DeformConv without weights")?.shape().to_vec();
                let v = self.gpu(&ins[1])?;
                let pos = self.contiguous(v)?;
                let v = self.gpu(&ins[2])?;
                let mask = self.contiguous(v)?;
                let bias = ins.get(4).filter(|v| v.is_some()).map(|v| self.gpu(v)).transpose()?;
                let pair = |name: &str| -> Result<[i64; 2]> {
                    match node.ints(name) {
                        Some(&[a, b]) => Ok([a, b]),
                        _ => bail!("DeformConv {name}"),
                    }
                };
                ensure!(xs.len() == 4 && xs[0] == 1 && ws.len() == 4, "DeformConv of {xs:?}");
                let r = pos.shape.len();
                let shape = kernels::DeformShape {
                    c: xs[1],
                    h: xs[2],
                    w: xs[3],
                    co: ws[0],
                    k: node.int("kernel", 1) as usize,
                    oh: pos.shape[r - 2],
                    ow: pos.shape[r - 1],
                    pads: pair("pads")?,
                    bounds: pair("bounds")?,
                };
                let out = self.alloc(&[1, shape.co, shape.oh, shape.ow], DType::F32);
                if let (Some(format), Some(Val::Const(wt))) = (self.opts.tex, &ins[3]) {
                    let (c, taps) = (shape.c, shape.k * shape.k);
                    let pads = [shape.pads[0] as usize, shape.pads[1] as usize];
                    let bottom = shape.bounds[0] + 1 - shape.h as i64 - shape.pads[0];
                    let right = shape.bounds[1] + 1 - shape.w as i64 - shape.pads[1];
                    ensure!(bottom >= 0 && right >= 0, "DeformConv bounds {:?} inside the image", shape.bounds);
                    let border = tex::Border { top: pads[0], left: pads[1], bottom: bottom as usize, right: right as usize };
                    let xd = tex::channel_desc(1, c, shape.h, shape.w, border, format);
                    let ds = tex::DeformSample { c, taps, oh: shape.oh, ow: shape.ow, bounds: shape.bounds };
                    let sd = tex::deform_desc(&ds, format);
                    let probe = tex::row_desc(taps * c.div_ceil(4) * 4, shape.co.div_ceil(4), format);
                    if self.fits(&xd) && self.fits(&sd) && self.fits(&probe) {
                        let wt = wt.clone();
                        let co = shape.co;
                        let (wid, wdesc) = self.weight_texture(&wt, "deform", |d| tex::conv_weight_bytes(d, co, c, taps, false, format))?;
                        let xid = self.texture(xd, None);
                        let sid = self.texture(sd, None);
                        let label = self.label.clone();
                        let parts = self.channel_parts(xval)?;
                        self.emit(tex::pack_channels(label.clone(), &parts, border, xid, &xd))?;
                        self.emit(tex::deform_sample(label.clone(), &pos, &mask, (xid, &xd), (sid, &sd), &ds))?;
                        self.emit(tex::deform_gemm(label, &out, (sid, &sd), (wid, &wdesc), bias.as_ref(), &ds, co))?;
                        return gpu(out);
                    }
                }
                let v = self.gpu(x)?;
                let xv = self.contiguous(v)?;
                let w = self.gpu(&ins[3])?;
                let label = self.label.clone();
                self.emit(kernels::deform_conv(label, &out, &xv, &pos, &mask, &w, &Epilogue { bias, act: None }, &shape))?;
                gpu(out)
            }
            "LayerNormalization" => {
                let xs = x.as_ref().context("LayerNormalization of nothing")?.shape().to_vec();
                let ax = reference::axis(node.int("axis", -1), xs.len())?;
                let cols = numel(&xs[ax..]);
                let scale = ins.get(1).filter(|v| v.is_some()).map(|v| self.gpu(v)).transpose()?;
                let bias = ins.get(2).filter(|v| v.is_some()).map(|v| self.gpu(v)).transpose()?;
                let src = self.rows_source(x, &[&xs[..ax], &[cols]].concat())?;
                self.row_op(src, RowOp::LayerNorm(scale, bias, node.float("epsilon", 1e-5)), xs)
            }
            "Softmax" => {
                let xs = x.as_ref().context("Softmax of nothing")?.shape().to_vec();
                ensure!(reference::axis(node.int("axis", -1), xs.len())? == xs.len() - 1, "Softmax along an inner axis");
                let src = self.rows_source(x, &xs)?;
                self.row_op(src, RowOp::Softmax, xs)
            }
            "ReduceMean" | "GlobalAveragePool" => {
                let xs = x.as_ref().context("ReduceMean of nothing")?.shape().to_vec();
                let rank = xs.len();
                let (first, keep) = if node.op == "GlobalAveragePool" {
                    (2, true)
                } else {
                    let axes = reference::axes(node, konst(ins.get(1).unwrap_or(&None)))?.context("ReduceMean over everything")?;
                    let mut ax: Vec<usize> = axes.iter().map(|&a| reference::axis(a, rank)).collect::<Result<_>>()?;
                    ax.sort_unstable();
                    ensure!(ax.iter().enumerate().all(|(i, &a)| a == rank - ax.len() + i), "ReduceMean over inner axes {ax:?}");
                    (rank - ax.len(), node.int("keepdims", 1) != 0)
                };
                let src = self.rows_source(x, &[&xs[..first], &[numel(&xs[first..])]].concat())?;
                let shape: Vec<usize> =
                    if keep { (0..rank).map(|d| if d < first { xs[d] } else { 1 }).collect() } else { xs[..first].to_vec() };
                let out = self.alloc(&shape, DType::F32);
                let label = self.label.clone();
                self.emit(kernels::rows(label, RowsOut::Buf(&out), &src, &RowOp::Mean))?;
                gpu(out)
            }
            "Resize" => {
                let v = self.gpu(x)?;
                let xv = self.contiguous(v)?;
                ensure!(node.string("mode", "nearest") == "linear", "Resize mode {} on the GPU", node.string("mode", "nearest"));
                let (shape, sc) =
                    reference::resize_plan(&xv.shape, konst(ins.get(2).unwrap_or(&None)), konst(ins.get(3).unwrap_or(&None)))?;
                ensure!(shape.len() == 4 && shape[..2] == xv.shape[..2], "Resize of {:?} to {shape:?}", xv.shape);
                let out = self.alloc(&shape, DType::F32);
                let label = self.label.clone();
                let ctm = node.string("coordinate_transformation_mode", "half_pixel");
                self.emit(kernels::resize_linear(label, &out, &xv, &ctm, [sc[2], sc[3]]))?;
                gpu(out)
            }
            "Pad" => {
                let v = self.gpu(x)?;
                let amounts = reference::pad_amounts(node, v.shape.len(), konst(&ins[1]), konst(ins.get(3).unwrap_or(&None)))?;
                let value = match ins.get(2) {
                    Some(Some(Val::Const(t))) => literal(t),
                    None | Some(None) => format!("{}({zero})", kernels::wgsl_type(dtype)),
                    _ => bail!("Pad value computed on the GPU"),
                };
                let shape: Vec<usize> = v.shape.iter().zip(&amounts).map(|(&d, &(b, e))| (d as isize + b + e) as usize).collect();
                let out = self.alloc(&shape, dtype);
                let before: Vec<isize> = amounts.iter().map(|a| a.0).collect();
                let label = self.label.clone();
                self.emit(kernels::pad(label, &out, &v, &before, &value))?;
                gpu(out)
            }
            "BatchNormalization" => {
                let c = |i: usize| konst(&ins[i]).context("BatchNormalization statistics computed on the GPU");
                let (scale, shift) = reference::batch_norm_affine(node, c(1)?, c(2)?, c(3)?, c(4)?)?;
                let rank = x.as_ref().context("BatchNormalization of nothing")?.shape().len();
                let mut shape = vec![1; rank - 1];
                shape[0] = scale.numel();
                let s = Some(Val::Const(Rc::new(scale.reshaped(shape.clone()))));
                let t = Some(Val::Const(Rc::new(shift.reshaped(shape))));
                self.elementwise(&[x, &s, &t], DType::F32, "a0 * a1 + a2")
            }
            op => bail!("no GPU lowering for {op}"),
        }
    }
}

fn slice_view(v: &View, start: &[usize], step: &[isize], count: &[usize]) -> View {
    let offset = v.offset as isize + start.iter().zip(&v.strides).map(|(&s, &st)| s as isize * st).sum::<isize>();
    View {
        offset: offset as usize,
        shape: count.to_vec(),
        strides: v.strides.iter().zip(step).map(|(&s, &k)| s * k).collect(),
        ..v.clone()
    }
}
