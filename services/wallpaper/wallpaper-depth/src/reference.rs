//! CPU reference ops. They fold the shape graph at load time and are the
//! ground truth every GPU kernel is checked against. Plain loops, written
//! for clarity first; the heavy ones spread over the worker threads.

use anyhow::{Context, Result, bail, ensure};

use crate::each;
use crate::onnx::{Attr, Node};
use crate::tensor::{DType, Data, Tensor, broadcast_shape, broadcast_strides, numel, offsets, par_rows, strides, walk};

pub fn axis(a: i64, rank: usize) -> Result<usize> {
    let r = rank as i64;
    ensure!((-r..r).contains(&a), "axis {a} outside rank {rank}");
    Ok(if a < 0 { a + r } else { a } as usize)
}

pub fn eval(node: &Node, ins: &[Option<&Tensor>], opset: i64) -> Result<Vec<Tensor>> {
    let x = |i: usize| -> Result<&Tensor> { ins.get(i).copied().flatten().with_context(|| format!("input {i} is missing")) };
    let opt = |i: usize| ins.get(i).copied().flatten();
    let one = |t: Tensor| Ok(vec![t]);
    match node.op.as_str() {
        "Constant" => one(constant(node)?),
        "Identity" => one(x(0)?.clone()),
        "Shape" => one(shape_of(node, &x(0)?.shape)),
        "Unsqueeze" => {
            let t = x(0)?;
            one(t.reshaped(unsqueeze_shape(&t.shape, &axes(node, opt(1))?.context("no axes")?)?))
        }
        "Squeeze" => {
            let t = x(0)?;
            one(t.reshaped(squeeze_shape(&t.shape, axes(node, opt(1))?.as_deref())?))
        }
        "Reshape" => {
            let t = x(0)?;
            one(t.reshaped(reshape_shape(&t.shape, &x(1)?.ints()?, node.int("allowzero", 0))?))
        }
        "Concat" => {
            let ts: Vec<&Tensor> = ins.iter().map(|t| t.context("missing concat input")).collect::<Result<_>>()?;
            one(concat(&ts, node.int("axis", 0))?)
        }
        "Add" | "Sub" | "Mul" | "Div" | "Pow" | "Mod" | "Equal" => one(binary(&node.op, x(0)?, x(1)?, node.int("fmod", 0))?),
        "Sum" => {
            let mut acc = x(0)?.clone();
            for i in 1..ins.len() {
                acc = binary("Add", &acc, x(i)?, 0)?;
            }
            one(acc)
        }
        "Where" => one(where_(x(0)?, x(1)?, x(2)?)?),
        "Not" => {
            let t = x(0)?;
            one(Tensor::bool(t.shape.clone(), t.as_bool()?.iter().map(|&v| !v).collect()))
        }
        "Floor" => one(unary(x(0)?, f32::floor)?),
        "Sqrt" => one(unary(x(0)?, f32::sqrt)?),
        "Relu" => one(unary(x(0)?, |v| v.max(0.0))?),
        "Sigmoid" => one(unary(x(0)?, |v| (1.0 / (1.0 + (-f64::from(v)).exp())) as f32)?),
        "Erf" => one(unary(x(0)?, |v| erf(f64::from(v)) as f32)?),
        "Cast" => one(cast(x(0)?, node.int("to", 0))?),
        "Clip" => one(clip(x(0)?, opt(1), opt(2))?),
        "Slice" => {
            let t = x(0)?;
            let ints = |i: usize| opt(i).map(Tensor::ints).transpose();
            let spec = slice_spec(&t.shape, &x(1)?.ints()?, &x(2)?.ints()?, ints(3)?, ints(4)?)?;
            one(slice(t, &spec))
        }
        "Split" => split(node, x(0)?, opt(1), opset),
        "Expand" => {
            let t = x(0)?;
            let target: Vec<usize> = x(1)?.ints()?.iter().map(|&d| d as usize).collect();
            let shape = broadcast_shape(&[&t.shape, &target])?;
            let off = offsets(&shape, 0, &broadcast_strides(&t.shape, &shape));
            one(Tensor { data: t.data.take(&off), shape })
        }
        "Transpose" => {
            let t = x(0)?;
            let perm = perm(node, t.shape.len())?;
            one(transpose(t, &perm))
        }
        "Gather" => one(gather(x(0)?, x(1)?, node.int("axis", 0))?),
        "GatherND" => one(gather_nd(x(0)?, x(1)?, node.int("batch_dims", 0) as usize)?),
        "ScatterND" => one(scatter_nd(x(0)?, x(1)?, x(2)?)?),
        "ConstantOfShape" => {
            let shape: Vec<usize> = x(0)?.ints()?.iter().map(|&d| d as usize).collect();
            let n = numel(&shape);
            let data = match node.attrs.get("value") {
                Some(Attr::Tensor(v)) => each!(&v.data, v => vec![v[0]; n]),
                _ => Data::F32(vec![0.0; n]),
            };
            one(Tensor { shape, data })
        }
        "MatMul" => one(matmul(x(0)?, x(1)?)?),
        "Conv" => one(conv(node, x(0)?, x(1)?, opt(2))?),
        "ConvTranspose" => one(conv_transpose(node, x(0)?, x(1)?, opt(2))?),
        "DeformConv" => one(deform_conv(node, x(0)?, x(1)?, x(2)?, x(3)?, opt(4))?),
        "LayerNormalization" => {
            ensure!(node.outputs.iter().skip(1).all(String::is_empty), "LayerNormalization statistics outputs");
            one(layer_norm(x(0)?, opt(1), opt(2), node.int("axis", -1), node.float("epsilon", 1e-5))?)
        }
        "ReduceMean" => one(reduce_mean(x(0)?, axes(node, opt(1))?, node.int("keepdims", 1) != 0)?),
        "Softmax" => {
            ensure!(opset >= 13, "Softmax before opset 13 flattens");
            one(softmax(x(0)?, node.int("axis", -1))?)
        }
        "Resize" => one(resize(node, x(0)?, opt(2), opt(3))?),
        "Pad" => one(pad(node, x(0)?, opt(1), opt(2), opt(3))?),
        "BatchNormalization" => {
            let (scale, shift) = batch_norm_affine(node, x(1)?, x(2)?, x(3)?, x(4)?)?;
            let t = x(0)?;
            let mut shape = vec![1; t.shape.len()];
            shape[1] = t.shape[1];
            let y = binary("Mul", t, &scale.reshaped(shape.clone()), 0)?;
            one(binary("Add", &y, &shift.reshaped(shape), 0)?)
        }
        "GlobalAveragePool" => {
            let t = x(0)?;
            let rows = t.shape[0] * t.shape[1];
            let cols = numel(&t.shape[2..]);
            let v = t.as_f32()?;
            let mut shape = t.shape.clone();
            shape[2..].iter_mut().for_each(|d| *d = 1);
            let means =
                (0..rows).map(|r| (v[r * cols..(r + 1) * cols].iter().map(|&x| f64::from(x)).sum::<f64>() / cols as f64) as f32).collect();
            one(Tensor::f32(shape, means))
        }
        op => bail!("no reference for {op}"),
    }
}

