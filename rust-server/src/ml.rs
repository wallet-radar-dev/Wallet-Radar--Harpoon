//! Port of the pure core of server/ml-smart-money.js — logistic regression
//! train/prob + the honest evaluation stack (out-of-fold pooling, AUC, and the
//! permuted-label null baseline). Feature extraction and labelling are network-bound
//! and live elsewhere. The accumulation order (samples in dataset order, features
//! in FEATS order) is replicated exactly so 400 epochs of exp() track V8 to within
//! ~1 ULP; user-facing outputs are rounded and must match exactly.
//!
//! Evaluation changed in 2026-07: per-fold averaging of balanced accuracy was replaced
//! by metrics over POOLED out-of-fold predictions (stable when the rare class is small),
//! AUC was added (threshold-free), and every score now ships with the same CV re-run on
//! shuffled labels. That null baseline is the whole point: on this dataset the procedure
//! alone scores ~51%, so a "72%" without it is not a result.

use crate::jsmath::{js_round, round_fixed};
pub use crate::smart_money::FEATS;

pub struct Model {
    pub w: Vec<f64>,
    pub b: f64,
    pub mean: Vec<f64>,
    pub std: Vec<f64>,
}

pub struct Sample {
    pub x: Vec<f64>,
    pub y: u8,
}

/// ml-smart-money.js:56-81. Full-batch GD, class-weighted, L2 on weights only.
pub fn train(data: &[Sample]) -> Model {
    train_params(data, 400, 0.12, 0.01)
}

fn train_params(data: &[Sample], epochs: usize, lr: f64, lambda: f64) -> Model {
    let n = data.len();
    let dim = data[0].x.len();
    let pos = data.iter().filter(|d| d.y == 1).count();
    let neg = n - pos;
    let w_pos = if pos > 0 { n as f64 / (2.0 * pos as f64) } else { 1.0 };
    let w_neg = if neg > 0 { n as f64 / (2.0 * neg as f64) } else { 1.0 };

    let mut mean = vec![0.0; dim];
    for d in data {
        for j in 0..dim {
            mean[j] += d.x[j];
        }
    }
    for m in &mut mean {
        *m /= n as f64;
    }
    let mut std = vec![0.0; dim];
    for d in data {
        for j in 0..dim {
            std[j] += (d.x[j] - mean[j]).powi(2);
        }
    }
    for s in &mut std {
        // `Math.sqrt(...) || 1` — a zero std (constant feature) becomes 1.
        *s = (*s / n as f64).sqrt();
        if *s == 0.0 {
            *s = 1.0;
        }
    }
    let xs: Vec<Vec<f64>> = data
        .iter()
        .map(|d| (0..dim).map(|j| (d.x[j] - mean[j]) / std[j]).collect())
        .collect();

    let mut w = vec![0.0; dim];
    let mut b = 0.0;
    for _ in 0..epochs {
        let mut gw = vec![0.0; dim];
        let mut gb = 0.0;
        let mut wsum = 0.0;
        for i in 0..n {
            let mut z = b;
            for j in 0..dim {
                z += w[j] * xs[i][j];
            }
            let cw = if data[i].y == 1 { w_pos } else { w_neg };
            let err = (1.0 / (1.0 + (-z).exp()) - data[i].y as f64) * cw;
            for j in 0..dim {
                gw[j] += err * xs[i][j];
            }
            gb += err;
            wsum += cw;
        }
        for j in 0..dim {
            w[j] -= lr * (gw[j] / wsum + lambda * w[j]);
        }
        b -= lr * gb / wsum;
    }
    Model { w, b, mean, std }
}

/// ml-smart-money.js:82-85
pub fn prob(m: &Model, x: &[f64]) -> f64 {
    let mut z = m.b;
    for j in 0..m.w.len() {
        let s = if m.std[j] == 0.0 { 1.0 } else { m.std[j] };
        z += m.w[j] * ((x[j] - m.mean[j]) / s);
    }
    1.0 / (1.0 + (-z).exp())
}

