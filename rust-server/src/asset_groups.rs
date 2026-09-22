//! Port of the pure cores of server/wallet-asset-groups.js — for a wallet's top
//! assets, find coordinated whale groups among each token's holders, clustered by
//! a shared ETH value-funder (union-find).
//!
//! `funders_of_eth` (incoming non-exchange ETH funders of an address) and
//! `detect_groups` (the union-find clustering) are pure and golden-tested. The
//! union-find keeps JS's exact quirks: path-halving `find`, unions applied in
//! funder-insertion order, and `shared` keyed by the root **at union time** (which
//! a later union can move), so a group's `commonFunders` is whatever landed under
//! its final root. The fetch orchestration stays in main.rs.

use crate::cex::is_cex;
use crate::jsmath::{js_num, js_round, round_fixed};
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Map, Value};
use std::cmp::Ordering::Equal;
use std::collections::HashMap;

/// wallet-asset-groups.js `fundersOfEth` reduction — funder → the list of incoming
/// ETH amounts, for transfers *to* `addr` from a non-exchange, non-self sender.
/// First-seen funder order.
pub fn funders_of_eth(addr: &str, nat: &[Value]) -> Value {
    let mut m: IndexMap<String, Vec<Value>> = IndexMap::new();
    for t in nat {
        if t["to"].as_str() == Some(addr) {
            if let Some(from) = t["from"].as_str() {
                if !from.is_empty() && !is_cex(from) && from != addr {
                    m.entry(from.to_string()).or_default().push(t["amount"].clone());
                }
            }
        }
    }
    let mut o = Map::new();
    for (k, v) in m {
        o.insert(k, Value::Array(v));
    }
    Value::Object(o)
}

/// path-halving `find`, mutating `parent` exactly as the JS closure does.
fn find(parent: &mut HashMap<String, String>, mut x: String) -> String {
    while parent[&x] != x {
        let p = parent[&x].clone();
        let gp = parent[&p].clone();
        parent.insert(x.clone(), gp.clone());
        x = gp;
    }
    x
}

/// wallet-asset-groups.js `detectGroups`, minus the fetch/filter. `holders` are the
/// already-filtered top holders (`{addr, share, isContract}`); `hf` maps each
/// holder addr → its `funders_of_eth` object; `px` the ETH price. Returns the
/// coordinated groups (≥2 members sharing a ≥$50-avg value-funder).
pub fn detect_groups(holders: &[Value], hf: &Value, px: f64) -> Value {
    // fh: funder → (holder set, pooled amounts), first-seen order across holders.
    let mut fh: IndexMap<String, (IndexSet<String>, Vec<f64>)> = IndexMap::new();
    for h in holders {
        let haddr = h["addr"].as_str().unwrap_or("");
        if let Some(funders) = hf[haddr].as_object() {
            for (f, amts) in funders {
                let e = fh.entry(f.clone()).or_insert((IndexSet::new(), Vec::new()));
                e.0.insert(haddr.to_string());
                for a in amts.as_array().unwrap_or(&vec![]) {
                    e.1.push(a.as_f64().unwrap_or(0.0));
                }
            }
        }
    }

    let mut parent: HashMap<String, String> = HashMap::new();
    for h in holders {
        let a = h["addr"].as_str().unwrap_or("").to_string();
        parent.insert(a.clone(), a);
    }

    // shared: root-at-union-time → common funders.
    let mut shared: IndexMap<String, Vec<Value>> = IndexMap::new();
    for (f, (hset, amts)) in &fh {
        if hset.len() < 2 {
            continue;
        }
        let avg_usd = (amts.iter().sum::<f64>() / amts.len() as f64) * px;
        if avg_usd < 50.0 {
            continue; // value-funder only, not gas
        }
        let hs: Vec<String> = hset.iter().cloned().collect();
        for i in 1..hs.len() {
            let r0 = find(&mut parent, hs[0].clone());
            let ri = find(&mut parent, hs[i].clone());
            parent.insert(r0, ri);
        }
        let root = find(&mut parent, hs[0].clone());
        shared.entry(root).or_default().push(json!({"funder": f, "avgUsd": js_num(js_round(avg_usd)), "holders": js_num(hs.len() as f64)}));
    }

    // gmap: final root → member holders, first-seen root order.
    let mut gmap: IndexMap<String, Vec<Value>> = IndexMap::new();
    for h in holders {
        let r = find(&mut parent, h["addr"].as_str().unwrap_or("").to_string());
        gmap.entry(r).or_default().push(h.clone());
    }

    let mut groups: Vec<Value> = Vec::new();
    for (root, g) in &gmap {
        if g.len() < 2 {
            continue;
        }
        let mut gsorted = g.clone();
        gsorted.sort_by(|a, b| share_desc(a, b));
        let members: Vec<Value> = gsorted
            .iter()
            .map(|h| json!({"addr": h["addr"].clone(), "share": js_num(round_fixed(h["share"].as_f64().unwrap_or(0.0), 2))}))
            .collect();
        let combined = round_fixed(g.iter().map(|h| h["share"].as_f64().unwrap_or(0.0)).sum(), 2);
        let mut common = shared.get(root).cloned().unwrap_or_default();
        common.sort_by(|a, b| b["avgUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["avgUsd"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));
        common.truncate(3);
        groups.push(json!({"members": members, "combinedShare": js_num(combined), "commonFunders": common}));
    }
    groups.sort_by(|a, b| b["combinedShare"].as_f64().unwrap_or(0.0).partial_cmp(&a["combinedShare"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));
    Value::Array(groups)
}

fn share_desc(a: &Value, b: &Value) -> std::cmp::Ordering {
    b["share"].as_f64().unwrap_or(0.0).partial_cmp(&a["share"].as_f64().unwrap_or(0.0)).unwrap_or(Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funders_of_eth_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/funders-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/funders-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(funders_of_eth(c["addr"].as_str().unwrap(), c["nat"].as_array().unwrap()), golden[i], "case {i}");
        }
    }

    #[test]
    fn detect_groups_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/detectgroups-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/detectgroups-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(detect_groups(c["holders"].as_array().unwrap(), &c["hf"], c["px"].as_f64().unwrap()), golden[i], "case {i}");
        }
    }
}
