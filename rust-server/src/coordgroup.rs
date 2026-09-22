//! Port of the pure cores behind server index.js `/api/group-coordination` —
//! shared-funder coordination analysis over a set of member wallets.
//!
//! `funders_of` (an address's incoming non-hub native funders → per-funder txs,
//! plus the first funding) and `group_coordination` (common funders, direct
//! funding, first funders, 0-100 score) are pure and golden-tested. The native
//! fetches + price fetch stay in main.rs. `native_usd` chain valuation and the
//! `usd < 50 ? gas : value` split are kept exactly.

use crate::jsmath::{js_num, js_round};
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Map, Value};
use std::cmp::Ordering::Equal;
use std::collections::HashSet;

/// index.js `HUBS = Object.keys(KNOWN)` — the ten hard-coded exchange/burn hubs.
/// Набір адрес із бази за тегом.
///
/// Шість модулів rust-server мали власну копію набору `KNOWN` з index.js, і копії
/// РОЗІЙШЛИСЬ: спільних 9 адрес, у трьох є ще FTX, у двох — ще одна біржа,
/// forensics не має жодної з двох. Тому в кожного свій тег: звести їх до одного
/// означало б мовчки розширити три модулі.
fn tag_has(tag: &str, addr: &str) -> bool {
    wr_store::snapshot().wallet_has_tag("eth", addr, tag)
}

fn is_hub(a: &str) -> bool {
    tag_has("coordgroup:is_hub", a)
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// index.js `nativeUsd(chain, amount, px)` — value a native amount using the
/// per-chain native coin; `px.xdai || 1`, everything else `|| 0`.
fn native_usd(chain: &str, amount: f64, px: &Value) -> f64 {
    const ETH_NATIVE: [&str; 10] =
        ["eth", "base", "optimism", "arbitrum", "zksync", "scroll", "linea", "blast", "mode", "zora"];
    let g = |k: &str| px[k].as_f64().unwrap_or(0.0);
    if ETH_NATIVE.contains(&chain) {
        amount * g("eth")
    } else if chain == "polygon" {
        amount * g("matic")
    } else if chain == "gnosis" {
        amount * if g("xdai") != 0.0 { g("xdai") } else { 1.0 } // px.xdai || 1
    } else if chain == "celo" {
        amount * g("celo")
    } else {
        amount * g("eth")
    }
}

/// index.js `fundersOf(addr)` reduction. `results` is `[{c, tr:[native transfer]}]`
/// per chain. Returns `{funders: {funder → [{amount,chain}]}, first}` where
/// incoming (to==addr, non-hub, non-self sender) is sorted by ts.
pub fn funders_of(addr: &str, results: &[Value]) -> Value {
    let empty = vec![];
    let mut incoming: Vec<Value> = Vec::new();
    for r in results {
        let c = r["c"].as_str().unwrap_or("");
        for t in r["tr"].as_array().unwrap_or(&empty) {
            let from = t["from"].as_str();
            if t["to"].as_str() == Some(addr) && from.is_some_and(|f| !f.is_empty() && !is_hub(f) && f != addr) {
                incoming.push(json!({"from": from.unwrap(), "amount": t["amount"].clone(), "chain": c, "ts": t["ts"].clone()}));
            }
        }
    }
    incoming.sort_by(|a, b| a["ts"].as_f64().unwrap_or(0.0).partial_cmp(&b["ts"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));

    let mut by_f: IndexMap<String, Vec<Value>> = IndexMap::new();
    for t in &incoming {
        by_f.entry(t["from"].as_str().unwrap_or("").to_string()).or_default().push(json!({"amount": t["amount"].clone(), "chain": t["chain"].clone()}));
    }
    let mut funders = Map::new();
    for (k, v) in by_f {
        funders.insert(k, Value::Array(v));
    }
    json!({"funders": Value::Object(funders), "first": incoming.first().cloned().unwrap_or(Value::Null)})
}

/// index.js `/api/group-coordination` reduction. `info` maps addr → `funders_of`
/// result; `px` the native price map. Produces common funders (≥2 members),
/// direct member-to-member funding, first funders, and the 0-100 score.
pub fn group_coordination(addrs: &[String], info: &Value, px: &Value) -> Value {
    // fmap: funder → (member set, pooled txs), first-seen order across addrs.
    let empty_arr = vec![];
    let mut fmap: IndexMap<String, (IndexSet<String>, Vec<Value>)> = IndexMap::new();
    for a in addrs {
        if let Some(funders) = info[a]["funders"].as_object() {
            for (f, txs) in funders {
                let e = fmap.entry(f.clone()).or_insert((IndexSet::new(), Vec::new()));
                e.0.insert(a.clone());
                for tx in txs.as_array().unwrap_or(&empty_arr) {
                    e.1.push(tx.clone());
                }
            }
        }
    }

    let mut common: Vec<Value> = fmap
        .iter()
        .filter(|(_, e)| e.0.len() >= 2)
        .map(|(funder, e)| {
            let sum: f64 = e.1.iter().map(|x| native_usd(x["chain"].as_str().unwrap_or(""), x["amount"].as_f64().unwrap_or(0.0), px)).sum();
            let usd = sum / if e.1.is_empty() { 1.0 } else { e.1.len() as f64 };
            let members: Vec<Value> = e.0.iter().map(|m| Value::String(m.clone())).collect();
            json!({"funder": funder, "members": members, "count": js_num(e.0.len() as f64), "avgUsd": js_num(js_round(usd)), "type": if usd < 50.0 { "gas" } else { "value" }})
        })
        .collect();
    common.sort_by(|a, b| b["count"].as_f64().unwrap_or(0.0).partial_cmp(&a["count"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));
    common.truncate(15);

    // direct member→member funding.
    let addr_set: HashSet<&str> = addrs.iter().map(|s| s.as_str()).collect();
    let mut direct: Vec<Value> = Vec::new();
    for a in addrs {
        if let Some(funders) = info[a]["funders"].as_object() {
            for f in funders.keys() {
                if addr_set.contains(f.as_str()) {
                    direct.push(json!({"from": f, "to": a}));
                }
            }
        }
    }

    let first_funders: Vec<Value> = addrs
        .iter()
        .map(|a| {
            let first = &info[a]["first"];
            let ff = if truthy(&first["from"]) { first["from"].clone() } else { Value::Null };
            let ts = if truthy(&first["ts"]) { first["ts"].clone() } else { Value::Null };
            json!({"address": a, "firstFunder": ff, "ts": ts})
        })
        .collect();

    let value_funders = common.iter().filter(|c| c["type"] == "value").count();
    let count_sum: f64 = common.iter().map(|c| c["count"].as_f64().unwrap_or(0.0)).sum();
    let score = (if common.is_empty() { 0.0 } else { 50.0 })
        + (if value_funders > 0 { 20.0 } else { 0.0 })
        + (if direct.is_empty() { 0.0 } else { 25.0 })
        + count_sum.min(10.0);

    json!({
        "addresses": addrs,
        "commonFunders": common,
        "directFunding": direct,
        "firstFunders": first_funders,
        "score": js_num(score.min(100.0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funders_of_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/coordfunders-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/coordfunders-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(funders_of(c["addr"].as_str().unwrap(), c["results"].as_array().unwrap()), golden[i], "case {i}");
        }
    }

    #[test]
    fn group_coordination_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/groupcoord-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/groupcoord-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let addrs: Vec<String> = c["addrs"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            assert_eq!(group_coordination(&addrs, &c["info"], &c["px"]), golden[i], "case {i}");
        }
    }
}
