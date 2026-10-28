//! Port of the prediction path of server/ml-smart-money.js — the "smart money"
//! logistic-regression screener. Given cheap structural features (behaviour flow,
//! token holdings) it returns P(this wallet is historically profitable).
//!
//! It is a SCREENER, not a forecaster: the JS label is realized PnL taken from the
//! same snapshot as the features, so the model is a cheap proxy for the expensive
//! computeWalletScore — never a claim about future returns. The forward-looking
//! track (sm-forward.json) is measured separately and owned by Node.
//!
//! `sm_features` (the 8-feature vector) and `sm_prob` (standardized logistic) are
//! pure and golden-tested against Node. The model is loaded lazily from the dataset
//! dir, and only if it passed the activation gate; fetch orchestration is in main.rs.

use crate::jsmath::js_num;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

/// ml-smart-money.js `prob(m, x)` — logistic on standardized features; a zero (or
/// NaN) std falls back to 1, matching `m.std[j] || 1`.
pub fn sm_prob(w: &[f64], b: f64, mean: &[f64], std: &[f64], x: &[f64]) -> f64 {
    let mut z = b;
    for j in 0..w.len() {
        let s = if std[j] == 0.0 || std[j].is_nan() { 1.0 } else { std[j] };
        z += w[j] * ((x[j] - mean[j]) / s);
    }
    1.0 / (1.0 + (-z).exp())
}

/// ml-smart-money.js `smFeatures(flow, tokens)` reduction (post-fetch). `flow` is
/// the extractFlow result (object, or Null on failure); `tokens` the flattened
/// tokenBalances (`{symbol, valueUsd}`) across eth/base/arbitrum. Order matches `FEATS`.
///
/// The crawler graph features (`log_gdeg`/`log_gvol`) were REMOVED in 2026-07: they are
/// both 0 for any address the BFS crawler has not reached, so they encoded crawl coverage
/// rather than wallet behaviour, and carried 69 of the model's claimed 73% accuracy.
/// `timespanDays` and `out/in` are now log-scaled like every other heavy-tailed feature.
pub fn sm_features(flow: &Value, tokens: &[Value]) -> Vec<f64> {
    let mut total_usd = 0.0;
    let mut syms: HashSet<String> = HashSet::new();
    for tk in tokens {
        total_usd += tk["valueUsd"].as_f64().unwrap_or(0.0);
        if let Some(s) = tk["symbol"].as_str() {
            syms.insert(s.to_string());
        }
    }
    // flow?.<k> || 0 — Null flow or missing key → 0.
    let fo = |k: &str| if flow.is_object() { flow[k].as_f64().unwrap_or(0.0) } else { 0.0 };
    let in_usd = fo("inUsd");
    let out_usd = fo("outUsd");
    vec![
        in_usd.ln_1p(),
        out_usd.ln_1p(),
        fo("txTotal").ln_1p(),
        fo("chains"),
        fo("timespanDays").ln_1p(),
        (out_usd / (in_usd + 1.0)).ln_1p(),
        syms.len() as f64,
        total_usd.ln_1p(),
    ]
}

/// The trained logistic model (weights + standardization), from `sm-model.json`.
pub struct SmModel {
    pub w: Vec<f64>,
    pub b: f64,
    pub mean: Vec<f64>,
    pub std: Vec<f64>,
}

/// Lazily load `sm-model.json` from the dataset dir. `None` if absent/malformed, if it
/// was trained on a different feature set, or if it failed the activation gate — then the
/// route returns `{available:false}`, exactly as JS does with `!model || !model.active`.
/// A model that does not beat its own permuted-label baseline must not show a number.
pub fn model(root: &Path) -> Option<&'static SmModel> {
    static M: OnceLock<Option<SmModel>> = OnceLock::new();
    M.get_or_init(|| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(root.join("sm-model.json")).ok()?).ok()?;
        if !v["active"].as_bool().unwrap_or(false) {
            return None; // gate: не б'є нульову лінію → ймовірність не показуємо
        }
        let feats: Vec<&str> = v["feats"].as_array()?.iter().filter_map(|f| f.as_str()).collect();
        if feats != FEATS {
            return None; // модель зі старим набором ознак несумісна з sm_features()
        }
        let arr = |k: &str| v[k].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect::<Vec<f64>>());
        Some(SmModel { w: arr("w")?, b: v["b"].as_f64()?, mean: arr("mean")?, std: arr("std")? })
    })
    .as_ref()
}

/// ml-smart-money.js FEATS — feature names, index-aligned with the model weights.
pub const FEATS: [&str; 8] =
    ["log_inUsd", "log_outUsd", "log_tx", "chains", "log_spanDays", "log_outInRatio", "numTokens", "log_totalUsd"];