fn constant(node: &Node) -> Result<Tensor> {
    Ok(match (node.attrs.get("value"), node.attrs.get("value_float"), node.attrs.get("value_int")) {
        (Some(Attr::Tensor(t)), ..) => t.clone(),
        (_, Some(Attr::Float(v)), _) => Tensor::f32(vec![], vec![*v]),
        (_, _, Some(Attr::Int(v))) => Tensor::i64(vec![], vec![*v]),
        _ => match (node.attrs.get("value_floats"), node.attrs.get("value_ints")) {
            (Some(Attr::Floats(v)), _) => Tensor::f32(vec![v.len()], v.clone()),
            (_, Some(Attr::Ints(v))) => Tensor::i64(vec![v.len()], v.clone()),
            _ => bail!("Constant {} has no value", node.name),
        },
    })
}

pub fn shape_of(node: &Node, shape: &[usize]) -> Tensor {
    let r = shape.len() as i64;
    let clamp = |v: i64| (if v < 0 { v + r } else { v }).clamp(0, r) as usize;
    let (s, e) = (clamp(node.int("start", 0)), clamp(node.int("end", r)));
    let dims: Vec<i64> = shape[s..e.max(s)].iter().map(|&d| d as i64).collect();
    Tensor::i64(vec![dims.len()], dims)
}

/// Axes from the attribute (older opsets) or the input (newer ones).
pub fn axes(node: &Node, input: Option<&Tensor>) -> Result<Option<Vec<i64>>> {
    Ok(match node.ints("axes") {
        Some(a) => Some(a.to_vec()),
        None => input.map(Tensor::ints).transpose()?,
    })
}

pub fn unsqueeze_shape(shape: &[usize], axes: &[i64]) -> Result<Vec<usize>> {
    let rank = shape.len() + axes.len();
    let mut ax: Vec<usize> = axes.iter().map(|&a| axis(a, rank)).collect::<Result<_>>()?;
    ax.sort_unstable();
    let mut dims = shape.iter();
    (0..rank)
        .map(|i| match ax.binary_search(&i) {
            Ok(_) => Ok(1),
            Err(_) => dims.next().copied().context("bad Unsqueeze axes"),
        })
        .collect()
}

pub fn squeeze_shape(shape: &[usize], axes: Option<&[i64]>) -> Result<Vec<usize>> {
    let ax: Vec<usize> = match axes {
        Some(a) => a.iter().map(|&a| axis(a, shape.len())).collect::<Result<_>>()?,
        None => (0..shape.len()).filter(|&i| shape[i] == 1).collect(),
    };
    for &a in &ax {
        ensure!(shape[a] == 1, "squeezing dim {a} of {shape:?}");
    }
    Ok((0..shape.len()).filter(|i| !ax.contains(i)).map(|i| shape[i]).collect())
}

pub fn reshape_shape(shape: &[usize], target: &[i64], allowzero: i64) -> Result<Vec<usize>> {
    let mut out = Vec::with_capacity(target.len());
    let mut infer = None;
    for (i, &d) in target.iter().enumerate() {
        match d {
            -1 => {
                infer = Some(i);
                out.push(1);
            }
            0 if allowzero == 0 => out.push(*shape.get(i).context("Reshape copies a missing dim")?),
            d if d >= 0 => out.push(d as usize),
            d => bail!("Reshape to {d}"),
        }
    }
    if let Some(i) = infer {
        let known = numel(&out);
        ensure!(known > 0 && numel(shape).is_multiple_of(known), "cannot reshape {shape:?} to {target:?}");
        out[i] = numel(shape) / known;
    }
    ensure!(numel(&out) == numel(shape), "cannot reshape {shape:?} to {target:?}");
    Ok(out)
}

fn concat_vec<T: Copy>(parts: &[&[T]], chunks: &[usize], outer: usize) -> Vec<T> {
    let mut out = Vec::with_capacity(chunks.iter().sum::<usize>() * outer);
    for o in 0..outer {
        for (p, &c) in parts.iter().zip(chunks) {
            out.extend_from_slice(&p[o * c..(o + 1) * c]);
        }
    }
    out
}

pub fn concat(ts: &[&Tensor], axis_attr: i64) -> Result<Tensor> {
    let rank = ts[0].shape.len();
    let ax = axis(axis_attr, rank)?;
    let mut shape = ts[0].shape.clone();
    shape[ax] = ts.iter().map(|t| t.shape[ax]).sum();
    for t in ts {
        ensure!(
            t.shape.len() == rank && (0..rank).all(|d| d == ax || t.shape[d] == shape[d]),
            "Concat of {:?} along {ax}",
            ts.iter().map(|t| &t.shape).collect::<Vec<_>>()
        );
    }
    let outer = numel(&shape[..ax]);
    let chunks: Vec<usize> = ts.iter().map(|t| numel(&t.shape[ax..])).collect();
    let data = match ts[0].dtype() {
        DType::F32 => Data::F32(concat_vec(&ts.iter().map(|t| t.as_f32()).collect::<Result<Vec<_>>>()?, &chunks, outer)),
        DType::I64 => Data::I64(concat_vec(&ts.iter().map(|t| t.as_i64()).collect::<Result<Vec<_>>>()?, &chunks, outer)),
        DType::Bool => Data::Bool(concat_vec(&ts.iter().map(|t| t.as_bool()).collect::<Result<Vec<_>>>()?, &chunks, outer)),
    };
    Ok(Tensor { shape, data })
}

