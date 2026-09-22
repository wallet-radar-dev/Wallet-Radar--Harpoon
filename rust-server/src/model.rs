//! Port of server/model.js — loads the trained classifier from
//! dataset/model.json and classifies a feature object into one of the entity
//! classes (burn / contract / exchange / fund / individual / mm).
//!
//! Two model shapes are supported, exactly as the JS switches on `M.type`:
//!   - "rf": a random forest — walk each tree to a leaf, average the leaf
//!     probability vectors.
//!   - anything else (logistic): a softmax over `W` linear scores.
//!
//! Feature values are coerced with JS `Number(x) || 0` semantics, optionally
//! `log1p`-compressed, then standardised with the stored mean/std. The public
//! outputs (label, confidence, per-class %) are rounded with `toFixed(1)` and
//! must match Node exactly.

use crate::jsmath::round_fixed;
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Deserialize)]
pub struct Model {
    #[serde(rename = "type")]
    kind: String,
    classes: Vec<String>,
    features: Vec<String>,
    log: Vec<bool>,
    mean: Vec<f64>,
    std: Vec<f64>,
    #[serde(default)]
    trees: Vec<Node>,
    #[serde(default, rename = "W")]
    w: Vec<Vec<f64>>,
}

/// A decision-tree node: either an internal split (`feat`/`thr`/`left`/`right`)
/// or a leaf holding a class-probability vector. `untagged` matches the JS
/// `while (!n.probs)` walk — a node is a leaf iff it carries `probs`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Node {
    Leaf { probs: Vec<f64> },
    Split { feat: usize, thr: f64, left: Box<Node>, right: Box<Node> },
}

impl Node {
    /// `let n = t; while (!n.probs) n = x[n.feat] <= n.thr ? n.left : n.right;`
    fn walk<'a>(&'a self, x: &[f64]) -> &'a [f64] {
        let mut n = self;
        loop {
            match n {
                Node::Leaf { probs } => return probs,
                Node::Split { feat, thr, left, right } => {
                    n = if x[*feat] <= *thr { left } else { right };
                }
            }
        }
    }
}

impl Model {
    /// `JSON.parse(readFileSync(...))`; a missing or malformed file leaves ML
    /// disabled (`ready()` false), so callers get `None` and skip classification.
    pub fn load(dataset_dir: &std::path::Path) -> Option<Model> {
        let text = std::fs::read_to_string(dataset_dir.join("model.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// model.js `classify(feat)`. Returns the `{label, confidence, probs}` object
    /// verbatim; key order in `probs` follows `classes`, matching Node.
    pub fn classify(&self, feat: &Value) -> Value {
        // x = M.features.map((f,i) => standardise(Number(feat[f])||0, log[i]))
        let x: Vec<f64> = self
            .features
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut v = number_or_zero(feat.get(f));
                if self.log[i] {
                    v = v.max(0.0).ln_1p();
                }
                (v - self.mean[i]) / self.std[i]
            })
            .collect();

        let p: Vec<f64> = if self.kind == "rf" {
            let n = self.classes.len();
            let mut acc = vec![0.0f64; n];
            for t in &self.trees {
                let probs = t.walk(&x);
                for (c, pr) in probs.iter().enumerate() {
                    acc[c] += pr;
                }
            }
            let ntrees = self.trees.len() as f64;
            acc.iter().map(|v| v / ntrees).collect()
        } else {
            // z = W.map(w => w[0] + Σ x[j]*w[j+1]); softmax(z)
            let z: Vec<f64> = self
                .w
                .iter()
                .map(|w| w[0] + x.iter().enumerate().map(|(j, v)| v * w[j + 1]).sum::<f64>())
                .collect();
            let m = z.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let e: Vec<f64> = z.iter().map(|v| (v - m).exp()).collect();
            let sum: f64 = e.iter().sum();
            e.iter().map(|v| v / sum).collect()
        };

        // argmax: first index of the maximum (`p[c] > p[a]` keeps the earlier one on ties).
        let mut a = 0;
        for c in 1..p.len() {
            if p[c] > p[a] {
                a = c;
            }
        }

        let mut probs = Map::new();
        for (i, c) in self.classes.iter().enumerate() {
            probs.insert(c.clone(), crate::jsmath::js_num(round_fixed(p[i] * 100.0, 1)));
        }

        Value::Object({
            let mut o = Map::new();
            o.insert("label".into(), Value::String(self.classes[a].clone()));
            o.insert("confidence".into(), crate::jsmath::js_num(round_fixed(p[a] * 100.0, 1)));
            o.insert("probs".into(), Value::Object(probs));
            o
        })
    }
}

/// `Number(x) || 0` for a JSON value (or a missing key → `undefined`).
///
/// `Number(x)` coerces per the ECMAScript ToNumber rules; the trailing `|| 0`
/// then maps every falsy result (`0`, `-0`, `NaN`) to `0`, so only a finite
/// non-zero number survives.
fn number_or_zero(v: Option<&Value>) -> f64 {
    let n = match v {
        None => f64::NAN, // undefined
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => if *b { 1.0 } else { 0.0 },
        Some(Value::Number(x)) => x.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_string_to_number(s),
        Some(Value::Array(a)) => match a.as_slice() {
            [] => 0.0,
            [x] => number_or_zero(Some(x)),
            _ => f64::NAN,
        },
        Some(Value::Object(_)) => f64::NAN,
    };
    if n == 0.0 || n.is_nan() { 0.0 } else { n }
}

/// ToNumber for strings: trim JS whitespace, empty → 0, then decimal / hex /
/// Infinity as V8 accepts them; anything else → NaN.
fn js_string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).map(|n| n as f64).unwrap_or(f64::NAN);
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn classify_matches_node() {
        // Golden generated from the real dataset/model.json (an rf model) over a
        // spread of feature vectors + edge cases (all-zero, empty object, strings
        // and negatives). Rounded outputs must match V8 exactly.
        let model = Model::load(Path::new("../dataset")).expect("dataset/model.json");
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/model-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/model-golden.json")).unwrap();

        for (i, feat) in inputs.as_array().unwrap().iter().enumerate() {
            let got = model.classify(feat);
            assert_eq!(got, golden[i], "case {i} feat={feat}");
        }
    }
}
