//! Port of the pure core of server/features-bs.js — the 14 wallet features
//! derived from a wallet's token transfers plus its native balance. These are
//! exactly the inputs the classifier (model.rs) consumes.
//!
//! The two network calls in the JS (`tokenTransfers`, `nativeBalance`) live
//! elsewhere; this takes their results — the transfer `items` and the native
//! balance — and reproduces the feature computation, including every JS
//! truthiness quirk and every `toFixed` rounding, so the output matches Node
//! byte-for-byte.

use crate::jsmath::{js_num, round_fixed};
use serde_json::{Map, Value};
use std::collections::HashSet;

const STABLE: [&str; 11] =
    ["USDT", "USDC", "DAI", "BUSD", "FDUSD", "TUSD", "USD1", "USDE", "PYUSD", "USDD", "GUSD"];

/// features-bs.js `extractFeaturesBS`, minus the two awaits. `items` are the
/// transfer objects `tokenTransfers` yields (`from`/`to`/`asset`/`ts`, all as
/// produced by blockscout parse_transfers_rich); `native_eth` is the RPC native
/// balance. `address` is compared case-sensitively against the already-lowercased
/// `from`/`to` fields, exactly as the JS lowercases once up front.
pub fn extract_features_bs(address: &str, items: &[Value], native_eth: f64) -> Value {
    let a = address.to_lowercase();
    let str_of = |it: &Value, k: &str| -> Option<String> { it[k].as_str().map(|s| s.to_string()) };

    let mut in_count = 0usize;
    let mut out_count = 0usize;
    let mut cp_counts: indexmap::IndexMap<String, u64> = indexmap::IndexMap::new();
    let mut tokens: HashSet<String> = HashSet::new();
    let mut stable = 0usize;
    let mut ts: Vec<f64> = Vec::new();

    for it in items {
        let from = str_of(it, "from");
        let to = str_of(it, "to");
        // inItems / outItems: strict `===` against the lowercased address.
        if to.as_deref() == Some(a.as_str()) {
            in_count += 1;
        }
        if from.as_deref() == Some(a.as_str()) {
            out_count += 1;
        }

        // cp = it.from === a ? it.to : it.from; counted only if truthy (a
        // present, non-empty string — undefined/"" are falsy and skipped).
        let cp = if from.as_deref() == Some(a.as_str()) { to.clone() } else { from.clone() };
        if let Some(c) = cp {
            if !c.is_empty() {
                *cp_counts.entry(c).or_insert(0) += 1;
            }
        }

        // uniqueTokens: `new Set(items.map(it => it.asset))` — undefined (missing
        // key) is its own bucket, distinct from any string, so key precisely.
        tokens.insert(asset_key(&it["asset"]));

        // stable: `(it.asset || "").toUpperCase()` — falsy asset → "" → not stable.
        let sym = it["asset"].as_str().unwrap_or("").to_uppercase();
        if STABLE.contains(&sym.as_str()) {
            stable += 1;
        }

        // ts filtered by truthiness: 0 and NaN drop out.
        if let Some(v) = it["ts"].as_f64() {
            if v != 0.0 && !v.is_nan() {
                ts.push(v);
            }
        }
    }

    let cps = cp_counts.len();
    let max_cp = cp_counts.values().copied().fold(0u64, u64::max);

    ts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let span_ms = if ts.len() >= 2 { ts[ts.len() - 1] - ts[0] } else { 0.0 };
    let timespan_days = round_fixed(span_ms / 86_400_000.0, 3);
    let avg_gap_min =
        if ts.len() >= 2 { round_fixed(span_ms / (ts.len() as f64 - 1.0) / 60_000.0, 2) } else { 0.0 };

    let tx_count = items.len();
    // JS `|| 1` guards: each divisor falls back to 1 when it is zero.
    let in_out_ratio = round_fixed(in_count as f64 / (if out_count == 0 { 1.0 } else { out_count as f64 }), 3);
    let tx_per_cp = round_fixed(tx_count as f64 / (if cps == 0 { 1.0 } else { cps as f64 }), 2);
    let denom_tx = if tx_count == 0 { 1.0 } else { tx_count as f64 };
    let stable_frac = round_fixed(stable as f64 / denom_tx, 3);
    let max_cp_share = round_fixed(max_cp as f64 / denom_tx, 3);
    let unique_token_ratio = round_fixed(tokens.len() as f64 / denom_tx, 3);
    // txPerDay divides by the *rounded* timespanDays, then Math.max(_, 0.01).
    let tx_per_day = round_fixed(tx_count as f64 / timespan_days.max(0.01), 2);

    let mut o = Map::new();
    o.insert("txCount".into(), js_num(tx_count as f64));
    o.insert("inCount".into(), js_num(in_count as f64));
    o.insert("outCount".into(), js_num(out_count as f64));
    o.insert("inOutRatio".into(), js_num(in_out_ratio));
    o.insert("uniqueCounterparties".into(), js_num(cps as f64));
    o.insert("uniqueTokens".into(), js_num(tokens.len() as f64));
    o.insert("spamRatio".into(), js_num(0.0));
    o.insert("txPerCounterparty".into(), js_num(tx_per_cp));
    o.insert("nativeEth".into(), js_num(round_fixed(native_eth, 4)));
    o.insert("timespanDays".into(), js_num(timespan_days));
    o.insert("avgGapMin".into(), js_num(avg_gap_min));
    o.insert("stableFrac".into(), js_num(stable_frac));
    o.insert("maxCpShare".into(), js_num(max_cp_share));
    o.insert("uniqueTokenRatio".into(), js_num(unique_token_ratio));
    o.insert("txPerDay".into(), js_num(tx_per_day));
    Value::Object(o)
}

/// Set-identity key for `it.asset`: a missing key (JS `undefined`) and each
/// distinct string are separate buckets; `null`/other JSON types get their own
/// keys too. Prefixes keep a string `"undefined"` from colliding with real undefined.
fn asset_key(v: &Value) -> String {
    match v {
        Value::Null => "\0undefined".to_string(), // missing key → serde Null
        Value::String(s) => format!("s\0{s}"),
        other => format!("j\0{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_bs_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("featbs-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("featbs-golden.json")).unwrap();

        for (i, case) in inputs.as_array().unwrap().iter().enumerate() {
            let address = case["address"].as_str().unwrap();
            let items = case["items"].as_array().unwrap();
            let native = case["native"].as_f64().unwrap();
            let got = extract_features_bs(address, items, native);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
