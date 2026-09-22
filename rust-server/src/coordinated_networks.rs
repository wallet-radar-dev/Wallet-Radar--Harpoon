//! Port of the pure core of server/coordinated-networks.js — clusters whales
//! (from whale-positions) into coordinated networks by SHARED ASSETS (who sits
//! together in the same coins) plus SHARED FUNDERS (conductors), via union-find.
//!
//! Steps 1-3 (linking + component assembly) are deterministic and ported here.
//! Step 4 — market-cap enrichment of the shared assets — is network-bound
//! (`prices.marketData`) and stays in the JS; it only *adds* `sharedAssets`/
//! `contract`/`marketCap` onto this result. `positions` is
//! `whalePositions().positions` (`{addr, assetCount, assets:[{symbol,share}]}`);
//! `conductors` is `conductors.json`'s array. Output matches Node byte-for-byte.

use crate::jsmath::{js_num, round_fixed};
use indexmap::{IndexMap, IndexSet};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Union-find `find` with path halving, exactly as the JS
/// `while (parent[x] !== x) { parent[x] = parent[parent[x]]; x = parent[x]; }`.
fn find(parent: &mut HashMap<String, String>, mut x: String) -> String {
    while parent[&x] != x {
        let gp = parent[&parent[&x]].clone();
        parent.insert(x.clone(), gp.clone());
        x = gp;
    }
    x
}

/// `parent[find(a)] = find(b)`, guarded by both keys existing (JS `!= null`).
fn union(parent: &mut HashMap<String, String>, a: &str, b: &str) {
    if parent.contains_key(a) && parent.contains_key(b) {
        let ra = find(parent, a.to_string());
        let rb = find(parent, b.to_string());
        parent.insert(ra, rb);
    }
}

