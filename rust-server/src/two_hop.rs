//! Port of the pure cores behind server index.js `/api/wallet-2hop` — a wallet's
//! EOA neighbourhood clustered by direct links and shared external sources.
//!
//! `neighbors_2hop` (top EOA counterparties from multi-chain transfers) and
//! `clusters_2hop` (union-find over neighbours by direct / ≥2-shared-source links)
//! are pure and golden-tested. The transfer fetches stay in main.rs.

use crate::flow_features::category_of;
use crate::jsmath::js_num;
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::cmp::Ordering::Equal;
use std::collections::{HashMap, HashSet};

/// index.js wallet-2hop step 1 — the target's top-12 non-contract EOA neighbours
/// by transfer count. `trs` is the flattened multi-chain enriched transfers.
pub fn neighbors_2hop(address: &str, trs: &[Value]) -> Value {
    // cp: counterparty → (count, first-seen name, any-contract), first-seen order.
    let mut cp: IndexMap<String, (i64, String, bool)> = IndexMap::new();
    for it in trs {
        let out = it["from"].as_str() == Some(address);
        let o = if out { it["to"].as_str() } else { it["from"].as_str() };
        let o = match o {
            Some(s) if !s.is_empty() && s != address => s,
            _ => continue,
        };
        let name = if out { it["toName"].as_str() } else { it["fromName"].as_str() }.unwrap_or("");
        let ctr = if out { it["toContract"].as_bool() } else { it["fromContract"].as_bool() }.unwrap_or(false);
        let e = cp.entry(o.to_string()).or_insert((0, String::new(), false));
        e.0 += 1;
        if !name.is_empty() && e.1.is_empty() {
            e.1 = name.to_string();
        }
        e.2 = e.2 || ctr;
    }

    let mut neighbors: Vec<(String, i64, String)> = cp
        .into_iter()
        .filter(|(_, e)| !e.2 && category_of(&e.1, e.2) == 4)
        .map(|(a, e)| (a, e.0, e.1))
        .collect();
    neighbors.sort_by(|a, b| b.1.cmp(&a.1)); // count desc, stable
    neighbors.truncate(12);
    Value::Array(
        neighbors
            .into_iter()
            .map(|(a, count, name)| json!({"addr": a, "name": if name.is_empty() { Value::Null } else { Value::String(name) }, "count": js_num(count as f64)}))
            .collect(),
    )
}

fn find(parent: &mut HashMap<String, String>, mut x: String) -> String {
    while parent[&x] != x {
        let p = parent[&x].clone();
        let gp = parent[&p].clone();
        parent.insert(x.clone(), gp.clone());
        x = gp;
    }
    x
}

/// index.js wallet-2hop steps 3-4 — link neighbours (direct transfer, or ≥2 shared
/// external sources) via union-find, then the >1 clusters. `neighbors` is
/// `neighbors_2hop`'s output; `neigh_cps` maps neighbour addr → its counterparties.
pub fn clusters_2hop(address: &str, neighbors: &[Value], neigh_cps: &Value) -> Value {
    let ids: Vec<String> = neighbors.iter().map(|n| n["addr"].as_str().unwrap_or("").to_string()).collect();
    let naddrs: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    // counterparty sets per neighbour.
    let sets: HashMap<&str, HashSet<&str>> = ids
        .iter()
        .map(|id| {
            let s: HashSet<&str> = neigh_cps[id.as_str()].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            (id.as_str(), s)
        })
        .collect();

    let mut parent: HashMap<String, String> = ids.iter().map(|a| (a.clone(), a.clone())).collect();
    let mut links: Vec<Value> = Vec::new();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            let (a, b) = (&ids[i], &ids[j]);
            let sa = sets.get(a.as_str());
            let sb = sets.get(b.as_str());
            let direct = sa.is_some_and(|s| s.contains(b.as_str())) || sb.is_some_and(|s| s.contains(a.as_str()));
            let shared = match (sa, sb) {
                (Some(sa), Some(sb)) => sa.iter().filter(|&&x| x != address && !naddrs.contains(x) && sb.contains(x)).count(),
                _ => 0,
            };
            if direct {
                links.push(json!({"a": a, "b": b, "type": "direct"}));
                let (ra, rb) = (find(&mut parent, a.clone()), find(&mut parent, b.clone()));
                parent.insert(ra, rb);
            } else if shared >= 2 {
                links.push(json!({"a": a, "b": b, "type": "shared", "shared": js_num(shared as f64)}));
                let (ra, rb) = (find(&mut parent, a.clone()), find(&mut parent, b.clone()));
                parent.insert(ra, rb);
            }
        }
    }

    let mut groups: IndexMap<String, Vec<String>> = IndexMap::new();
    for a in &ids {
        let r = find(&mut parent, a.clone());
        groups.entry(r).or_default().push(a.clone());
    }
    let mut clusters: Vec<Value> = groups
        .values()
        .filter(|g| g.len() > 1)
        .map(|g| {
            let members: Vec<Value> = g
                .iter()
                .map(|a| {
                    let n = neighbors.iter().find(|x| x["addr"].as_str() == Some(a.as_str()));
                    let name = n.map(|x| x["name"].clone()).unwrap_or(Value::Null);
                    let count = n.map(|x| x["count"].clone()).unwrap_or_else(|| js_num(0.0));
                    json!({"addr": a, "name": name, "count": count})
                })
                .collect();
            json!({"size": js_num(g.len() as f64), "members": members})
        })
        .collect();
    clusters.sort_by(|a, b| b["size"].as_f64().unwrap_or(0.0).partial_cmp(&a["size"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));

    json!({"address": address, "neighbors": js_num(neighbors.len() as f64), "clusters": clusters, "links": links})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbors_2hop_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("twohop-neighbors-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("twohop-neighbors-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(neighbors_2hop(c["address"].as_str().unwrap(), c["trs"].as_array().unwrap()), golden[i], "case {i}");
        }
    }

    #[test]
    fn clusters_2hop_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("twohop-clusters-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("twohop-clusters-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(clusters_2hop(c["address"].as_str().unwrap(), c["neighbors"].as_array().unwrap(), &c["neighCps"]), golden[i], "case {i}");
        }
    }
}