/// ml-smart-money.js `smStatusBuild` — model metadata + the top-5 features by absolute
/// (2-dp-rounded) weight. `model` is the raw `sm-model.json` value (`Null` if
/// untrained); `samples` the training-set size.
///
/// `nullAcc`/`nullAuc` (the same CV re-run on permuted labels) ship alongside `cvAcc`/`cvAuc`
/// on purpose: a cross-validated score without its null baseline is not interpretable.
/// `active` is the JS activation gate — an unactivated model must not surface a probability.
pub fn ml_status_build(model: &Value, samples: i64) -> Value {
    if !model.is_object() {
        return serde_json::json!({
            "trained": false, "active": false, "samples": js_num(samples as f64),
            "posRate": Value::Null, "acc": Value::Null, "cvAcc": Value::Null, "cvAuc": Value::Null,
            "nullAcc": Value::Null, "nullAuc": Value::Null, "baselineAcc": Value::Null, "kind": "retrospective",
            "updatedAt": js_num(0.0), "importance": [],
        });
    }
    let empty = vec![];
    let w = model["w"].as_array().unwrap_or(&empty);
    let mut importance: Vec<Value> = FEATS
        .iter()
        .enumerate()
        .map(|(j, f)| {
            let wj = w.get(j).and_then(|v| v.as_f64()).unwrap_or(0.0);
            serde_json::json!({"f": f, "w": js_num(crate::jsmath::round_fixed(wj, 2))})
        })
        .collect();
    importance.sort_by(|a, b| {
        b["w"].as_f64().unwrap_or(0.0).abs().partial_cmp(&a["w"].as_f64().unwrap_or(0.0).abs()).unwrap_or(std::cmp::Ordering::Equal)
    });
    importance.truncate(5);
    let nullish = |k: &str| if model[k].is_null() { Value::Null } else { model[k].clone() };
    // Планка, яку accuracy мусить перебити, щоб узагалі щось означати: «завжди відповідай
    // мажоритарним класом». При posRate 12% це 88% — а модель дає 66% CV, тобто ГІРШЕ, ніж не
    // робити нічого. Досі поруч стояв лише nullAcc (та сама CV на перемішаних мітках, ~49%), і на
    // його тлі 66% виглядали перемогою. nullAcc відповідає на питання «чи є сигнал», а не «чи
    // корисна ця точність» — і сплутати їх легко саме тому, що обидва звуться baseline.
    // Число віддаємо назовні, а не ховаємо в коментар: воно потрібне тому, хто дивиться на acc.
    let baseline_acc = model["posRate"]
        .as_f64()
        .map(|p| js_num(crate::jsmath::js_round(p.max(100.0 - p))))
        .unwrap_or(Value::Null);
    serde_json::json!({
        "trained": true,
        "active": model["active"].as_bool().unwrap_or(false),
        "samples": js_num(samples as f64),
        "posRate": nullish("posRate"),
        "acc": nullish("acc"),
        "cvAcc": nullish("cvAcc"),
        "cvAuc": nullish("cvAuc"),
        "nullAcc": nullish("nullAcc"),
        "nullAuc": nullish("nullAuc"),
        "baselineAcc": baseline_acc,
        "kind": "retrospective",
        "updatedAt": if model["updatedAt"].is_null() { js_num(0.0) } else { model["updatedAt"].clone() },
        "importance": importance,
    })
}

/// Raw `sm-model.json` value (cached), for fields `ml_status_build` needs beyond
/// the standardization struct.
pub fn model_raw(root: &Path) -> Option<&'static Value> {
    static M: OnceLock<Option<Value>> = OnceLock::new();
    M.get_or_init(|| serde_json::from_str(&std::fs::read_to_string(root.join("sm-model.json")).ok()?).ok()).as_ref()
}

/// Training-set size from `sm-train.json` (cached); 0 if absent/malformed.
pub fn train_len(root: &Path) -> usize {
    static L: OnceLock<usize> = OnceLock::new();
    *L.get_or_init(|| {
        std::fs::read_to_string(root.join("sm-train.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_array().map(|a| a.len()))
            .unwrap_or(0)
    })
}

// `graph_node()` (the crawler graph degree/volume lookup) was deleted along with the
// log_gdeg/log_gvol features it fed — see `sm_features`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ml_status_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("mlstatus-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("mlstatus-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(ml_status_build(&c["model"], c["samples"].as_i64().unwrap()), golden[i], "case {i}");
        }
    }

    #[test]
    fn sm_predict_matches_node() {
        // The route emits `prob` rounded to 1 decimal, so that value is asserted
        // exactly; the raw feature vector is checked within a tight tolerance
        // because `Math.log1p` and Rust's `ln_1p` can differ by ~1 ULP (a genuine
        // libm portability limit that the 1-decimal rounding absorbs).
        let inputs: Value = serde_json::from_str(&fixture!("smpredict-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("smpredict-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let tokens = c["tokens"].as_array().unwrap();
            let x = sm_features(&c["flow"], tokens);
            assert_eq!(x.len(), FEATS.len(), "case {i}: feature vector must match FEATS");
            let gx = golden[i]["x"].as_array().unwrap();
            for (j, xj) in x.iter().enumerate() {
                assert!((xj - gx[j].as_f64().unwrap()).abs() < 1e-9, "case {i} feature {j}: {xj} vs {}", gx[j]);
            }
            let m = &c["model"];
            let f = |k: &str| m[k].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<f64>>();
            let p = sm_prob(&f("w"), m["b"].as_f64().unwrap(), &f("mean"), &f("std"), &x);
            let prob1 = crate::jsmath::round_fixed(p * 100.0, 1);
            assert_eq!(prob1, golden[i]["prob1"].as_f64().unwrap(), "case {i} prob1");
        }
    }
}
