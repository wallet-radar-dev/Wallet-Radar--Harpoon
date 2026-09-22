//! Port of the pure core of the /api/connection-path forensics in
//! server/index.js — finding a transfer path between two wallets and classifying
//! its nature (peel-chain / gas-funding / plain transfer).
//!
//! Two pure pieces live here and are golden-tested byte-for-byte against Node:
//!   * `peel_analysis` — server/index.js `peelAnalysis(edges)`: pattern verdict
//!     over a resolved edge list.
//!   * `connection_path` — server/index.js `connectionPath(from, to, maxHops)`:
//!     the bidirectional BFS, generic over a neighbour provider so the live
//!     `bs.transfersRich`-backed `neighborsOf` and a deterministic test stub
//!     share one implementation.
//!
//! The live wrappers (`neighborsOf`, `edgeDetail`) and route glue stay in main.rs.

use crate::jsmath::{js_num, round_fixed};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;

const DAY_MS: f64 = 86_400_000.0;

/// JS truthiness for a single Value (matches `if (e.native)` / `.filter(Boolean)`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// server/index.js `peelAnalysis(edges)`. Keeps every JS quirk: the
/// `typeof === "number" && > 0` amount filter, `amts[i-1] || 1` guard, the
/// `Set(assets).size === 1` sameness test (undefined assets collapse to one
/// key), `Math.abs` consecutive gaps, and `Infinity` maxGap when < 2 timestamps.
pub fn peel_analysis(edges: &[Value]) -> Value {
    // v = edges.filter(e => typeof e.amount === "number" && e.amount > 0)
    let v: Vec<&Value> = edges
        .iter()
        .filter(|e| matches!(&e["amount"], Value::Number(n) if n.as_f64().map(|f| f > 0.0).unwrap_or(false)))
        .collect();
    if v.len() < 2 {
        return json!({"type": "unknown", "note": ""});
    }

    let amts: Vec<f64> = v.iter().map(|e| e["amount"].as_f64().unwrap()).collect();

    // sameAsset = new Set(v.map(e => e.asset)).size === 1 — undefined assets
    // serialize to one shared key, exactly like JS collapses `undefined`.
    let assets: HashSet<String> = v.iter().map(|e| e["asset"].to_string()).collect();
    let same_asset = assets.len() == 1;

    // allSmall = v.every(e => e.native && e.amount < 0.05)
    let all_small = v.iter().all(|e| truthy(&e["native"]) && e["amount"].as_f64().unwrap() < 0.05);

    let mut peel = 0.0f64;
    for i in 1..amts.len() {
        let denom = if amts[i - 1] != 0.0 { amts[i - 1] } else { 1.0 };
        let r = amts[i] / denom;
        if (0.7..=1.02).contains(&r) {
            peel += 1.0;
        }
    }
    let peel_ratio = peel / (amts.len() as f64 - 1.0);

    // ts = v.map(e => e.ts).filter(Boolean)
    let ts: Vec<f64> = v
        .iter()
        .filter(|e| truthy(&e["ts"]))
        .map(|e| e["ts"].as_f64().unwrap())
        .collect();

    let chrono = ts.len() >= 2 && ts.iter().enumerate().all(|(i, &t)| i == 0 || t >= ts[i - 1] - DAY_MS);

    let max_gap = if ts.len() >= 2 {
        (1..ts.len()).map(|i| (ts[i] - ts[i - 1]).abs()).fold(f64::NEG_INFINITY, f64::max)
    } else {
        f64::INFINITY
    };
    let tight = max_gap <= 7.0 * DAY_MS;

    let ratio = js_num(round_fixed(peel_ratio, 2));
    if all_small {
        return json!({
            "type": "gas_funding", "note": "малі суми — фінансування газу",
            "peelRatio": ratio, "chrono": chrono, "sameAsset": same_asset, "tight": tight
        });
    }
    if same_asset && peel_ratio >= 0.6 && chrono && tight {
        return json!({
            "type": "peel_chain", "note": "той самий актив, схожі спадні суми, ≤7 днів",
            "peelRatio": ratio, "chrono": chrono, "sameAsset": same_asset, "tight": tight
        });
    }
    let note = if same_asset { "той самий актив, але без peel-патерну" } else { "різні активи/суми" };
    json!({
        "type": "transfer", "note": note,
        "peelRatio": ratio, "chrono": chrono, "sameAsset": same_asset, "tight": tight
    })
}

