//! Port of the pure reductions behind server/group-stats.js and
//! server/group-flow.js — group-level aggregation over a set of member wallets.
//!
//! `group_stats` folds each member's `walletBalances` into group volume + shared
//! holdings. `group_flow_member` classifies one member's exchange deposits /
//! withdrawals; `group_flow_reduce` aggregates them per exchange / per member into
//! the group money-flow graph. All three are golden-tested byte-for-byte; the
//! fetch orchestration stays in main.rs.

use crate::cex::is_cex;
use crate::jsmath::{js_num, js_round};
use indexmap::IndexMap;
use serde_json::{json, Value};

/// `entityOf(addr)?.name || "Біржа"` — the exchange display name.
fn entity_name(addr: &str) -> String {
    crate::entities::entity_of(addr).map(|e| e.name.to_string()).unwrap_or_else(|| "Біржа".to_string())
}

/// group-stats.js reduction. `per` is `[{addr, b:{totalUsd, tokens}}]` (each `b` a
/// `walletBalances` result). Produces group count / volume / sorted members /
/// combined top-15 holdings.
pub fn group_stats(per: &[Value]) -> Value {
    let empty = vec![];
    let mut members: Vec<Value> = per
        .iter()
        .map(|p| {
            let b = &p["b"];
            let total = b["totalUsd"].as_f64().unwrap_or(0.0); // totalUsd || 0
            let top: Vec<Value> = b["tokens"].as_array().unwrap_or(&empty).iter().take(5).map(|tk| tk["symbol"].clone()).collect();
            json!({"addr": p["addr"].clone(), "totalUsd": js_num(total), "top": top})
        })
        .collect();
    members.sort_by(|a, b| num_desc(a, b, "totalUsd"));
    let total_usd: f64 = members.iter().map(|m| m["totalUsd"].as_f64().unwrap_or(0.0)).sum();

    // Combined holdings by symbol (first-seen order), then top-15 by value.
    let mut by_tok: IndexMap<String, f64> = IndexMap::new();
    for p in per {
        for tk in p["b"]["tokens"].as_array().unwrap_or(&empty) {
            *by_tok.entry(tk["symbol"].as_str().unwrap_or("").to_string()).or_insert(0.0) += tk["valueUsd"].as_f64().unwrap_or(0.0);
        }
    }
    let mut holdings: Vec<Value> =
        by_tok.into_iter().map(|(symbol, usd)| json!({"symbol": symbol, "usd": js_num(js_round(usd))})).collect();
    holdings.sort_by(|a, b| num_desc(a, b, "usd"));
    holdings.truncate(15);

    json!({"count": js_num(members.len() as f64), "totalUsd": js_num(total_usd), "members": members, "holdings": holdings})
}

/// group-flow.js per-member classification: one member's exchange deposits (`dep`)
/// / withdrawals (`wd`), from native (valued at `px`) and token transfers.
pub fn group_flow_member(addr: &str, nat: &[Value], tok: &[Value], px: f64) -> Vec<Value> {
    let mut flows = Vec::new();
    let some_a = Some(addr);
    for t in nat {
        let (from, to) = (t["from"].as_str(), t["to"].as_str());
        let usd = t["amount"].as_f64().unwrap_or(0.0) * px;
        if from == some_a && to.is_some_and(is_cex) {
            flows.push(json!({"ex": entity_name(to.unwrap()), "dir": "dep", "usd": js_num(usd)}));
        } else if to == some_a && from.is_some_and(is_cex) {
            flows.push(json!({"ex": entity_name(from.unwrap()), "dir": "wd", "usd": js_num(usd)}));
        }
    }
    for t in tok {
        let (from, to) = (t["from"].as_str(), t["to"].as_str());
        let usd = t["usd"].as_f64().unwrap_or(0.0);
        if from == some_a && to.is_some_and(is_cex) {
            flows.push(json!({"ex": entity_name(to.unwrap()), "dir": "dep", "usd": js_num(usd)}));
        } else if to == some_a && from.is_some_and(is_cex) {
            flows.push(json!({"ex": entity_name(from.unwrap()), "dir": "wd", "usd": js_num(usd)}));
        }
    }
    flows
}

