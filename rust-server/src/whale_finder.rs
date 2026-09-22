// Pure reduction core of server/whale-finder.js `clusterWhales`: union the found
// $50M+ whales into coordinated groups by shared ETH value-funders (sum → $1B+
// groups). The seed gathering, per-candidate walletBalances fetch, fundersOfEth
// fetch, alerts, and Date.now() bookkeeping are the worker's job in main.rs. This
// takes the whale list + each whale's funders (Map(funder→[amounts])) + ETH price
// and reproduces the union-find grouping exactly. Golden-tested byte-for-byte.
//
// KEEPS the JS quirk (as in asset_groups): `shared` is keyed by the root AT UNION
// TIME, so a group's commonFunders is only what landed under its final root.

use crate::jsmath::{js_num, js_round};
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value};
use std::collections::HashMap;

fn find(parent: &mut HashMap<String, String>, mut x: String) -> String {
    while parent[&x] != x {
        let gp = parent[&parent[&x]].clone();
        parent.insert(x.clone(), gp.clone());
        x = gp;
    }
    x
}

/// clusterWhales(whales, hf, px). `whales`: [{addr,totalUsd,ethShare}]; `hf`: object
/// addr → {funder: [amounts]}; `px`: ETH price.
pub fn cluster_whales(whales_all: &[Value], hf: &Value, px: f64) -> Value {
    // whales = filter totalUsd >= 50M, take 60 (input order)
    let whales: Vec<&Value> = whales_all.iter()
        .filter(|w| w["totalUsd"].as_f64().unwrap_or(0.0) >= 50_000_000.0)
        .take(60)
        .collect();
    if whales.len() < 2 {
        return json!([]);
    }
    let addr_of = |w: &Value| w["addr"].as_str().unwrap_or("").to_string();

    // fh: funder → {who (insertion-ordered set of whale addrs), amts}
    let mut fh: IndexMap<String, (IndexSet<String>, Vec<f64>)> = IndexMap::new();
    for w in &whales {
        let addr = addr_of(w);
        if let Some(funders) = hf.get(&addr).and_then(|v| v.as_object()) {
            for (f, amts) in funders {
                let e = fh.entry(f.clone()).or_insert_with(|| (IndexSet::new(), Vec::new()));
                e.0.insert(addr.clone());
                if let Some(arr) = amts.as_array() {
                    for a in arr {
                        e.1.push(a.as_f64().unwrap_or(0.0));
                    }
                }
            }
        }
    }

    let mut parent: HashMap<String, String> = HashMap::new();
    for w in &whales {
        let a = addr_of(w);
        parent.insert(a.clone(), a);
    }

    let mut shared: IndexMap<String, Vec<Value>> = IndexMap::new();
    for (f, (who, amts)) in &fh {
        if who.len() < 2 {
            continue;
        }
        let avg_usd = amts.iter().sum::<f64>() / amts.len() as f64 * px;
        if avg_usd < 50.0 {
            continue;
        }
        let ws: Vec<String> = who.iter().cloned().collect();
        for i in 1..ws.len() {
            let r0 = find(&mut parent, ws[0].clone());
            let ri = find(&mut parent, ws[i].clone());
            parent.insert(r0, ri);
        }
        let root = find(&mut parent, ws[0].clone());
        shared.entry(root).or_default().push(json!({"funder": f, "avgUsd": js_num(js_round(avg_usd))}));
    }

    // balOf
    let mut bal_of: HashMap<String, &Value> = HashMap::new();
    for w in &whales {
        bal_of.insert(addr_of(w), w);
    }

    // gmap: root → member addrs (whale order)
    let mut gmap: IndexMap<String, Vec<String>> = IndexMap::new();
    for w in &whales {
        let a = addr_of(w);
        let r = find(&mut parent, a.clone());
        gmap.entry(r).or_default().push(a);
    }

    let mut groups: Vec<Value> = Vec::new();
    for (root, m) in &gmap {
        if m.len() < 2 {
            continue;
        }
        let mut members: Vec<&Value> = m.iter().map(|a| bal_of[a]).collect();
        members.sort_by(|x, y| y["totalUsd"].as_f64().unwrap_or(0.0).partial_cmp(&x["totalUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        let members_out: Vec<Value> = members.iter().map(|w| json!({"addr": w["addr"], "totalUsd": w["totalUsd"], "ethShare": w["ethShare"]})).collect();
        let combined: f64 = m.iter().map(|a| bal_of[a]["totalUsd"].as_f64().unwrap_or(0.0)).sum();
        let eth_accumulator = m.iter().all(|a| bal_of[a]["ethShare"].as_f64().unwrap_or(0.0) >= 40.0);
        let mut cf: Vec<Value> = shared.get(root).cloned().unwrap_or_default();
        cf.sort_by(|a, b| b["avgUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["avgUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        cf.truncate(3);
        groups.push(json!({
            "members": members_out,
            "combinedUsd": js_num(combined),
            "ethAccumulator": eth_accumulator,
            "commonFunders": cf,
        }));
    }
    groups.sort_by(|a, b| b["combinedUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["combinedUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    Value::Array(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_whales_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/clusterwhales-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/clusterwhales-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let empty = vec![];
            let whales = c["whales"].as_array().unwrap_or(&empty);
            let got = cluster_whales(whales, &c["hf"], c["px"].as_f64().unwrap());
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