/// `f` over two operands broadcast to `shape` (strides `sa` and `sb`).
fn zip<A: Copy, B: Copy, R>(a: &[A], sa: &[isize], b: &[B], sb: &[isize], shape: &[usize], f: impl Fn(A, B) -> R) -> Vec<R> {
    if sa == sb && a.len() == numel(shape) {
        return a.iter().zip(b).map(|(&x, &y)| f(x, y)).collect();
    }
    let mut out = Vec::with_capacity(numel(shape));
    walk(shape, [(0, sa), (0, sb)], |[i, j]| out.push(f(a[i], b[j])));
    out
}

pub fn python_mod(a: i64, b: i64) -> i64 {
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) { r + b } else { r }
}

pub fn binary(op: &str, a: &Tensor, b: &Tensor, fmod: i64) -> Result<Tensor> {
    let shape = broadcast_shape(&[&a.shape, &b.shape])?;
    let (sa, sb) = (broadcast_strides(&a.shape, &shape), broadcast_strides(&b.shape, &shape));
    let s = &shape;
    let data = match (op, &a.data, &b.data) {
        ("Add", Data::F32(x), Data::F32(y)) => Data::F32(zip(x, &sa, y, &sb, s, |x, y| x + y)),
        ("Sub", Data::F32(x), Data::F32(y)) => Data::F32(zip(x, &sa, y, &sb, s, |x, y| x - y)),
        ("Mul", Data::F32(x), Data::F32(y)) => Data::F32(zip(x, &sa, y, &sb, s, |x, y| x * y)),
        ("Div", Data::F32(x), Data::F32(y)) => Data::F32(zip(x, &sa, y, &sb, s, |x, y| x / y)),
        ("Pow", Data::F32(x), Data::F32(y)) => Data::F32(zip(x, &sa, y, &sb, s, f32::powf)),
        ("Pow", Data::F32(x), Data::I64(y)) => Data::F32(zip(x, &sa, y, &sb, s, |x, y| x.powi(y as i32))),
        ("Mod", Data::F32(x), Data::F32(y)) if fmod == 1 => Data::F32(zip(x, &sa, y, &sb, s, |x, y| x % y)),
        ("Add", Data::I64(x), Data::I64(y)) => Data::I64(zip(x, &sa, y, &sb, s, i64::wrapping_add)),
        ("Sub", Data::I64(x), Data::I64(y)) => Data::I64(zip(x, &sa, y, &sb, s, i64::wrapping_sub)),
        ("Mul", Data::I64(x), Data::I64(y)) => Data::I64(zip(x, &sa, y, &sb, s, i64::wrapping_mul)),
        ("Div", Data::I64(x), Data::I64(y)) => {
            ensure!(!y.contains(&0), "integer division by zero");
            Data::I64(zip(x, &sa, y, &sb, s, |x, y| x / y))
        }
        ("Mod", Data::I64(x), Data::I64(y)) => {
            ensure!(!y.contains(&0), "integer modulo by zero");
            if fmod == 1 { Data::I64(zip(x, &sa, y, &sb, s, |x, y| x % y)) } else { Data::I64(zip(x, &sa, y, &sb, s, python_mod)) }
        }
        ("Equal", Data::F32(x), Data::F32(y)) => Data::Bool(zip(x, &sa, y, &sb, s, |x, y| x == y)),
        ("Equal", Data::I64(x), Data::I64(y)) => Data::Bool(zip(x, &sa, y, &sb, s, |x, y| x == y)),
        ("Equal", Data::Bool(x), Data::Bool(y)) => Data::Bool(zip(x, &sa, y, &sb, s, |x, y| x == y)),
        _ => bail!("{op} on {:?} and {:?}", a.dtype(), b.dtype()),
    };
    Ok(Tensor { shape, data })
}

fn where_(c: &Tensor, x: &Tensor, y: &Tensor) -> Result<Tensor> {
    let shape = broadcast_shape(&[&c.shape, &x.shape, &y.shape])?;
    let st = [c, x, y].map(|t| broadcast_strides(&t.shape, &shape));
    let cond = c.as_bool()?;
    fn pick<T: Copy>(shape: &[usize], st: &[Vec<isize>; 3], cond: &[bool], a: &[T], b: &[T]) -> Vec<T> {
        let mut out = Vec::with_capacity(numel(shape));
        walk(shape, [(0, &st[0][..]), (0, &st[1][..]), (0, &st[2][..])], |[c, i, j]| out.push(if cond[c] { a[i] } else { b[j] }));
        out
    }
    let data = match (&x.data, &y.data) {
        (Data::F32(a), Data::F32(b)) => Data::F32(pick(&shape, &st, cond, a, b)),
        (Data::I64(a), Data::I64(b)) => Data::I64(pick(&shape, &st, cond, a, b)),
        (Data::Bool(a), Data::Bool(b)) => Data::Bool(pick(&shape, &st, cond, a, b)),
        _ => bail!("Where on {:?} and {:?}", x.dtype(), y.dtype()),
    };
    Ok(Tensor { shape, data })
}

fn unary(t: &Tensor, f: impl Fn(f32) -> f32) -> Result<Tensor> {
    Ok(Tensor::f32(t.shape.clone(), t.as_f32()?.iter().map(|&v| f(v)).collect()))
}

/// Error function, fractional error below 1.2e-7 (Numerical Recipes' erfc).
pub fn erf(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -z * z - 1.26551223
        + t * (1.00002368
            + t * (0.37409196
                + t * (0.09678418
                    + t * (-0.18628806 + t * (0.27886807 + t * (-1.13520398 + t * (1.48851587 + t * (-0.82215223 + t * 0.17087277))))))));
    let r = t * poly.exp();
    if x >= 0.0 { 1.0 - r } else { r - 1.0 }
}

fn cast(t: &Tensor, to: i64) -> Result<Tensor> {
    let data = match (to, &t.data) {
        (1, Data::F32(v)) => Data::F32(v.clone()),
        (1, Data::I64(v)) => Data::F32(v.iter().map(|&x| x as f32).collect()),
        (1, Data::Bool(v)) => Data::F32(v.iter().map(|&x| f32::from(u8::from(x))).collect()),
        (6 | 7, Data::F32(v)) => Data::I64(v.iter().map(|&x| x as i64).collect()),
        (6 | 7, Data::I64(v)) => Data::I64(v.clone()),
        (6 | 7, Data::Bool(v)) => Data::I64(v.iter().map(|&x| i64::from(x)).collect()),
        (9, Data::F32(v)) => Data::Bool(v.iter().map(|&x| x != 0.0).collect()),
        (9, Data::I64(v)) => Data::Bool(v.iter().map(|&x| x != 0).collect()),
        (9, Data::Bool(v)) => Data::Bool(v.clone()),
        (to, _) => bail!("Cast of {:?} to {to}", t.dtype()),
    };
    Ok(Tensor { shape: t.shape.clone(), data })
}

