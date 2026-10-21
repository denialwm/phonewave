//! Host tensors: constants folded at load time, reference results, and the
//! .npy files the ORT references come in.

use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DType {
    F32,
    I64,
    Bool,
}

#[derive(Clone, Debug)]
pub enum Data {
    F32(Vec<f32>),
    I64(Vec<i64>),
    Bool(Vec<bool>),
}

#[derive(Clone, Debug)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Data,
}

/// Runs the same generic code on whichever element type a `Data` holds and
/// wraps the resulting vector back into the same variant.
#[macro_export]
macro_rules! each {
    ($data:expr, $v:ident => $e:expr) => {
        match $data {
            $crate::tensor::Data::F32($v) => $crate::tensor::Data::F32($e),
            $crate::tensor::Data::I64($v) => $crate::tensor::Data::I64($e),
            $crate::tensor::Data::Bool($v) => $crate::tensor::Data::Bool($e),
        }
    };
}

pub fn numel(shape: &[usize]) -> usize {
    shape.iter().product()
}

pub fn strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![1; shape.len()];
    for i in (0..shape.len().saturating_sub(1)).rev() {
        s[i] = s[i + 1] * shape[i + 1];
    }
    s
}

pub fn broadcast_shape(shapes: &[&[usize]]) -> Result<Vec<usize>> {
    let rank = shapes.iter().map(|s| s.len()).max().unwrap_or(0);
    let mut out = vec![1; rank];
    for s in shapes {
        for (i, &d) in s.iter().enumerate() {
            let o = &mut out[rank - s.len() + i];
            if *o == 1 {
                *o = d;
            } else if d != 1 && d != *o {
                bail!("shapes {shapes:?} do not broadcast");
            }
        }
    }
    Ok(out)
}

/// Strides that read `shape` as if it were broadcast to `out`.
pub fn broadcast_strides(shape: &[usize], out: &[usize]) -> Vec<isize> {
    let st = strides(shape);
    let pad = out.len() - shape.len();
    (0..out.len()).map(|i| if i < pad || shape[i - pad] == 1 { 0 } else { st[i - pad] as isize }).collect()
}

pub fn signed(strides: &[usize]) -> Vec<isize> {
    strides.iter().map(|&s| s as isize).collect()
}

/// Calls `f` with the offsets of every element of `shape`, in row-major
/// order, into operands with the given strides and base offsets.
pub fn walk<const N: usize>(shape: &[usize], operands: [(usize, &[isize]); N], mut f: impl FnMut([usize; N])) {
    if numel(shape) == 0 {
        return;
    }
    let mut base: [isize; N] = operands.map(|(b, _)| b as isize);
    let Some(&inner) = shape.last() else { return f(base.map(|o| o as usize)) };
    let rank = shape.len();
    let step: [isize; N] = operands.map(|(_, s)| s[rank - 1]);
    let mut index = vec![0; rank];
    loop {
        let mut o = base;
        for _ in 0..inner {
            f(o.map(|v| v as usize));
            for k in 0..N {
                o[k] += step[k];
            }
        }
        let mut d = rank - 1;
        loop {
            if d == 0 {
                return;
            }
            d -= 1;
            index[d] += 1;
            for k in 0..N {
                base[k] += operands[k].1[d];
            }
            if index[d] < shape[d] {
                break;
            }
            for k in 0..N {
                base[k] -= operands[k].1[d] * shape[d] as isize;
            }
            index[d] = 0;
        }
    }
}

/// Offsets of every element of `out` in one operand.
pub fn offsets(out: &[usize], base: usize, strides: &[isize]) -> Vec<usize> {
    let mut result = Vec::with_capacity(numel(out));
    walk(out, [(base, strides)], |[o]| result.push(o));
    result
}

/// Number of worker threads: the CPUs the process may use, at most eight.
pub fn workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get()).min(8)
}

/// Fills `out` row by row on the worker threads; `f` gets the row index.
pub fn par_rows<T: Send>(out: &mut [T], row: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if row == 0 || out.is_empty() {
        return;
    }
    let rows = out.len() / row;
    let per = rows.div_ceil(workers().min(rows));
    std::thread::scope(|s| {
        for (t, part) in out.chunks_mut(per * row).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, r) in part.chunks_mut(row).enumerate() {
                    f(t * per + i, r);
                }
            });
        }
    });
}