/// Reconstruct the full `from → … → to` path through meeting node `m`, following
/// the forward parents (`pf`) up to `from`, then the backward parents (`pb`) down
/// to `to`. Mirrors the JS `build(m)` closure exactly.
fn build_path(m: &str, pf: &HashMap<String, Option<String>>, pb: &HashMap<String, Option<String>>) -> Vec<String> {
    let mut f = Vec::new();
    let mut x = Some(m.to_string());
    while let Some(cur) = x {
        f.push(cur.clone());
        x = pf.get(&cur).cloned().flatten();
    }
    f.reverse();
    let mut x = pb.get(m).cloned().flatten();
    while let Some(cur) = x {
        f.push(cur.clone());
        x = pb.get(&cur).cloned().flatten();
    }
    f
}

/// server/index.js `connectionPath(from, to, maxHops)` — bidirectional BFS that
/// alternates expanding the smaller frontier, capped at 140 node expansions.
/// `neighbors` stands in for `neighborsOf`; the live caller swallows fetch errors
/// to an empty list (the JS `try/catch`), so this side never sees a failure.
pub async fn connection_path<F, Fut>(from: &str, to: &str, max_hops: usize, neighbors: F) -> Value
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Vec<String>>,
{
    let from = from.to_lowercase();
    let to = to.to_lowercase();
    if from == to {
        return json!({"found": true, "path": [from], "hops": 0});
    }

    let mut pf: HashMap<String, Option<String>> = HashMap::from([(from.clone(), None)]);
    let mut pb: HashMap<String, Option<String>> = HashMap::from([(to.clone(), None)]);
    let mut front_f = vec![from.clone()];
    let mut front_b = vec![to.clone()];
    let mut explored: i64 = 0;
    const CAP: i64 = 140;

    let mut d = 0;
    while d < max_hops && explored < CAP {
        let fwd = front_f.len() <= front_b.len();
        let front = if fwd { front_f.clone() } else { front_b.clone() };
        let mut next: Vec<String> = Vec::new();
        for node in front {
            if explored >= CAP {
                break;
            }
            explored += 1;
            let nbs = neighbors(node.clone()).await;
            for nb in nbs {
                // seen = fwd ? pf : pb; otherSeen = fwd ? pb : pf.
                let already = if fwd { pf.contains_key(&nb) } else { pb.contains_key(&nb) };
                if already {
                    continue;
                }
                if fwd {
                    pf.insert(nb.clone(), Some(node.clone()));
                } else {
                    pb.insert(nb.clone(), Some(node.clone()));
                }
                let met = if fwd { pb.contains_key(&nb) } else { pf.contains_key(&nb) };
                if met {
                    let path = build_path(&nb, &pf, &pb);
                    let hops = path.len() - 1;
                    return json!({"found": true, "path": path, "hops": hops, "explored": explored});
                }
                next.push(nb);
            }
        }
        if fwd {
            front_f = next;
        } else {
            front_b = next;
        }
        if front_f.is_empty() && front_b.is_empty() {
            break;
        }
        d += 1;
    }
    json!({"found": false, "explored": explored})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peel_analysis_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/peel-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/peel-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = peel_analysis(c["edges"].as_array().unwrap());
            assert_eq!(got, golden[i], "case {i}");
        }
    }

    #[test]
    fn connection_path_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/connpath-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/connpath-golden.json")).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            // Deterministic neighbour provider straight from the case's adjacency map.
            let adj = c["adj"].clone();
            let neighbors = |node: String| {
                let adj = adj.clone();
                async move {
                    adj[&node]
                        .as_array()
                        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default()
                }
            };
            let from = c["from"].as_str().unwrap();
            let to = c["to"].as_str().unwrap();
            let max_hops = c["maxHops"].as_u64().unwrap() as usize;
            let got = rt.block_on(connection_path(from, to, max_hops, neighbors));
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