fn clip(t: &Tensor, lo: Option<&Tensor>, hi: Option<&Tensor>) -> Result<Tensor> {
    let data = match &t.data {
        Data::F32(v) => {
            let lo = lo.map(|l| l.as_f32().map(|l| l[0])).transpose()?.unwrap_or(f32::NEG_INFINITY);
            let hi = hi.map(|h| h.as_f32().map(|h| h[0])).transpose()?.unwrap_or(f32::INFINITY);
            Data::F32(v.iter().map(|&x| x.max(lo).min(hi)).collect())
        }
        Data::I64(v) => {
            let lo = lo.map(|l| l.as_i64().map(|l| l[0])).transpose()?.unwrap_or(i64::MIN);
            let hi = hi.map(|h| h.as_i64().map(|h| h[0])).transpose()?.unwrap_or(i64::MAX);
            Data::I64(v.iter().map(|&x| x.max(lo).min(hi)).collect())
        }
        Data::Bool(_) => bail!("Clip of bool"),
    };
    Ok(Tensor { shape: t.shape.clone(), data })
}

/// Per dim of the input: first index, step and count.
pub struct SliceSpec {
    pub start: Vec<usize>,
    pub step: Vec<isize>,
    pub count: Vec<usize>,
}

pub fn slice_spec(shape: &[usize], starts: &[i64], ends: &[i64], axes: Option<Vec<i64>>, steps: Option<Vec<i64>>) -> Result<SliceSpec> {
    let rank = shape.len();
    let mut spec = SliceSpec { start: vec![0; rank], step: vec![1; rank], count: shape.to_vec() };
    let axes = axes.unwrap_or_else(|| (0..starts.len() as i64).collect());
    for (i, &a) in axes.iter().enumerate() {
        let a = axis(a, rank)?;
        let d = shape[a] as i64;
        let step = steps.as_ref().map_or(1, |s| s[i]);
        ensure!(step != 0, "Slice step 0");
        let norm = |v: i64| if v < 0 { v.saturating_add(d) } else { v };
        let (start, end) = (norm(starts[i]), norm(ends[i]));
        let (start, count) = if step > 0 {
            let (s, e) = (start.clamp(0, d), end.clamp(0, d));
            (s, if e > s { (e - s + step - 1) / step } else { 0 })
        } else {
            let (s, e) = (start.clamp(0, d - 1), end.clamp(-1, d - 1));
            (s, if s > e { (s - e - step - 1) / -step } else { 0 })
        };
        spec.start[a] = start.max(0) as usize;
        spec.step[a] = step as isize;
        spec.count[a] = count as usize;
    }
    Ok(spec)
}

fn slice(t: &Tensor, spec: &SliceSpec) -> Tensor {
    let st = strides(&t.shape);
    let base: usize = spec.start.iter().zip(&st).map(|(a, b)| a * b).sum();
    let s: Vec<isize> = st.iter().zip(&spec.step).map(|(&a, &b)| a as isize * b).collect();
    let off = offsets(&spec.count, base, &s);
    Tensor { shape: spec.count.clone(), data: t.data.take(&off) }
}

fn split(node: &Node, t: &Tensor, sizes: Option<&Tensor>, opset: i64) -> Result<Vec<Tensor>> {
    let ax = axis(node.int("axis", 0), t.shape.len())?;
    let parts = split_sizes(node, t.shape[ax], sizes, opset)?;
    let mut start = 0;
    Ok(parts
        .into_iter()
        .map(|n| {
            let mut spec = SliceSpec { start: vec![0; t.shape.len()], step: vec![1; t.shape.len()], count: t.shape.clone() };
            spec.start[ax] = start;
            spec.count[ax] = n;
            start += n;
            slice(t, &spec)
        })
        .collect())
}

pub fn split_sizes(node: &Node, dim: usize, sizes: Option<&Tensor>, opset: i64) -> Result<Vec<usize>> {
    let given = match sizes {
        Some(s) if opset >= 13 => Some(s.ints()?),
        _ => node.ints("split").map(<[i64]>::to_vec),
    };
    Ok(match given {
        Some(v) => v.iter().map(|&n| n as usize).collect(),
        None => {
            let n = node.outputs.len();
            let each = dim.div_ceil(n);
            (0..n).map(|i| each.min(dim - (each * i).min(dim))).collect()
        }
    })
}

pub fn perm(node: &Node, rank: usize) -> Result<Vec<usize>> {
    Ok(match node.ints("perm") {
        Some(p) => p.iter().map(|&a| axis(a, rank)).collect::<Result<_>>()?,
        None => (0..rank).rev().collect(),
    })
}

fn transpose(t: &Tensor, perm: &[usize]) -> Tensor {
    let st = strides(&t.shape);
    let shape: Vec<usize> = perm.iter().map(|&p| t.shape[p]).collect();
    let s: Vec<isize> = perm.iter().map(|&p| st[p] as isize).collect();
    let off = offsets(&shape, 0, &s);
    Tensor { shape, data: t.data.take(&off) }
}

fn gather(data: &Tensor, indices: &Tensor, axis_attr: i64) -> Result<Tensor> {
    let ax = axis(axis_attr, data.shape.len())?;
    let n = data.shape[ax] as i64;
    let outer = numel(&data.shape[..ax]);
    let inner = numel(&data.shape[ax + 1..]);
    let ind: Vec<usize> = indices
        .ints()?
        .iter()
        .map(|&i| {
            let i = if i < 0 { i + n } else { i };
            ensure!((0..n).contains(&i), "Gather index {i} outside {n}");
            Ok(i as usize)
        })
        .collect::<Result<_>>()?;
    let mut idx = Vec::with_capacity(outer * ind.len() * inner);
    for o in 0..outer {
        for &i in &ind {
            idx.extend((0..inner).map(|j| (o * n as usize + i) * inner + j));
        }
    }
    let shape = [&data.shape[..ax], &indices.shape[..], &data.shape[ax + 1..]].concat();
    Ok(Tensor { shape, data: data.data.take(&idx) })
}