impl Data {
    pub fn len(&self) -> usize {
        match self {
            Data::F32(v) => v.len(),
            Data::I64(v) => v.len(),
            Data::Bool(v) => v.len(),
        }
    }

    pub fn take(&self, idx: &[usize]) -> Data {
        each!(self, v => idx.iter().map(|&i| v[i]).collect())
    }
}

impl Tensor {
    pub fn f32(shape: Vec<usize>, v: Vec<f32>) -> Self {
        Tensor { shape, data: Data::F32(v) }
    }

    pub fn i64(shape: Vec<usize>, v: Vec<i64>) -> Self {
        Tensor { shape, data: Data::I64(v) }
    }

    pub fn bool(shape: Vec<usize>, v: Vec<bool>) -> Self {
        Tensor { shape, data: Data::Bool(v) }
    }

    pub fn dtype(&self) -> DType {
        match self.data {
            Data::F32(_) => DType::F32,
            Data::I64(_) => DType::I64,
            Data::Bool(_) => DType::Bool,
        }
    }

    pub fn numel(&self) -> usize {
        numel(&self.shape)
    }

    pub fn data_len(&self) -> usize {
        self.data.len()
    }

    pub fn as_f32(&self) -> Result<&[f32]> {
        match &self.data {
            Data::F32(v) => Ok(v),
            _ => bail!("expected a float tensor, got {:?}", self.dtype()),
        }
    }

    pub fn as_i64(&self) -> Result<&[i64]> {
        match &self.data {
            Data::I64(v) => Ok(v),
            _ => bail!("expected an int64 tensor, got {:?}", self.dtype()),
        }
    }

    pub fn as_bool(&self) -> Result<&[bool]> {
        match &self.data {
            Data::Bool(v) => Ok(v),
            _ => bail!("expected a bool tensor, got {:?}", self.dtype()),
        }
    }

    /// Integer values of a shape-like tensor.
    pub fn ints(&self) -> Result<Vec<i64>> {
        Ok(self.as_i64()?.to_vec())
    }

    pub fn to_f32(&self) -> Vec<f32> {
        match &self.data {
            Data::F32(v) => v.clone(),
            Data::I64(v) => v.iter().map(|&x| x as f32).collect(),
            Data::Bool(v) => v.iter().map(|&x| f32::from(u8::from(x))).collect(),
        }
    }

    pub fn reshaped(&self, shape: Vec<usize>) -> Tensor {
        Tensor { shape, data: self.data.clone() }
    }
}

pub fn read_npy(path: &std::path::Path) -> Result<Tensor> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    ensure!(bytes.starts_with(b"\x93NUMPY"), "{} is not a .npy file", path.display());
    let (header_len, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        _ => (u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, 12),
    };
    let header = std::str::from_utf8(&bytes[start..start + header_len])?;
    ensure!(header.contains("'<f4'"), "only float32 .npy files are read: {header}");
    ensure!(header.contains("'fortran_order': False"), "fortran order: {header}");
    let shape_text = header.split("'shape': (").nth(1).and_then(|s| s.split(')').next()).context("no shape in .npy header")?;
    let shape: Vec<usize> = shape_text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::parse).collect::<Result<_, _>>()?;
    let data: Vec<f32> = bytes[start + header_len..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    ensure!(data.len() == numel(&shape), "{} is truncated", path.display());
    Ok(Tensor::f32(shape, data))
}

pub fn write_npy(path: &std::path::Path, t: &Tensor) -> Result<()> {
    let shape: Vec<String> = t.shape.iter().map(|d| d.to_string()).collect();
    let shape = match shape.len() {
        1 => format!("{},", shape[0]),
        _ => shape.join(", "),
    };
    let mut header = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': ({shape}), }}");
    while (header.len() + 11) % 64 != 0 {
        header.push(' ');
    }
    header.push('\n');
    let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
    bytes.extend((header.len() as u16).to_le_bytes());
    bytes.extend(header.as_bytes());
    for v in t.to_f32() {
        bytes.extend(v.to_le_bytes());
    }
    std::fs::write(path, bytes)?;
    Ok(())
}
