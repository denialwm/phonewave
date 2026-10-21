//! ONNX reader. The protobuf wire format is decoded by hand, and only the
//! fields the two models use are read.

use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};

use crate::tensor::{Data, Tensor};

pub struct Model {
    pub opset: i64,
    pub nodes: Vec<Node>,
    pub initializers: Vec<(String, Tensor)>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub struct Node {
    pub op: String,
    pub name: String,
    /// Absent optional inputs are empty strings, as in the file.
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attrs: HashMap<String, Attr>,
}

#[derive(Clone, Debug)]
pub enum Attr {
    Int(i64),
    Float(f32),
    Ints(Vec<i64>),
    Floats(Vec<f32>),
    Str(String),
    Tensor(Tensor),
}

impl Node {
    pub fn int(&self, name: &str, default: i64) -> i64 {
        match self.attrs.get(name) {
            Some(Attr::Int(v)) => *v,
            _ => default,
        }
    }

    pub fn float(&self, name: &str, default: f32) -> f32 {
        match self.attrs.get(name) {
            Some(Attr::Float(v)) => *v,
            _ => default,
        }
    }

    pub fn ints(&self, name: &str) -> Option<&[i64]> {
        match self.attrs.get(name) {
            Some(Attr::Ints(v)) => Some(v),
            _ => None,
        }
    }

    pub fn string(&self, name: &str, default: &'static str) -> String {
        match self.attrs.get(name) {
            Some(Attr::Str(v)) => v.clone(),
            _ => default.to_owned(),
        }
    }

    pub fn label(&self) -> String {
        format!("{} {}", self.op, self.name)
    }
}

enum Wire<'a> {
    Varint(u64),
    Fixed64,
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Wire<'a> {
    fn int(&self) -> Result<i64> {
        match self {
            Wire::Varint(v) => Ok(*v as i64),
            _ => bail!("expected a varint"),
        }
    }

    fn bytes(&self) -> Result<&'a [u8]> {
        match self {
            Wire::Bytes(b) => Ok(b),
            _ => bail!("expected a length-delimited field"),
        }
    }

    fn string(&self) -> Result<String> {
        Ok(String::from_utf8(self.bytes()?.to_vec())?)
    }

    /// A repeated varint field, packed or not.
    fn ints(&self, out: &mut Vec<i64>) -> Result<()> {
        match self {
            Wire::Varint(v) => out.push(*v as i64),
            Wire::Bytes(b) => {
                let mut i = 0;
                while i < b.len() {
                    out.push(varint(b, &mut i)? as i64);
                }
            }
            _ => bail!("expected varints"),
        }
        Ok(())
    }

    /// A repeated float field, packed or not.
    fn floats(&self, out: &mut Vec<f32>) -> Result<()> {
        match self {
            Wire::Fixed32(v) => out.push(f32::from_bits(*v)),
            Wire::Bytes(b) => out.extend(b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap()))),
            _ => bail!("expected floats"),
        }
        Ok(())
    }
}

fn varint(b: &[u8], i: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i).context("truncated varint")?;
        *i += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    bail!("varint too long")
}

fn fields(b: &[u8]) -> impl Iterator<Item = Result<(u32, Wire<'_>)>> {
    spans(b).map(|f| f.map(|(number, wire, _)| (number, wire)))
}