fn gather_nd(data: &Tensor, indices: &Tensor, b: usize) -> Result<Tensor> {
    let q = indices.shape.len();
    let k = indices.shape[q - 1];
    ensure!(data.shape[..b] == indices.shape[..b], "GatherND batch dims differ");
    let batch = numel(&data.shape[..b]);
    let m = numel(&indices.shape[b..q - 1]);
    let inner = numel(&data.shape[b + k..]);
    let block = numel(&data.shape[b..]);
    let st = strides(&data.shape[b..]);
    let ind = indices.ints()?;
    let mut idx = Vec::with_capacity(batch * m * inner);
    for bi in 0..batch {
        for mi in 0..m {
            let t = &ind[(bi * m + mi) * k..][..k];
            let mut off = bi * block;
            for j in 0..k {
                let d = data.shape[b + j] as i64;
                let v = if t[j] < 0 { t[j] + d } else { t[j] };
                ensure!((0..d).contains(&v), "GatherND index {v} outside {d}");
                off += v as usize * st[j];
            }
            idx.extend(off..off + inner);
        }
    }
    let shape = [&indices.shape[..q - 1], &data.shape[b + k..]].concat();
    Ok(Tensor { shape, data: data.data.take(&idx) })
}

fn scatter_nd(data: &Tensor, indices: &Tensor, updates: &Tensor) -> Result<Tensor> {
    let q = indices.shape.len();
    let k = indices.shape[q - 1];
    let inner = numel(&data.shape[k..]);
    let st = strides(&data.shape);
    let ind = indices.ints()?;
    let n = numel(&indices.shape[..q - 1]);
    let mut targets = Vec::with_capacity(n);
    for i in 0..n {
        let mut off = 0;
        for j in 0..k {
            let d = data.shape[j] as i64;
            let v = ind[i * k + j];
            let v = if v < 0 { v + d } else { v };
            ensure!((0..d).contains(&v), "ScatterND index {v} outside {d}");
            off += v as usize * st[j];
        }
        targets.push(off);
    }
    fn scatter<T: Copy>(d: &[T], u: &[T], targets: &[usize], inner: usize) -> Vec<T> {
        let mut out = d.to_vec();
        for (i, &t) in targets.iter().enumerate() {
            out[t..t + inner].copy_from_slice(&u[i * inner..(i + 1) * inner]);
        }
        out
    }
    let out = match (&data.data, &updates.data) {
        (Data::F32(d), Data::F32(u)) => Data::F32(scatter(d, u, &targets, inner)),
        (Data::I64(d), Data::I64(u)) => Data::I64(scatter(d, u, &targets, inner)),
        (Data::Bool(d), Data::Bool(u)) => Data::Bool(scatter(d, u, &targets, inner)),
        _ => bail!("ScatterND of {:?} with {:?}", data.dtype(), updates.dtype()),
    };
    Ok(Tensor { shape: data.shape.clone(), data: out })
}

pub fn matmul(a: &Tensor, b: &Tensor) -> Result<Tensor> {
    let (ra, rb) = (a.shape.len(), b.shape.len());
    ensure!(ra >= 2 && rb >= 2, "MatMul of {:?} and {:?}", a.shape, b.shape);
    let (m, k) = (a.shape[ra - 2], a.shape[ra - 1]);
    let (k2, n) = (b.shape[rb - 2], b.shape[rb - 1]);
    ensure!(k == k2, "MatMul of {:?} and {:?}", a.shape, b.shape);
    let batch = broadcast_shape(&[&a.shape[..ra - 2], &b.shape[..rb - 2]])?;
    let scale = |s: Vec<isize>, by: usize| s.into_iter().map(|v| v * by as isize).collect::<Vec<_>>();
    let boff = [
        offsets(&batch, 0, &scale(broadcast_strides(&a.shape[..ra - 2], &batch), m * k)),
        offsets(&batch, 0, &scale(broadcast_strides(&b.shape[..rb - 2], &batch), k * n)),
    ];
    let (av, bv) = (a.as_f32()?, b.as_f32()?);
    let mut out = vec![0f32; numel(&batch) * m * n];
    par_rows(&mut out, n, |r, row| {
        let (bi, i) = (r / m, r % m);
        let arow = &av[boff[0][bi] + i * k..][..k];
        let bmat = &bv[boff[1][bi]..];
        for (kk, &x) in arow.iter().enumerate() {
            for (o, &y) in row.iter_mut().zip(&bmat[kk * n..(kk + 1) * n]) {
                *o += x * y;
            }
        }
    });
    let shape = [&batch[..], &[m, n]].concat();
    Ok(Tensor::f32(shape, out))
}

pub struct ConvGeometry {
    pub stride: [usize; 2],
    /// Top, left, bottom, right.
    pub pads: [usize; 4],
    pub dilation: [usize; 2],
    pub group: usize,
}

pub fn conv_geometry(node: &Node) -> Result<ConvGeometry> {
    let auto_pad = node.string("auto_pad", "NOTSET");
    ensure!(auto_pad == "NOTSET", "auto_pad {auto_pad}");
    let pair = |name: &str| -> Result<[usize; 2]> {
        Ok(match node.ints(name) {
            Some(&[a, b]) => [a as usize, b as usize],
            None => [1, 1],
            Some(v) => bail!("{name} {v:?}"),
        })
    };
    let pads = match node.ints("pads") {
        Some(&[a, b, c, d]) => [a, b, c, d].map(|p| p as usize),
        None => [0; 4],
        Some(v) => bail!("pads {v:?}"),
    };
    ensure!(node.ints("output_shape").is_none(), "ConvTranspose output_shape");
    Ok(ConvGeometry { stride: pair("strides")?, pads, dilation: pair("dilations")?, group: node.int("group", 1) as usize })
}

pub fn conv_out(input: usize, k: usize, stride: usize, before: usize, after: usize, dilation: usize) -> usize {
    (input + before + after - dilation * (k - 1) - 1) / stride + 1
}

