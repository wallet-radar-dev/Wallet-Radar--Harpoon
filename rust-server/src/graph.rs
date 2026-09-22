//! Port of clusterGraph from server/expand-worker.js:84-104. Pure over the
//! graph-db.json state (nodes + edges); the crawl that grows that state is
//! network-bound and lives in the worker. Union-find with path halving over
//! "significant" wallets, linking pairs that share >=3 common counterparties.

use crate::jsmath::js_round;
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value};

const VOL_MIN: f64 = 5000.0;

pub fn cluster_graph(graph: &Value) -> Value {
    // Node lookup: addr -> (degree, volumeUsd).
    let mut node: IndexMap<String, (f64, f64)> = IndexMap::new();
    if let Some(obj) = graph["nodes"].as_object() {
        for (a, n) in obj {
            node.insert(a.clone(), (n["d"].as_f64().unwrap_or(0.0), n["v"].as_f64().unwrap_or(0.0)));
        }
    }
    let sig = |a: &str| -> bool {
        match node.get(a) {
            Some(&(d, v)) => v >= VOL_MIN || d >= 6.0,
            None => false,
        }
    };
    let vol = |a: &str| -> f64 { node.get(a).map(|&(_, v)| v).unwrap_or(0.0) };

    // Adjacency over significant-on-both-ends edges. Insertion order preserved.
    let mut adj: IndexMap<String, IndexSet<String>> = IndexMap::new();
    let empty = vec![];
    for e in graph["edges"].as_array().unwrap_or(&empty) {
        let k = e["k"].as_str().unwrap_or_default();
        let Some((a, b)) = k.split_once('|') else { continue };
        if sig(a) && sig(b) {
            adj.entry(a.to_string()).or_default().insert(b.to_string());
            adj.entry(b.to_string()).or_default().insert(a.to_string());
        }
    }

    let hubs: Vec<String> = adj.keys().cloned().collect();
    let mut parent: IndexMap<String, String> = hubs.iter().map(|h| (h.clone(), h.clone())).collect();

    // Path halving, matching `parent[x] = parent[parent[x]]`.
    fn find(parent: &mut IndexMap<String, String>, x: &str) -> String {
        let mut x = x.to_string();
        while parent[&x] != x {
            let gp = parent[&parent[&x]].clone();
            *parent.get_mut(&x).unwrap() = gp.clone();
            x = gp;
        }
        x
    }

    // Link pairs sharing >=3 common counterparties. `b <= hubs[i]` is a string
    // comparison, matching JS on ASCII hex addresses.
    for i in 0..hubs.len() {
        let a_set = adj[&hubs[i]].clone();
        for b in &a_set {
            if b.as_str() <= hubs[i].as_str() {
                continue;
            }
            let Some(b_set) = adj.get(b) else { continue };
            let mut shared = 0;
            for x in &a_set {
                if b_set.contains(x) {
                    shared += 1;
                    if shared >= 3 {
                        break;
                    }
                }
            }
            if shared >= 3 {
                let ra = find(&mut parent, &hubs[i]);
                let rb = find(&mut parent, b);
                *parent.get_mut(&ra).unwrap() = rb;
            }
        }
    }

    // Components keyed by root; JS `Object.values` keeps insertion order for
    // these non-integer (address) keys → IndexMap.
    let mut comp: IndexMap<String, Vec<String>> = IndexMap::new();
    for h in &hubs {
        let r = find(&mut parent, h);
        comp.entry(r).or_default().push(h.clone());
    }

    struct Group {
        size: usize,
        volume_usd: f64,
        members: Vec<String>,
    }
    let mut groups: Vec<Group> = comp
        .values()
        .filter(|g| g.len() >= 3)
        .map(|g| {
            let mut members = g.clone();
            members.sort_by(|a, b| vol(b).partial_cmp(&vol(a)).unwrap_or(std::cmp::Ordering::Equal));
            members.truncate(12);
            Group {
                size: g.len(),
                volume_usd: js_round(g.iter().map(|a| vol(a)).sum()),
                members,
            }
        })
        .collect();
    groups.sort_by(|a, b| b.volume_usd.partial_cmp(&a.volume_usd).unwrap_or(std::cmp::Ordering::Equal));
    groups.truncate(40);

    let out: Vec<Value> = groups
        .into_iter()
        .map(|g| json!({"size": g.size, "volumeUsd": g.volume_usd as i64, "members": g.members}))
        .collect();

    json!({ "count": out.len(), "groups": out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_fixture() {
        let input: Value = serde_json::from_str(include_str!("../tests/fixtures/graph-input.json")).unwrap();
        let want: Value = serde_json::from_str(include_str!("../tests/fixtures/graph-golden.json")).unwrap();
        assert_eq!(cluster_graph(&input), want);
    }
}