pub fn coordinated_networks_core(pos: &[Value], conductors: &[Value]) -> Value {
    // posMap: addr → position; sets: addr → its asset-symbol set (insertion order);
    // addrs: addresses in position order.
    let mut pos_map: HashMap<&str, &Value> = HashMap::new();
    let mut sets: HashMap<String, IndexSet<String>> = HashMap::new();
    let mut addrs: Vec<String> = Vec::new();
    for p in pos {
        let addr = p["addr"].as_str().unwrap_or("");
        pos_map.insert(addr, p);
        let mut s: IndexSet<String> = IndexSet::new();
        if let Some(assets) = p["assets"].as_array() {
            for a in assets {
                s.insert(a["symbol"].as_str().unwrap_or("").to_string());
            }
        }
        sets.insert(addr.to_string(), s);
        addrs.push(addr.to_string());
    }

    let mut parent: HashMap<String, String> = addrs.iter().map(|a| (a.clone(), a.clone())).collect();

    // 1) link by SHARED ASSETS: ≥4 common coins, or Jaccard basket similarity ≥0.45.
    for i in 0..addrs.len() {
        for j in (i + 1)..addrs.len() {
            let a_set = &sets[&addrs[i]];
            let b_set = &sets[&addrs[j]];
            let sh = a_set.iter().filter(|x| b_set.contains(*x)).count();
            let denom = a_set.len() + b_set.len() - sh;
            let jac = sh as f64 / (if denom == 0 { 1.0 } else { denom as f64 });
            if sh >= 4 || jac >= 0.45 {
                let (u, v) = (addrs[i].clone(), addrs[j].clone());
                union(&mut parent, &u, &v);
            }
        }
    }

    // 2) link by SHARED CONDUCTOR: a funder backing these holders.
    for c in conductors {
        let empty = vec![];
        let h: Vec<String> = c["holders"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|x| x.as_str().unwrap_or("").to_lowercase())
            .filter(|x| parent.contains_key(x))
            .collect();
        for i in 1..h.len() {
            let (a, b) = (h[0].clone(), h[i].clone());
            union(&mut parent, &a, &b);
        }
    }

    // 3) assemble components (insertion order = first-seen root), keep size ≥2.
    let mut comp: IndexMap<String, Vec<String>> = IndexMap::new();
    for a in &addrs {
        let r = find(&mut parent, a.clone());
        comp.entry(r).or_default().push(a.clone());
    }

    // (sharedSyms.len, size, network Value) — carried so the final sort keys are cheap.
    let mut networks: Vec<(usize, usize, Value)> = Vec::new();
    for members in comp.values() {
        if members.len() < 2 {
            continue;
        }

        // asset counts across members (first-seen order).
        let mut cnt: IndexMap<String, u32> = IndexMap::new();
        for m in members {
            for x in &sets[m] {
                *cnt.entry(x.clone()).or_insert(0) += 1;
            }
        }
        let majority = (members.len() + 1) / 2; // Math.ceil(len/2)
        let shared_syms: Vec<String> = cnt.iter().filter(|&(_, &n)| n >= majority as u32).map(|(s, _)| s.clone()).collect();
        let asset_count = cnt.len();

        // collective: summed share + holder count per symbol.
        let mut coll_map: IndexMap<String, (f64, u32)> = IndexMap::new();
        for m in members {
            if let Some(assets) = pos_map[m.as_str()]["assets"].as_array() {
                for a in assets {
                    let sym = a["symbol"].as_str().unwrap_or("").to_string();
                    let e = coll_map.entry(sym).or_insert((0.0, 0));
                    e.0 += a["share"].as_f64().unwrap_or(0.0);
                    e.1 += 1;
                }
            }
        }
        let mut collective: Vec<(String, f64, u32)> =
            coll_map.into_iter().map(|(sym, (share, holders))| (sym, round_fixed(share, 2), holders)).collect();
        collective.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
        collective.truncate(20);
        let collective: Vec<Value> = collective
            .into_iter()
            .map(|(symbol, combined, holders)| {
                json_obj(&[
                    ("symbol", Value::String(symbol)),
                    ("combinedShare", js_num(combined)),
                    ("holders", js_num(holders as f64)),
                ])
            })
            .collect();

        // conductor: first funder whose holders intersect this network.
        let cond = conductors.iter().find(|c| {
            c["holders"].as_array().is_some_and(|hs| {
                hs.iter().any(|h| members.contains(&h.as_str().unwrap_or("").to_lowercase()))
            })
        });
        let conductor = cond.map(|c| c["funder"].clone()).unwrap_or(Value::Null);

        // members output: sorted by assetCount desc (stable).
        let mut sorted_members = members.clone();
        sorted_members.sort_by(|a, b| {
            let ca = pos_map[a.as_str()]["assetCount"].as_f64().unwrap_or(0.0);
            let cb = pos_map[b.as_str()]["assetCount"].as_f64().unwrap_or(0.0);
            cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
        });
        let members_out: Vec<Value> = sorted_members
            .iter()
            .map(|a| json_obj(&[("addr", Value::String(a.clone())), ("assetCount", pos_map[a.as_str()]["assetCount"].clone())]))
            .collect();

        let shared_syms_val: Vec<Value> = shared_syms.iter().map(|s| Value::String(s.clone())).collect();
        let net = json_obj(&[
            ("members", Value::Array(members_out)),
            ("size", js_num(members.len() as f64)),
            ("sharedSyms", Value::Array(shared_syms_val)),
            ("assetCount", js_num(asset_count as f64)),
            ("collective", Value::Array(collective)),
            ("conductor", conductor),
        ]);
        networks.push((shared_syms.len(), members.len(), net));
    }

    // sort by sharedSyms.length desc, then size desc (stable); top 30.
    networks.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let out: Vec<Value> = networks.into_iter().take(30).map(|(_, _, v)| v).collect();
    Value::Array(out)
}

fn json_obj(fields: &[(&str, Value)]) -> Value {
    let mut o = Map::new();
    for (k, v) in fields {
        o.insert((*k).to_string(), v.clone());
    }
    Value::Object(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinated_networks_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/conet-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/conet-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = coordinated_networks_core(c["pos"].as_array().unwrap(), c["conductors"].as_array().unwrap());
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