fn conv(node: &Node, x: &Tensor, w: &Tensor, b: Option<&Tensor>) -> Result<Tensor> {
    ensure!(x.shape.len() == 4 && w.shape.len() == 4, "only 2-D Conv: {:?} {:?}", x.shape, w.shape);
    let g = conv_geometry(node)?;
    let [n, c, h, wd] = x.shape[..] else { unreachable!() };
    let [co, cig, kh, kw] = w.shape[..] else { unreachable!() };
    ensure!(cig * g.group == c && co % g.group == 0, "Conv groups");
    let oh = conv_out(h, kh, g.stride[0], g.pads[0], g.pads[2], g.dilation[0]);
    let ow = conv_out(wd, kw, g.stride[1], g.pads[1], g.pads[3], g.dilation[1]);
    let (xv, wv) = (x.as_f32()?, w.as_f32()?);
    let bias = b.map(Tensor::as_f32).transpose()?;
    let cog = co / g.group;
    let mut out = vec![0f32; n * co * oh * ow];
    par_rows(&mut out, oh * ow, |plane, o| {
        let (ni, oc) = (plane / co, plane % co);
        o.fill(bias.map_or(0.0, |b| b[oc]));
        let gi = oc / cog;
        for ci in 0..cig {
            let input = &xv[(ni * c + gi * cig + ci) * h * wd..][..h * wd];
            for ky in 0..kh {
                for kx in 0..kw {
                    let wt = wv[((oc * cig + ci) * kh + ky) * kw + kx];
                    let shift = (kx * g.dilation[1]) as isize - g.pads[1] as isize;
                    let s = g.stride[1] as isize;
                    let lo = if shift >= 0 { 0 } else { ((-shift) + s - 1) / s } as usize;
                    let hi = (((wd as isize - 1 - shift).div_euclid(s)) + 1).clamp(0, ow as isize) as usize;
                    for oy in 0..oh {
                        let iy = (oy * g.stride[0] + ky * g.dilation[0]) as isize - g.pads[0] as isize;
                        if iy < 0 || iy >= h as isize {
                            continue;
                        }
                        let row = &input[iy as usize * wd..][..wd];
                        let orow = &mut o[oy * ow..][..ow];
                        for ox in lo..hi.max(lo) {
                            orow[ox] += wt * row[(ox as isize * s + shift) as usize];
                        }
                    }
                }
            }
        }
    });
    Ok(Tensor::f32(vec![n, co, oh, ow], out))
}

fn conv_transpose(node: &Node, x: &Tensor, w: &Tensor, b: Option<&Tensor>) -> Result<Tensor> {
    ensure!(x.shape.len() == 4 && w.shape.len() == 4, "only 2-D ConvTranspose");
    let g = conv_geometry(node)?;
    ensure!(g.group == 1, "grouped ConvTranspose");
    let op = node.ints("output_padding").map_or([0, 0], |v| [v[0] as usize, v[1] as usize]);
    let [n, c, h, wd] = x.shape[..] else { unreachable!() };
    let [ci2, co, kh, kw] = w.shape[..] else { unreachable!() };
    ensure!(ci2 == c, "ConvTranspose channels");
    let oh = (h - 1) * g.stride[0] + g.dilation[0] * (kh - 1) + 1 + op[0] - g.pads[0] - g.pads[2];
    let ow = (wd - 1) * g.stride[1] + g.dilation[1] * (kw - 1) + 1 + op[1] - g.pads[1] - g.pads[3];
    let (xv, wv) = (x.as_f32()?, w.as_f32()?);
    let bias = b.map(Tensor::as_f32).transpose()?;
    let mut out = vec![0f32; n * co * oh * ow];
    par_rows(&mut out, oh * ow, |plane, o| {
        let (ni, oc) = (plane / co, plane % co);
        o.fill(bias.map_or(0.0, |b| b[oc]));
        for ic in 0..c {
            let input = &xv[(ni * c + ic) * h * wd..][..h * wd];
            for ky in 0..kh {
                for kx in 0..kw {
                    let wt = wv[((ic * co + oc) * kh + ky) * kw + kx];
                    for iy in 0..h {
                        let oy = (iy * g.stride[0] + ky * g.dilation[0]) as isize - g.pads[0] as isize;
                        if oy < 0 || oy >= oh as isize {
                            continue;
                        }
                        for ix in 0..wd {
                            let ox = (ix * g.stride[1] + kx * g.dilation[1]) as isize - g.pads[1] as isize;
                            if ox >= 0 && ox < ow as isize {
                                o[oy as usize * ow + ox as usize] += wt * input[iy * wd + ix];
                            }
                        }
                    }
                }
            }
        }
    });
    Ok(Tensor::f32(vec![n, co, oh, ow], out))
}

/// The deformable convolution `fuse::deform_convs` puts in place of
/// BiRefNet's exported blocks, with the export's semantics: positions are in
/// the padded input, corner indices are clipped to `bounds`, the bilinear
/// weights come from the unclipped positions, and the mask scales each tap.
fn deform_conv(node: &Node, x: &Tensor, pos: &Tensor, mask: &Tensor, w: &Tensor, b: Option<&Tensor>) -> Result<Tensor> {
    let k = node.int("kernel", 1) as usize;
    let pads = node.ints("pads").context("DeformConv pads")?;
    let bounds = node.ints("bounds").context("DeformConv bounds")?;
    let [n, c, h, wd] = x.shape[..] else { bail!("DeformConv input {:?}", x.shape) };
    let [co, ci, kh, kw] = w.shape[..] else { bail!("DeformConv weight {:?}", w.shape) };
    ensure!(n == 1 && ci == c && kh == k && kw == k, "DeformConv of {:?} with {:?}", x.shape, w.shape);
    let r = pos.shape.len();
    let (oh, ow) = (pos.shape[r - 2], pos.shape[r - 1]);
    let (hw, k2) = (oh * ow, k * k);
    ensure!(pos.numel() == 2 * k2 * hw && mask.numel() == k2 * hw, "DeformConv positions {:?}, mask {:?}", pos.shape, mask.shape);
    let (xv, pv, mv) = (x.as_f32()?, pos.as_f32()?, mask.as_f32()?);
    let sample = |cc: usize, yy: i64, xx: i64| {
        let (iy, ix) = (yy - pads[0], xx - pads[1]);
        if iy < 0 || ix < 0 || iy >= h as i64 || ix >= wd as i64 { 0.0 } else { xv[(cc * h + iy as usize) * wd + ix as usize] }
    };
    let mut s = vec![0f32; c * k2 * hw];
    par_rows(&mut s, hw, |row, out| {
        let (cc, t) = (row / k2, row % k2);
        for (i, o) in out.iter_mut().enumerate() {
            let (py, px) = (pv[2 * t * hw + i], pv[(2 * t + 1) * hw + i]);
            let (fy0, fx0) = (py.floor(), px.floor());
            let (fy, fx) = (py - fy0, px - fx0);
            let (y0, x0) = (fy0 as i64, fx0 as i64);
            let (y0c, y1c) = (y0.clamp(0, bounds[0]), (y0 + 1).clamp(0, bounds[0]));
            let (x0c, x1c) = (x0.clamp(0, bounds[1]), (x0 + 1).clamp(0, bounds[1]));
            let v = (sample(cc, y0c, x0c) * (1.0 - fx) + sample(cc, y0c, x1c) * fx) * (1.0 - fy)
                + (sample(cc, y1c, x0c) * (1.0 - fx) + sample(cc, y1c, x1c) * fx) * fy;
            *o = v * mv[t * hw + i];
        }
    });
    let y = matmul(&w.reshaped(vec![co, c * k2]), &Tensor::f32(vec![c * k2, hw], s))?;
    let mut y = y.as_f32()?.to_vec();
    if let Some(b) = b {
        let b = b.as_f32()?;
        for (row, &bias) in y.chunks_mut(hw).zip(b) {
            row.iter_mut().for_each(|v| *v += bias);
        }
    }
    Ok(Tensor::f32(vec![1, co, oh, ow], y))
}

