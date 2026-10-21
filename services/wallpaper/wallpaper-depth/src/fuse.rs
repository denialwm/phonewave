//! Graph rewrites before planning.
//!
//! BiRefNet's export spells each modulated deformable convolution as about
//! sixty nodes: the sample positions (base grid plus offsets), four GatherND
//! corner samples from the padded input with bilinear weights, the modulator
//! mask, and a k-strided Conv over the samples laid out as an image k times
//! larger. At 256x256 with k = 7 that image alone is 800 MB. The rewrite
//! keeps the nodes that compute the positions and the mask and replaces the
//! rest with one DeformConv node that samples inside the convolution.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::onnx::{Attr, Model, Node};
use crate::reference;
use crate::tensor::Tensor;

struct Graph<'a> {
    producer: HashMap<&'a str, &'a Node>,
    consts: HashMap<&'a str, Tensor>,
}

impl<'a> Graph<'a> {
    fn new(model: &'a Model) -> Result<Self> {
        let mut producer = HashMap::new();
        let mut consts: HashMap<&str, Tensor> = model.initializers.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
        for node in &model.nodes {
            for o in &node.outputs {
                producer.insert(o.as_str(), node);
            }
            if node.op == "Constant" {
                consts.insert(node.outputs[0].as_str(), reference::eval(node, &[], model.opset)?.remove(0));
            }
        }
        Ok(Graph { producer, consts })
    }

    /// The node producing `name` if it is an `op`.
    fn from(&self, name: &str, op: &str) -> Option<&'a Node> {
        self.producer.get(name).copied().filter(|n| n.op == op)
    }

    fn int(&self, name: &str) -> Option<i64> {
        self.consts.get(name)?.ints().ok().filter(|v| v.len() == 1).map(|v| v[0])
    }

    /// Follows Reshape and Transpose nodes back to the first other node.
    fn through_views(&self, mut name: &'a str) -> &'a str {
        while let Some(n) = self.producer.get(name).filter(|n| matches!(n.op.as_str(), "Reshape" | "Transpose")) {
            name = n.inputs[0].as_str();
        }
        name
    }

    /// The Clip bound and the channel of the sample positions behind one
    /// GatherND index component: Reshape(Clip(Slice(Cast(Floor(pos))))),
    /// possibly with +1 between Slice and Clip.
    fn index_component(&self, name: &str) -> Option<(i64, i64, &'a str)> {
        let clip = self.from(self.through_views(name), "Clip")?;
        let (lo, hi) = (self.int(&clip.inputs[1])?, self.int(&clip.inputs[2])?);
        let mut src = clip.inputs[0].as_str();
        if let Some(add) = self.from(src, "Add") {
            src = add.inputs.iter().find(|i| !self.consts.contains_key(i.as_str()))?;
        }
        let slice = self.from(src, "Slice")?;
        let start = self.consts.get(slice.inputs[1].as_str())?.ints().ok()?;
        let floor = self.from(&self.from(&slice.inputs[0], "Cast")?.inputs[0], "Floor")?;
        (lo == 0 && start.len() == 1).then_some((hi, start[0], floor.inputs[0].as_str()))
    }

    /// The DeformConv replacing `conv`, if `conv` ends a deformable block.
    fn deform(&self, conv: &Node) -> Option<Node> {
        let [k1, k2] = conv.ints("kernel_shape")? else { return None };
        let k = *k1;
        if k != *k2 || conv.ints("strides")? != [k, k] || conv.ints("pads").is_some_and(|p| p.iter().any(|&p| p != 0)) {
            return None;
        }
        let modulated = self.from(self.through_views(&conv.inputs[0]), "Mul")?;
        let (sum, mask) = match self.from(&modulated.inputs[0], "Sum") {
            Some(s) => (s, &modulated.inputs[1]),
            None => (self.from(&modulated.inputs[1], "Sum")?, &modulated.inputs[0]),
        };
        let mask = self.through_views(mask);
        let corner = self.from(&sum.inputs[0], "Mul")?;
        let gather = corner.inputs.iter().find_map(|i| self.from(self.through_views(i), "GatherND"))?;
        let pad = self.from(self.through_views(&gather.inputs[0]), "Pad")?;
        let pads = self.consts.get(pad.inputs[1].as_str())?.ints().ok()?;
        let index = self.from(&gather.inputs[1], "Concat")?;
        let (hy, cy, pos) = self.index_component(&index.inputs[0])?;
        let (hx, cx, pos_x) = self.index_component(&index.inputs[1])?;
        if pos != pos_x || (cy, cx) != (0, 1) || pads.len() != 8 || sum.inputs.len() != 4 {
            return None;
        }
        let mut inputs = vec![pad.inputs[0].clone(), pos.to_owned(), mask.to_owned(), conv.inputs[1].clone()];
        inputs.extend(conv.inputs.get(2).cloned());
        let attrs = HashMap::from([
            ("kernel".to_owned(), Attr::Int(k)),
            ("pads".to_owned(), Attr::Ints(vec![pads[2], pads[3]])),
            ("bounds".to_owned(), Attr::Ints(vec![hy, hx])),
        ]);
        Some(Node { op: "DeformConv".into(), name: conv.name.clone(), inputs, outputs: conv.outputs.clone(), attrs })
    }
}

/// Replaces every deformable block and drops the nodes nothing reads any
/// more. Returns how many blocks were replaced.
pub fn deform_convs(model: &mut Model) -> Result<usize> {
    let replaced: Vec<(usize, Node)> = {
        let graph = Graph::new(model)?;
        model.nodes.iter().enumerate().filter(|(_, n)| n.op == "Conv").filter_map(|(i, n)| graph.deform(n).map(|d| (i, d))).collect()
    };
    let count = replaced.len();
    for (i, node) in replaced {
        model.nodes[i] = node;
    }
    let mut needed: HashSet<String> = model.outputs.iter().cloned().collect();
    let mut keep = vec![false; model.nodes.len()];
    for (i, node) in model.nodes.iter().enumerate().rev() {
        if node.outputs.iter().any(|o| needed.contains(o)) {
            keep[i] = true;
            needed.extend(node.inputs.iter().filter(|n| !n.is_empty()).cloned());
        }
    }
    let mut flags = keep.into_iter();
    model.nodes.retain(|_| flags.next().unwrap());
    Ok(count)
}