/// `balAccFrom(pairs)` — mean of per-class recall at the 0.5 threshold, over
/// (probability, label) pairs. Fed out-of-fold predictions, never in-sample ones.
pub fn bal_acc_from(pairs: &[(f64, u8)]) -> f64 {
    let (mut tp, mut tn, mut pos, mut neg) = (0.0, 0.0, 0.0, 0.0);
    for &(p, y) in pairs {
        let c = if p >= 0.5 { 1 } else { 0 };
        if y == 1 {
            pos += 1.0;
            if c == 1 {
                tp += 1.0;
            }
        } else {
            neg += 1.0;
            if c == 0 {
                tn += 1.0;
            }
        }
    }
    ((if pos > 0.0 { tp / pos } else { 0.0 }) + (if neg > 0.0 { tn / neg } else { 0.0 })) / 2.0
}

/// `aucFrom(pairs)` — Mann–Whitney AUC with ties given their average rank. Threshold-free,
/// so it still shows signal where balanced accuracy is hostage to the arbitrary 0.5 cut.
pub fn auc_from(pairs: &[(f64, u8)]) -> Option<f64> {
    let p_count = pairs.iter().filter(|&&(_, y)| y == 1).count();
    let n_count = pairs.len() - p_count;
    if p_count == 0 || n_count == 0 {
        return None;
    }
    let mut s: Vec<(f64, u8)> = pairs.to_vec();
    s.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)); // stable, like JS sort
    let mut rank = vec![0.0; s.len()];
    let mut i = 0;
    while i < s.len() {
        let mut j = i;
        while j + 1 < s.len() && s[j + 1].0 == s[i].0 {
            j += 1;
        }
        let avg = (i + j + 2) as f64 / 2.0;
        for r in rank.iter_mut().take(j + 1).skip(i) {
            *r = avg;
        }
        i = j + 1;
    }
    let sum_pos: f64 = (0..s.len()).filter(|&k| s[k].1 == 1).map(|k| rank[k]).sum();
    let (p, n) = (p_count as f64, n_count as f64);
    Some((sum_pos - p * (p + 1.0) / 2.0) / (p * n))
}

/// Stratified deterministic order (interleave pos/neg) — no RNG, so folds are reproducible.
fn strat_order(data: &[Sample]) -> Vec<&Sample> {
    let pos: Vec<&Sample> = data.iter().filter(|d| d.y == 1).collect();
    let neg: Vec<&Sample> = data.iter().filter(|d| d.y == 0).collect();
    let mut order: Vec<&Sample> = Vec::with_capacity(data.len());
    for i in 0..pos.len().max(neg.len()) {
        if i < pos.len() {
            order.push(pos[i]);
        }
        if i < neg.len() {
            order.push(neg[i]);
        }
    }
    order
}

/// `oofPairs(data, kmax)` — every sample scored by a fold that did NOT see it. Metrics are
/// computed over the pooled result rather than averaged per fold, which keeps them stable
/// when the positive class is small (here: 110 of 890).
pub fn oof_pairs(data: &[Sample], kmax: usize) -> Option<Vec<(f64, u8)>> {
    let pos = data.iter().filter(|d| d.y == 1).count();
    let neg = data.len() - pos;
    let k = kmax.min(pos).min(neg);
    if k < 2 {
        return None;
    }
    let order = strat_order(data);
    let mut pairs: Vec<(f64, u8)> = Vec::with_capacity(data.len());
    for kk in 0..k {
        let test: Vec<&Sample> = order.iter().enumerate().filter(|(i, _)| i % k == kk).map(|(_, s)| *s).collect();
        let tr: Vec<Sample> = order.iter().enumerate().filter(|(i, _)| i % k != kk).map(|(_, s)| Sample { x: s.x.clone(), y: s.y }).collect();
        if test.is_empty() || !tr.iter().any(|d| d.y == 1) || !tr.iter().any(|d| d.y == 0) {
            continue;
        }
        let m = train(&tr);
        for s in test {
            pairs.push((prob(&m, &s.x), s.y));
        }
    }
    if pairs.is_empty() {
        None
    } else {
        Some(pairs)
    }
}