fn layer_norm(x: &Tensor, scale: Option<&Tensor>, bias: Option<&Tensor>, axis_attr: i64, eps: f32) -> Result<Tensor> {
    let ax = axis(axis_attr, x.shape.len())?;
    let cols = numel(&x.shape[ax..]);
    let v = x.as_f32()?;
    let scale = scale.map(Tensor::as_f32).transpose()?;
    let bias = bias.map(Tensor::as_f32).transpose()?;
    for t in scale.iter().chain(bias.iter()) {
        ensure!(t.len() == cols, "LayerNormalization scale of {} for {cols} columns", t.len());
    }
    let mut out = vec![0f32; v.len()];
    par_rows(&mut out, cols, |r, o| {
        let row = &v[r * cols..][..cols];
        let mean = row.iter().map(|&x| f64::from(x)).sum::<f64>() / cols as f64;
        let var = row.iter().map(|&x| (f64::from(x) - mean).powi(2)).sum::<f64>() / cols as f64;
        let inv = 1.0 / (var + f64::from(eps)).sqrt();
        for (j, (o, &x)) in o.iter_mut().zip(row).enumerate() {
            let y = (f64::from(x) - mean) * inv;
            *o = (y * scale.map_or(1.0, |s| f64::from(s[j])) + bias.map_or(0.0, |b| f64::from(b[j]))) as f32;
        }
    });
    Ok(Tensor::f32(x.shape.clone(), out))
}

fn reduce_mean(x: &Tensor, axes: Option<Vec<i64>>, keep: bool) -> Result<Tensor> {
    let rank = x.shape.len();
    let reduced: Vec<usize> = match axes {
        Some(a) if !a.is_empty() => a.iter().map(|&a| axis(a, rank)).collect::<Result<_>>()?,
        _ => (0..rank).collect(),
    };
    let st = strides(&x.shape);
    let kept: Vec<usize> = (0..rank).filter(|d| !reduced.contains(d)).collect();
    let outer_shape: Vec<usize> = kept.iter().map(|&d| x.shape[d]).collect();
    let inner_shape: Vec<usize> = reduced.iter().map(|&d| x.shape[d]).collect();
    let outer = offsets(&outer_shape, 0, &kept.iter().map(|&d| st[d] as isize).collect::<Vec<_>>());
    let inner = offsets(&inner_shape, 0, &reduced.iter().map(|&d| st[d] as isize).collect::<Vec<_>>());
    let v = x.as_f32()?;
    let means = outer.iter().map(|&o| (inner.iter().map(|&i| f64::from(v[o + i])).sum::<f64>() / inner.len() as f64) as f32).collect();
    let shape = if keep { (0..rank).map(|d| if reduced.contains(&d) { 1 } else { x.shape[d] }).collect() } else { outer_shape };
    Ok(Tensor::f32(shape, means))
}

fn softmax(x: &Tensor, axis_attr: i64) -> Result<Tensor> {
    let ax = axis(axis_attr, x.shape.len())?;
    let (outer, n, inner) = (numel(&x.shape[..ax]), x.shape[ax], numel(&x.shape[ax + 1..]));
    let v = x.as_f32()?;
    let mut out = vec![0f32; v.len()];
    for o in 0..outer {
        for i in 0..inner {
            let at = |j: usize| (o * n + j) * inner + i;
            let max = (0..n).map(|j| v[at(j)]).fold(f32::NEG_INFINITY, f32::max);
            let sum: f64 = (0..n).map(|j| f64::from(v[at(j)] - max).exp()).sum();
            for j in 0..n {
                out[at(j)] = (f64::from(v[at(j)] - max).exp() / sum) as f32;
            }
        }
    }
    Ok(Tensor::f32(x.shape.clone(), out))
}

/// Source taps of one resized axis: for every output index, (input index,
/// weight) pairs.
pub fn resize_taps(mode: &str, ctm: &str, a: f32, len_in: usize, len_out: usize, scale: f32) -> Result<Vec<Vec<(usize, f32)>>> {
    let last = len_in as f32 - 1.0;
    (0..len_out)
        .map(|o| {
            let o = o as f32;
            let x = match ctm {
                "align_corners" if len_out == 1 => 0.0,
                "align_corners" => o * last / (len_out as f32 - 1.0),
                "half_pixel" => (o + 0.5) / scale - 0.5,
                "pytorch_half_pixel" if len_out > 1 => (o + 0.5) / scale - 0.5,
                "pytorch_half_pixel" => 0.0,
                "asymmetric" => o / scale,
                m => bail!("coordinate_transformation_mode {m}"),
            };
            let clamp = |i: i64| i.clamp(0, len_in as i64 - 1) as usize;
            Ok(match mode {
                "linear" => {
                    let x = x.clamp(0.0, last);
                    let i0 = x.floor();
                    let f = x - i0;
                    vec![(clamp(i0 as i64), 1.0 - f), (clamp(i0 as i64 + 1), f)]
                }
                "cubic" => {
                    let i0 = x.floor();
                    let r = x - i0;
                    let c = [
                        ((a * (r + 1.0) - 5.0 * a) * (r + 1.0) + 8.0 * a) * (r + 1.0) - 4.0 * a,
                        ((a + 2.0) * r - (a + 3.0)) * r * r + 1.0,
                        ((a + 2.0) * (1.0 - r) - (a + 3.0)) * (1.0 - r) * (1.0 - r) + 1.0,
                        ((a * (2.0 - r) - 5.0 * a) * (2.0 - r) + 8.0 * a) * (2.0 - r) - 4.0 * a,
                    ];
                    (0..4).map(|t| (clamp(i0 as i64 - 1 + t as i64), c[t])).collect()
                }
                m => bail!("Resize mode {m}"),
            })
        })
        .collect()
}