/// group-flow.js reduction. `per` is `[{addr, flows:[{ex,dir,usd}]}]`. Aggregates
/// into per-exchange dep/wd totals, per-member totals, ≥$1000 edges, and group
/// deposit/withdrawal totals.
pub fn group_flow_reduce(per: &[Value]) -> Value {
    let empty = vec![];
    let mut ex_agg: IndexMap<String, (f64, f64)> = IndexMap::new(); // ex → (dep, wd)
    let mut member_agg: IndexMap<String, f64> = IndexMap::new();
    let mut edges: Vec<Value> = Vec::new();

    for m in per {
        let addr = m["addr"].as_str().unwrap_or("");
        let mut by_ex_dir: IndexMap<(String, String), f64> = IndexMap::new();
        for fl in m["flows"].as_array().unwrap_or(&empty) {
            let usd = fl["usd"].as_f64().unwrap_or(0.0);
            if !(usd > 0.0) {
                continue;
            }
            let ex = fl["ex"].as_str().unwrap_or("").to_string();
            let dir = fl["dir"].as_str().unwrap_or("").to_string();
            *by_ex_dir.entry((ex.clone(), dir.clone())).or_insert(0.0) += usd;
            let e = ex_agg.entry(ex).or_insert((0.0, 0.0));
            if dir == "dep" {
                e.0 += usd;
            } else {
                e.1 += usd;
            }
            *member_agg.entry(addr.to_string()).or_insert(0.0) += usd;
        }
        for ((ex, dir), usd) in &by_ex_dir {
            if *usd >= 1000.0 {
                edges.push(json!({"member": addr, "exchange": ex, "dir": dir, "usd": js_num(js_round(*usd))}));
            }
        }
    }

    let mut members: Vec<Value> =
        member_agg.into_iter().map(|(addr, usd)| json!({"addr": addr, "usd": js_num(js_round(usd))})).collect();
    members.sort_by(|a, b| num_desc(a, b, "usd"));

    let mut exchanges: Vec<Value> = ex_agg
        .iter()
        .map(|(name, (dep, wd))| json!({"name": name, "depUsd": js_num(js_round(*dep)), "wdUsd": js_num(js_round(*wd))}))
        .collect();
    exchanges.sort_by(|a, b| {
        let bt = b["depUsd"].as_f64().unwrap_or(0.0) + b["wdUsd"].as_f64().unwrap_or(0.0);
        let at = a["depUsd"].as_f64().unwrap_or(0.0) + a["wdUsd"].as_f64().unwrap_or(0.0);
        bt.partial_cmp(&at).unwrap_or(std::cmp::Ordering::Equal)
    });

    let total_dep: f64 = ex_agg.values().map(|v| v.0).sum();
    let total_wd: f64 = ex_agg.values().map(|v| v.1).sum();
    json!({"members": members, "exchanges": exchanges, "edges": edges, "totalDepUsd": js_num(js_round(total_dep)), "totalWdUsd": js_num(js_round(total_wd))})
}

/// Descending stable comparator on a numeric field (`b - a`).
fn num_desc(a: &Value, b: &Value, key: &str) -> std::cmp::Ordering {
    b[key].as_f64().unwrap_or(0.0).partial_cmp(&a[key].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_stats_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/groupstats-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/groupstats-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(group_stats(c["per"].as_array().unwrap()), golden[i], "case {i}");
        }
    }

    #[test]
    fn group_flow_member_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/groupflow-member-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/groupflow-member-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = group_flow_member(c["addr"].as_str().unwrap(), c["nat"].as_array().unwrap(), c["tok"].as_array().unwrap(), c["px"].as_f64().unwrap());
            assert_eq!(Value::Array(got), golden[i], "case {i}");
        }
    }

    #[test]
    fn group_flow_reduce_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/groupflow-reduce-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/groupflow-reduce-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(group_flow_reduce(c["per"].as_array().unwrap()), golden[i], "case {i}");
        }
    }
}