/// `nullBaseline(data, kmax, reps)` — the SAME cross-validation on SHUFFLED labels: how much
/// "accuracy" the procedure manufactures out of nothing (class imbalance + weighting + the 0.5
/// threshold). Without this number `cvAcc` is uninterpretable.
///
/// The shuffle is a Fisher–Yates driven by the same LCG as JS (`Math.imul(seed, 1664525) +
/// 1013904223 >>> 0`), so Node and Rust permute identically and the baselines match exactly.
pub fn null_baseline(data: &[Sample], kmax: usize, reps: usize) -> Option<(f64, f64)> {
    let (mut acc_sum, mut auc_sum, mut ok) = (0.0, 0.0, 0usize);
    for r in 0..reps {
        let mut ys: Vec<u8> = data.iter().map(|d| d.y).collect();
        let mut seed: u32 = (1013904223u64 + r as u64 * 2654435761u64) as u32; // JS ToUint32
        for i in (1..ys.len()).rev() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let j = (seed % (i as u32 + 1)) as usize;
            ys.swap(i, j);
        }
        let shuffled: Vec<Sample> = data.iter().enumerate().map(|(i, d)| Sample { x: d.x.clone(), y: ys[i] }).collect();
        let Some(pairs) = oof_pairs(&shuffled, kmax) else { continue };
        acc_sum += bal_acc_from(&pairs);
        auc_sum += auc_from(&pairs).unwrap_or(0.5);
        ok += 1;
    }
    if ok == 0 {
        return None;
    }
    Some((acc_sum / ok as f64, auc_sum / ok as f64))
}

/// The activation gate from retrain(): a model ships only if it beats its own null baseline
/// by `GATE_ACC_MARGIN` points AND clears `GATE_MIN_AUC`. Otherwise the UI shows no number.
pub const GATE_ACC_MARGIN: i64 = 5;
pub const GATE_MIN_AUC: f64 = 0.60;
pub fn passes_gate(cv_acc: i64, cv_auc: f64, null_acc: i64) -> bool {
    cv_acc >= null_acc + GATE_ACC_MARGIN && cv_auc >= GATE_MIN_AUC
}