/// Output size and scale per dim of a Resize.
pub fn resize_plan(shape: &[usize], scales: Option<&Tensor>, sizes: Option<&Tensor>) -> Result<(Vec<usize>, Vec<f32>)> {
    match (scales.filter(|t| t.numel() > 0), sizes.filter(|t| t.numel() > 0)) {
        (Some(s), _) => {
            let s = s.as_f32()?;
            Ok((shape.iter().zip(s).map(|(&d, &s)| (d as f32 * s).floor() as usize).collect(), s.to_vec()))
        }
        (None, Some(z)) => {
            let z: Vec<usize> = z.ints()?.iter().map(|&d| d as usize).collect();
            let s = shape.iter().zip(&z).map(|(&i, &o)| o as f32 / i as f32).collect();
            Ok((z, s))
        }
        _ => bail!("Resize without scales or sizes"),
    }
}

fn resize(node: &Node, x: &Tensor, scales: Option<&Tensor>, sizes: Option<&Tensor>) -> Result<Tensor> {
    ensure!(x.shape.len() == 4, "only 4-D Resize");
    ensure!(node.int("exclude_outside", 0) == 0, "exclude_outside");
    let (out, sc) = resize_plan(&x.shape, scales, sizes)?;
    ensure!(out[..2] == x.shape[..2], "Resize of batch or channels");
    let mode = node.string("mode", "nearest");
    let ctm = node.string("coordinate_transformation_mode", "half_pixel");
    let a = node.float("cubic_coeff_a", -0.75);
    let ty = resize_taps(&mode, &ctm, a, x.shape[2], out[2], sc[2])?;
    let tx = resize_taps(&mode, &ctm, a, x.shape[3], out[3], sc[3])?;
    let v = x.as_f32()?;
    let (h, w) = (x.shape[2], x.shape[3]);
    let mut o = vec![0f32; numel(&out)];
    par_rows(&mut o, out[2] * out[3], |plane, dst| {
        let src = &v[plane * h * w..][..h * w];
        for (oy, ys) in ty.iter().enumerate() {
            for (ox, xs) in tx.iter().enumerate() {
                let mut acc = 0f32;
                for &(iy, wy) in ys {
                    let mut row = 0f32;
                    for &(ix, wx) in xs {
                        row += wx * src[iy * w + ix];
                    }
                    acc += wy * row;
                }
                dst[oy * out[3] + ox] = acc;
            }
        }
    });
    Ok(Tensor::f32(out, o))
}

pub fn pad_amounts(node: &Node, rank: usize, pads: Option<&Tensor>, axes: Option<&Tensor>) -> Result<Vec<(isize, isize)>> {
    ensure!(axes.is_none(), "Pad axes");
    let mode = node.string("mode", "constant");
    ensure!(mode == "constant", "Pad mode {mode}");
    let p = match pads {
        Some(p) => p.ints()?,
        None => node.ints("pads").context("Pad without pads")?.to_vec(),
    };
    ensure!(p.len() == 2 * rank, "Pad of {} values for rank {rank}", p.len());
    Ok((0..rank).map(|d| (p[d] as isize, p[d + rank] as isize)).collect())
}

fn pad(node: &Node, x: &Tensor, pads: Option<&Tensor>, value: Option<&Tensor>, axes: Option<&Tensor>) -> Result<Tensor> {
    let amounts = pad_amounts(node, x.shape.len(), pads, axes)?;
    let shape: Vec<usize> = x.shape.iter().zip(&amounts).map(|(&d, &(b, e))| (d as isize + b + e) as usize).collect();
    let st = strides(&x.shape);
    let n = numel(&shape);
    let mut idx = Vec::with_capacity(n);
    let mut index = vec![0usize; shape.len()];
    for _ in 0..n {
        let mut off = Some(0usize);
        for d in 0..shape.len() {
            let s = index[d] as isize - amounts[d].0;
            off = off.filter(|_| s >= 0 && s < x.shape[d] as isize).map(|o| o + s as usize * st[d]);
        }
        idx.push(off);
        for d in (0..shape.len()).rev() {
            index[d] += 1;
            if index[d] < shape[d] {
                break;
            }
            index[d] = 0;
        }
    }
    fn fill<T: Copy>(v: &[T], idx: &[Option<usize>], value: T) -> Vec<T> {
        idx.iter().map(|i| i.map_or(value, |i| v[i])).collect()
    }
    let data = match &x.data {
        Data::F32(v) => Data::F32(fill(v, &idx, value.map(|t| t.as_f32().map(|t| t[0])).transpose()?.unwrap_or(0.0))),
        Data::I64(v) => Data::I64(fill(v, &idx, value.map(|t| t.as_i64().map(|t| t[0])).transpose()?.unwrap_or(0))),
        Data::Bool(v) => Data::Bool(fill(v, &idx, value.map(|t| t.as_bool().map(|t| t[0])).transpose()?.unwrap_or(false))),
    };
    Ok(Tensor { shape, data })
}

/// Inference BatchNormalization as a per-channel scale and shift.
pub fn batch_norm_affine(node: &Node, scale: &Tensor, bias: &Tensor, mean: &Tensor, var: &Tensor) -> Result<(Tensor, Tensor)> {
    ensure!(node.int("training_mode", 0) == 0, "training BatchNormalization");
    let eps = f64::from(node.float("epsilon", 1e-5));
    let (s, b, m, v) = (scale.as_f32()?, bias.as_f32()?, mean.as_f32()?, var.as_f32()?);
    let k: Vec<f32> = s.iter().zip(v).map(|(&s, &v)| (f64::from(s) / (f64::from(v) + eps).sqrt()) as f32).collect();
    let t: Vec<f32> = b.iter().zip(m).zip(&k).map(|((&b, &m), &k)| b - m * k).collect();
    Ok((Tensor::f32(vec![k.len()], k), Tensor::f32(vec![t.len()], t)))
}