/// Like `fields`, with each field's bytes, key included, for copying it.
fn spans(b: &[u8]) -> impl Iterator<Item = Result<(u32, Wire<'_>, &[u8])>> {
    let mut i = 0;
    std::iter::from_fn(move || {
        (i < b.len()).then(|| {
            let start = i;
            let key = varint(b, &mut i)?;
            let wire = match key & 7 {
                0 => Wire::Varint(varint(b, &mut i)?),
                1 => {
                    b.get(i..i + 8).context("truncated fixed64")?;
                    i += 8;
                    Wire::Fixed64
                }
                2 => {
                    let len = varint(b, &mut i)? as usize;
                    let v = b.get(i..i + len).context("truncated field")?;
                    i += len;
                    Wire::Bytes(v)
                }
                5 => {
                    let v = b.get(i..i + 4).context("truncated fixed32")?;
                    i += 4;
                    Wire::Fixed32(u32::from_le_bytes(v.try_into().unwrap()))
                }
                t => bail!("unsupported wire type {t}"),
            };
            Ok(((key >> 3) as u32, wire, &b[start..i]))
        })
    })
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_bytes(out: &mut Vec<u8>, number: u32, b: &[u8]) {
    put_varint(out, u64::from(number) << 3 | 2);
    put_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

/// The model with its float weights stored as float16, which halves the
/// file. The reader widens them back to f32, so only their precision
/// changes. Tensors under `HALVE_MIN` values stay f32: they are constants
/// such as scales and epsilons, which f16 could round to zero, and they
/// weigh nothing.
pub fn halve(model: &[u8]) -> Result<Vec<u8>> {
    rewrite(model, |n, graph| {
        (n == 7).then(|| {
            rewrite(graph, |n, b| match n {
                5 => Some(halve_tensor(b)),
                // Constant nodes hold their tensor in an attribute.
                1 => Some(rewrite(b, |n, attr| (n == 5).then(|| rewrite(attr, |n, t| (n == 5).then(|| halve_tensor(t)))))),
                _ => None,
            })
        })
    })
}

const HALVE_MIN: usize = 1024;

/// Copies a message, replacing each length-delimited field for which `f`
/// returns new bytes.
fn rewrite(b: &[u8], f: impl Fn(u32, &[u8]) -> Option<Result<Vec<u8>>>) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(b.len());
    for field in spans(b) {
        let (n, wire, bytes) = field?;
        match wire {
            Wire::Bytes(inner) => match f(n, inner) {
                Some(new) => put_bytes(&mut out, n, &new?),
                None => out.extend_from_slice(bytes),
            },
            _ => out.extend_from_slice(bytes),
        }
    }
    Ok(out)
}

fn halve_tensor(b: &[u8]) -> Result<Vec<u8>> {
    let (_, tensor) = tensor(b)?;
    let Data::F32(values) = &tensor.data else { return Ok(b.to_vec()) };
    if values.len() < HALVE_MIN || !values.iter().all(|v| v.abs() <= f32::from(half::f16::MAX)) {
        return Ok(b.to_vec());
    }
    // Everything but the type and the data, which follow as float16.
    let mut out = Vec::with_capacity(b.len() / 2 + 64);
    for field in spans(b) {
        let (n, _, bytes) = field?;
        if !matches!(n, 2 | 4 | 9) {
            out.extend_from_slice(bytes);
        }
    }
    put_varint(&mut out, 2 << 3);
    put_varint(&mut out, 10);
    let data: Vec<u8> = values.iter().flat_map(|&v| half::f16::from_f32(v).to_le_bytes()).collect();
    put_bytes(&mut out, 9, &data);
    Ok(out)
}

pub fn load(path: &std::path::Path) -> Result<Model> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<Model> {
    let mut opset = 0;
    let mut graph = None;
    for field in fields(bytes) {
        match field? {
            (7, w) => graph = Some(w.bytes()?),
            (8, w) => {
                let mut domain = String::new();
                let mut version = 0;
                for f in fields(w.bytes()?) {
                    match f? {
                        (1, w) => domain = w.string()?,
                        (2, w) => version = w.int()?,
                        _ => {}
                    }
                }
                if domain.is_empty() || domain == "ai.onnx" {
                    opset = version;
                }
            }
            _ => {}
        }
    }
    let mut model = Model { opset, nodes: Vec::new(), initializers: Vec::new(), inputs: Vec::new(), outputs: Vec::new() };
    for field in fields(graph.context("model has no graph")?) {
        match field? {
            (1, w) => model.nodes.push(node(w.bytes()?)?),
            (5, w) => model.initializers.push(tensor(w.bytes()?)?),
            (11, w) => model.inputs.push(value_name(w.bytes()?)?),
            (12, w) => model.outputs.push(value_name(w.bytes()?)?),
            _ => {}
        }
    }
    // Initializers may also be listed as graph inputs.
    let initialized: std::collections::HashSet<_> = model.initializers.iter().map(|(n, _)| n.clone()).collect();
    model.inputs.retain(|n| !initialized.contains(n));
    Ok(model)
}

fn value_name(b: &[u8]) -> Result<String> {
    for f in fields(b) {
        if let (1, w) = f? {
            return w.string();
        }
    }
    bail!("value info without a name")
}

fn node(b: &[u8]) -> Result<Node> {
    let mut node = Node { op: String::new(), name: String::new(), inputs: Vec::new(), outputs: Vec::new(), attrs: HashMap::new() };
    let mut domain = String::new();
    for f in fields(b) {
        match f? {
            (1, w) => node.inputs.push(w.string()?),
            (2, w) => node.outputs.push(w.string()?),
            (3, w) => node.name = w.string()?,
            (4, w) => node.op = w.string()?,
            (5, w) => {
                let (name, attr) = attribute(w.bytes()?)?;
                node.attrs.insert(name, attr);
            }
            (7, w) => domain = w.string()?,
            _ => {}
        }
    }
    ensure!(domain.is_empty() || domain == "ai.onnx", "{} is in the unsupported domain {domain}", node.op);
    Ok(node)
}

fn attribute(b: &[u8]) -> Result<(String, Attr)> {
    let mut name = String::new();
    let mut kind = 0;
    let (mut f, mut i, mut s, mut t) = (0.0, 0, String::new(), None);
    let (mut floats, mut ints) = (Vec::new(), Vec::new());
    for field in fields(b) {
        match field? {
            (1, w) => name = w.string()?,
            (2, Wire::Fixed32(v)) => f = f32::from_bits(v),
            (3, w) => i = w.int()?,
            (4, w) => s = w.string()?,
            (5, w) => t = Some(tensor(w.bytes()?)?.1),
            (7, w) => w.floats(&mut floats)?,
            (8, w) => w.ints(&mut ints)?,
            (20, w) => kind = w.int()?,
            _ => {}
        }
    }
    let attr = match kind {
        1 => Attr::Float(f),
        2 => Attr::Int(i),
        3 => Attr::Str(s),
        4 => Attr::Tensor(t.context("tensor attribute without a tensor")?),
        6 => Attr::Floats(floats),
        7 => Attr::Ints(ints),
        k => bail!("attribute {name} has unsupported type {k}"),
    };
    Ok((name, attr))
}

fn tensor(b: &[u8]) -> Result<(String, Tensor)> {
    let mut name = String::new();
    let mut dims = Vec::new();
    let mut dtype = 0;
    let mut raw: Option<&[u8]> = None;
    let mut floats = Vec::new();
    let mut ints = Vec::new();
    for f in fields(b) {
        match f? {
            (1, w) => w.ints(&mut dims)?,
            (2, w) => dtype = w.int()?,
            (4, w) => w.floats(&mut floats)?,
            (5 | 7, w) => w.ints(&mut ints)?,
            (8, w) => name = w.string()?,
            (9, w) => raw = Some(w.bytes()?),
            (13 | 14, _) => bail!("tensor {name} keeps its data outside the model"),
            _ => {}
        }
    }
    let shape: Vec<usize> = dims.iter().map(|&d| d as usize).collect();
    let data = match (dtype, raw) {
        (1, Some(r)) => Data::F32(r.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()),
        (1, None) => Data::F32(floats),
        (10, Some(r)) => Data::F32(r.chunks_exact(2).map(|c| half::f16::from_le_bytes(c.try_into().unwrap()).to_f32()).collect()),
        (10, None) => Data::F32(ints.iter().map(|&v| half::f16::from_bits(v as u16).to_f32()).collect()),
        (7, Some(r)) => Data::I64(r.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect()),
        (6, Some(r)) => Data::I64(r.chunks_exact(4).map(|c| i64::from(i32::from_le_bytes(c.try_into().unwrap()))).collect()),
        (6 | 7, None) => Data::I64(ints),
        (9, Some(r)) => Data::Bool(r.iter().map(|&v| v != 0).collect()),
        (9, None) => Data::Bool(ints.iter().map(|&v| v != 0).collect()),
        (t, _) => bail!("tensor {name} has unsupported data type {t}"),
    };
    let tensor = Tensor { shape, data };
    ensure!(tensor.data_len() == tensor.numel(), "tensor {name} holds {} values for shape {:?}", tensor.data_len(), tensor.shape);
    Ok((name, tensor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tensor_proto(name: &str, values: &[f32]) -> Vec<u8> {
        let mut t = Vec::new();
        put_varint(&mut t, 1 << 3);
        put_varint(&mut t, values.len() as u64);
        put_varint(&mut t, 2 << 3);
        put_varint(&mut t, 1);
        put_bytes(&mut t, 8, name.as_bytes());
        put_bytes(&mut t, 9, &values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
        t
    }

    #[test]
    fn halve_rounds_weights_and_keeps_small_constants() {
        let big: Vec<f32> = (0..HALVE_MIN).map(|i| i as f32 / 7.0).collect();
        let small = [1e-12f32, 0.5, 3.0];
        // A weight, a small constant, and a Constant node holding a weight.
        let mut attr = Vec::new();
        put_bytes(&mut attr, 1, b"value");
        put_bytes(&mut attr, 5, &tensor_proto("c", &big));
        put_varint(&mut attr, 20 << 3);
        put_varint(&mut attr, 4);
        let mut node = Vec::new();
        put_bytes(&mut node, 2, b"c");
        put_bytes(&mut node, 4, b"Constant");
        put_bytes(&mut node, 5, &attr);
        let mut graph = Vec::new();
        put_bytes(&mut graph, 1, &node);
        put_bytes(&mut graph, 5, &tensor_proto("w", &big));
        put_bytes(&mut graph, 5, &tensor_proto("eps", &small));
        let mut model = Vec::new();
        put_bytes(&mut model, 7, &graph);

        let halved = halve(&model).unwrap();
        assert!(halved.len() < model.len() * 6 / 10, "{} of {} bytes", halved.len(), model.len());
        let m = parse(&halved).unwrap();
        let rounded = |t: &Tensor| match &t.data {
            Data::F32(v) => v.iter().zip(&big).all(|(a, b)| (a - b).abs() <= b.abs() / 1024.0),
            _ => false,
        };
        assert!(rounded(&m.initializers[0].1));
        assert!(matches!(&m.nodes[0].attrs["value"], Attr::Tensor(t) if rounded(t)));
        assert!(matches!(&m.initializers[1].1.data, Data::F32(v) if v == &small));
    }
}