/// mlStatus importance: `{f, w: +w.toFixed(2)}` sorted by |w| desc (stable), top 5.
pub fn importance(w: &[f64]) -> Vec<(&'static str, f64)> {
    let mut items: Vec<(&'static str, f64)> = FEATS.iter().enumerate().map(|(j, f)| (*f, round_fixed(w[j], 2))).collect();
    items.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
    items.truncate(5);
    items
}

/// The full retrain() metric block, in one call — mirrors what Node writes to sm-model.json.
pub struct Metrics {
    pub in_sample: i64,
    pub cv_acc: Option<i64>,
    pub cv_auc: Option<f64>,
    pub null_acc: Option<i64>,
    pub null_auc: Option<f64>,
    pub active: bool,
}
pub fn evaluate(data: &[Sample], m: &Model) -> Metrics {
    let in_pairs: Vec<(f64, u8)> = data.iter().map(|d| (prob(m, &d.x), d.y)).collect();
    let in_sample = js_round(bal_acc_from(&in_pairs) * 100.0) as i64;
    let Some(pairs) = oof_pairs(data, 5) else {
        return Metrics { in_sample, cv_acc: None, cv_auc: None, null_acc: None, null_auc: None, active: false };
    };
    let cv_acc = js_round(bal_acc_from(&pairs) * 100.0) as i64;
    let cv_auc = round_fixed(auc_from(&pairs).unwrap_or(0.5), 3);
    let (null_acc, null_auc) = match null_baseline(data, 5, 5) {
        Some((a, u)) => (Some(js_round(a * 100.0) as i64), Some(round_fixed(u, 3))),
        None => (None, None),
    };
    let active = null_acc.map(|na| passes_gate(cv_acc, cv_auc, na)).unwrap_or(false);
    Metrics { in_sample, cv_acc: Some(cv_acc), cv_auc: Some(cv_auc), null_acc, null_auc, active }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn load() -> Option<(Vec<Sample>, Value)> {
        let Ok(input) = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ml-input.json")) else {
            eprintln!("SKIP: golden-фікстура ml-input.json недоступна");
            return None;
        };
        let input: Value = serde_json::from_str(&input).unwrap();
        let data = input
            .as_array()
            .unwrap()
            .iter()
            .map(|d| Sample {
                x: d["x"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect(),
                y: d["y"].as_u64().unwrap() as u8,
            })
            .collect();
        let Ok(golden) = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ml-golden.json")) else {
            eprintln!("SKIP: golden-фікстура ml-golden.json недоступна");
            return None;
        };
        let golden: Value = serde_json::from_str(&golden).unwrap();
        Some((data, golden))
    }

    #[test]
    fn train_matches_node() {
        let Some((data, g)) = load() else { return };
        let m = train(&data);

        // mean/std/w/b carry sub-ULP FP noise vs V8 (reduction order, exp()) →
        // relative tolerance. The rounded user-facing outputs are asserted exact
        // in rounded_metrics_exact, which is the parity guarantee that matters.
        let close = |got: f64, want: f64, what: &str| {
            assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{what}: {got} vs {want}");
        };
        for (j, want) in g["mean"].as_array().unwrap().iter().enumerate() {
            close(m.mean[j], want.as_f64().unwrap(), &format!("mean[{j}]"));
        }
        for (j, want) in g["std"].as_array().unwrap().iter().enumerate() {
            close(m.std[j], want.as_f64().unwrap(), &format!("std[{j}]"));
        }
        // w/b: 400 epochs of exp() may drift ~1 ULP vs V8 → relative tolerance.
        let b_want = g["b"].as_f64().unwrap();
        assert!((m.b - b_want).abs() <= 1e-9 * b_want.abs().max(1.0), "b: {} vs {}", m.b, b_want);
        for (j, want) in g["w"].as_array().unwrap().iter().enumerate() {
            let ww = want.as_f64().unwrap();
            assert!((m.w[j] - ww).abs() <= 1e-9 * ww.abs().max(1.0), "w[{j}]: {} vs {}", m.w[j], ww);
        }
    }

    #[test]
    fn rounded_metrics_exact() {
        let Some((data, g)) = load() else { return };
        let m = train(&data);
        let e = evaluate(&data, &m);
        // These are what the UI shows — must match Node exactly.
        assert_eq!(e.in_sample, g["balAcc"].as_i64().unwrap(), "in-sample balAcc");
        assert_eq!(e.cv_acc, Some(g["cvAcc"].as_i64().unwrap()), "cvAcc");
        assert_eq!(e.cv_auc, Some(g["cvAuc"].as_f64().unwrap()), "cvAuc");
        assert_eq!(e.active, g["active"].as_bool().unwrap(), "activation gate");

        let imp = importance(&m.w);
        let want = g["importance"].as_array().unwrap();
        assert_eq!(imp.len(), want.len());
        for (got, w) in imp.iter().zip(want) {
            assert_eq!(got.0, w["f"].as_str().unwrap());
            assert_eq!(got.1, w["w"].as_f64().unwrap());
        }
    }

    /// The null baseline is the load-bearing number in this module: it is what exposed the
    /// graph features as an artifact. Its LCG shuffle must match Node bit-for-bit, or the
    /// two servers would disagree on whether a model is allowed to ship at all.
    #[test]
    fn null_baseline_matches_node() {
        let Some((data, g)) = load() else { return };
        let (acc, auc) = null_baseline(&data, 5, 5).expect("baseline");
        assert_eq!(js_round(acc * 100.0) as i64, g["nullAcc"].as_i64().unwrap(), "nullAcc");
        assert_eq!(round_fixed(auc, 3), g["nullAuc"].as_f64().unwrap(), "nullAuc");
    }

    /// A model that only matches its null baseline must not reach users.
    #[test]
    fn gate_rejects_a_model_that_does_not_beat_chance() {
        assert!(!passes_gate(53, 0.54, 51), "53% vs a 51% baseline is noise");
        assert!(!passes_gate(67, 0.55, 51), "AUC below 0.60 fails even with a wide acc margin");
        assert!(passes_gate(67, 0.725, 51), "the real de-leaked model ships");
    }
}
