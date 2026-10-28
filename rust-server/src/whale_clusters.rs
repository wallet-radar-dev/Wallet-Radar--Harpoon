//! Port of server/whale-clusters.js — cross-asset whale-cluster builder.
//! Pure given `data` (the network fan-out happens in forensics-worker, which
//! assembles `data`). Ordering is load-bearing throughout: Map/Set insertion
//! order, and critically `Object.values(comp)` iterating ascending integer label.

use crate::cex;
use crate::entities;
use crate::labels::Labels;
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const MIRROR_MIN_OVERLAP: usize = 6;
const MIRROR_MIN_CONTAIN: f64 = 0.5;

/// whale-clusters.js:16-19
fn is_infra(labels: &Labels, addr: &str) -> bool {
    let a = addr.to_lowercase();
    cex::is_cex(&a) || labels.is_infra_addr(&a) || entities::entity_of(&a).is_some()
}

/// JS number serialization: integral values print without a decimal point, so
/// `900000000` not `900000000.0`. serde_json's f64 always adds `.0`, which would
/// break parity — emit an integer when the value is integral.
fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

/// whale-clusters.js:24-28. `s` = the smaller set (or `b` on a size tie).
fn contain(a: &IndexSet<String>, b: &IndexSet<String>) -> (usize, f64) {
    let (s, l) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let n = s.iter().filter(|x| l.contains(*x)).count();
    let denom = a.len().min(b.len()).max(1) as f64;
    (n, n as f64 / denom)
}

// ── SIZE INVARIANT ──
// A cluster holding 339 wallets is not a finding, it is a merge failure. That is exactly what
// production produced: one cluster with 92% of every clustered address, i.e. clustering that
// separated nothing.
const MAX_CLUSTER_SHARE: f64 = 0.25; // no cluster may hold >25% of all clustered wallets
const MAX_CLUSTER_ABS: usize = 100; // …nor more than 100 addresses outright
const GAMMA_BASE: f64 = 1.5; // default Louvain resolution
const GAMMA_STEPS: [f64; 4] = [1.5, 3.0, 6.0, 12.0]; // escalation for oversized communities

type Adj = IndexMap<String, IndexMap<String, f64>>;

/// Weighted Louvain (modularity), deterministic — mirrors `louvain()` in whale-clusters.js.
///
/// It replaced label propagation, which on a dense graph inevitably collapses into a single blob:
/// it has no resolution parameter, so nothing stops the merge. Louvain maximizes modularity with a
/// resolution γ — higher γ, smaller communities. Traversal order is fixed and ties break toward the
/// lower community id: no RNG anywhere, so Node and Rust agree.
fn louvain(nodes: &[String], adj: &Adj, gamma: f64) -> Vec<usize> {
    let idx: IndexMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    let mut comm: Vec<usize> = (0..nodes.len()).collect();
    let k: Vec<f64> = nodes.iter().map(|n| adj.get(n).map_or(0.0, |m| m.values().sum())).collect();
    let m2: f64 = k.iter().sum();
    if m2 == 0.0 {
        return comm;
    }
    let mut tot = k.clone();
    for _ in 0..20 {
        let mut moved = false;
        for i in 0..nodes.len() {
            let Some(nb) = adj.get(&nodes[i]) else { continue };
            let ci = comm[i];
            tot[ci] -= k[i]; // temporarily pull the node out of its own community
            let mut w_to: IndexMap<usize, f64> = IndexMap::new();
            for (n2, w) in nb {
                let Some(&j) = idx.get(n2.as_str()) else { continue };
                *w_to.entry(comm[j]).or_insert(0.0) += w;
            }
            let mut best = ci;
            let mut best_gain = w_to.get(&ci).copied().unwrap_or(0.0) - gamma * tot[ci] * k[i] / m2;
            for (&c, &w) in &w_to {
                let gain = w - gamma * tot[c] * k[i] / m2;
                if gain > best_gain + 1e-12 || ((gain - best_gain).abs() <= 1e-12 && c < best) {
                    best_gain = gain;
                    best = c;
                }
            }
            tot[best] += k[i];
            if best != ci {
                comm[i] = best;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    comm
}

/// Group nodes into communities, ESCALATING the resolution for oversized ones. A community that
/// still refuses to split is NOT served as a whale cluster — it goes to diagnostics. "The merge
/// failed" is an honest answer; 339 unrelated addresses declared one entity is not.
fn communities(nodes: &[String], adj: &Adj) -> (Vec<Vec<String>>, Vec<Vec<String>>, usize) {
    let limit = ((nodes.len() as f64 * MAX_CLUSTER_SHARE).floor() as usize).min(MAX_CLUSTER_ABS).max(2);
    let mut resolved: Vec<Vec<String>> = Vec::new();
    let mut unresolved: Vec<Vec<String>> = Vec::new();

    fn split(subset: Vec<String>, step: usize, adj: &Adj, limit: usize, resolved: &mut Vec<Vec<String>>, unresolved: &mut Vec<Vec<String>>) {
        if subset.len() <= limit {
            if subset.len() >= 2 {
                resolved.push(subset);
            }
            return;
        }
        if step >= GAMMA_STEPS.len() {
            unresolved.push(subset); // ran out of evidence to split on
            return;
        }
        let sub: IndexSet<&str> = subset.iter().map(String::as_str).collect();
        let mut sub_adj: Adj = IndexMap::new();
        for n in &subset {
            let Some(m) = adj.get(n) else { continue };
            let mm: IndexMap<String, f64> = m.iter().filter(|(x, _)| sub.contains(x.as_str())).map(|(x, w)| (x.clone(), *w)).collect();
            sub_adj.insert(n.clone(), mm);
        }
        let comm = louvain(&subset, &sub_adj, GAMMA_STEPS[step]);
        let mut by: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (i, &c) in comm.iter().enumerate() {
            by.entry(c).or_default().push(subset[i].clone());
        }
        let parts: Vec<Vec<String>> = by.into_values().collect();
        // Won't split — don't invent a split. And if raising the resolution shattered the subset into
        // nothing but singletons, that is also "failed", not "no clusters here": otherwise those
        // addresses would simply vanish from the result (singletons are not clusters and go nowhere).
        if parts.len() <= 1 || !parts.iter().any(|p| p.len() >= 2) {
            unresolved.push(subset);
            return;
        }
        for part in parts {
            split(part, step + 1, adj, limit, resolved, unresolved);
        }
    }

    let top = louvain(nodes, adj, GAMMA_BASE);
    let mut by_top: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, &c) in top.iter().enumerate() {
        by_top.entry(c).or_default().push(nodes[i].clone());
    }
    for part in by_top.into_values() {
        split(part, 0, adj, limit, &mut resolved, &mut unresolved);
    }
    (resolved, unresolved, limit)
}

pub fn build_whale_clusters(data: &Value, labels: &Labels) -> Value {
    let empty = vec![];
    let assets = data["assets"].as_array().unwrap_or(&empty);
    let fingerprints = &data["fingerprints"];
    let funders = &data["funders"];

    // addr -> set of asset symbols. Insertion order = first-seen, drives `nodes`.
    let mut in_assets: IndexMap<String, IndexSet<String>> = IndexMap::new();
    for a in assets {
        let sym = a["symbol"].as_str().unwrap_or_default().to_string();
        for g in a["groups"].as_array().unwrap_or(&empty) {
            for m in g["members"].as_array().unwrap_or(&empty) {
                let addr = m["addr"].as_str().unwrap_or_default().to_string();
                if is_infra(labels, &addr) {
                    continue;
                }
                in_assets.entry(addr).or_default().insert(sym.clone());
            }
        }
    }
    let nodes: Vec<String> = in_assets.keys().cloned().collect();
    let in_set: IndexSet<String> = nodes.iter().cloned().collect();

    // Weighted adjacency (not union-find). adj[x][y] += w for both directions.
    let mut adj: IndexMap<String, IndexMap<String, f64>> = IndexMap::new();
    let add_edge = |adj: &mut IndexMap<String, IndexMap<String, f64>>, a: &str, b: &str, w: f64| {
        if a == b {
            return;
        }
        for (x, y) in [(a, b), (b, a)] {
            *adj.entry(x.to_string()).or_default().entry(y.to_string()).or_insert(0.0) += w;
        }
    };

    // 1) shared per-asset group membership. The weight is NORMALIZED BY GROUP SIZE: a group is a
    // hyperedge, and a pair co-occurring in a 3-member group is strong evidence while the same pair
    // in a 38-member group is nearly none. Both used to get weight 2, which turned the 23 groups of
    // 15–38 holders into full cliques (6115 edges) and glued the whole graph into one blob. Now a
    // group spreads a total weight of 2 across its pairs: 2/(s−1) each.
    for a in assets {
        for g in a["groups"].as_array().unwrap_or(&empty) {
            let ms: Vec<String> = g["members"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter_map(|m| m["addr"].as_str().map(str::to_string))
                .filter(|x| in_set.contains(x))
                .collect();
            let w = 2.0 / (ms.len().saturating_sub(1)).max(1) as f64;
            for i in 0..ms.len() {
                for j in (i + 1)..ms.len() {
                    add_edge(&mut adj, &ms[i], &ms[j], w);
                }
            }
        }
    }

    // 2) cross-asset shared funder, weight 1. Skip fan-out >40 (distributor/exchange).
    //
    // Спершу — САМ фандер. is_infra() тут не викликався взагалі: інфраструктуру відсіювали лише
    // серед членів кластера (вище), а фандера пускали будь-якого, покладаючись на фан-аут >40 як
    // на ознаку біржі. Але біржа, що нагодувала 16 наших холдерів, у ту стелю не впирається — і
    // «Binance 16», «Gate.io», «Kraken 4» ставали доказом координації. Двоє людей, що вивели
    // кошти з Binance, не знайомі між собою; на цій хибі трималося 11 зі 100 кластерів
    // ($711 млн, 31 адреса) — вони не мали ні спільних активів, ні дзеркал, лише біржу.
    // Відповідь у системи була весь час: CEX_NAMES містить і binance, і gate\.io.
    let mut funder_edges: IndexMap<String, Vec<String>> = IndexMap::new();
    if let Some(obj) = funders.as_object() {
        for (f, arr) in obj {
            if is_infra(labels, f) {
                continue;
            }
            let mut within: IndexSet<String> = IndexSet::new();
            for x in arr.as_array().unwrap_or(&empty) {
                if let Some(s) = x.as_str() {
                    if in_set.contains(s) {
                        within.insert(s.to_string());
                    }
                }
            }
            let within: Vec<String> = within.into_iter().collect();
            if within.len() >= 2 && within.len() <= 40 {
                // Same normalization: a funder backing 40 holders is not 40 people who know each other.
                let w = 1.0 / (within.len().saturating_sub(1)).max(1) as f64;
                for i in 0..within.len() {
                    for j in (i + 1)..within.len() {
                        add_edge(&mut adj, &within[i], &within[j], w);
                    }
                }
            }
            funder_edges.insert(f.clone(), within);
        }
    }

    // 3) mirror portfolios — drop non-discriminating tokens, then match baskets.
    let tokens_of = |n: &str| -> IndexSet<String> {
        fingerprints[n]["tokens"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect()
    };
    let mut df: IndexMap<String, usize> = IndexMap::new();
    for n in &nodes {
        for tok in tokens_of(n) {
            *df.entry(tok).or_insert(0) += 1;
        }
    }
    let big_n = nodes.len().max(1) as f64;
    let stop: IndexSet<String> = df
        .iter()
        .filter(|(_, c)| **c as f64 / big_n > 0.4 || **c >= 8)
        .map(|(t, _)| t.clone())
        .collect();
    let fp_set: IndexMap<String, IndexSet<String>> = nodes
        .iter()
        .map(|n| (n.clone(), tokens_of(n).into_iter().filter(|t| !stop.contains(t)).collect()))
        .collect();

    let mut mirror_pairs: Vec<(String, String, usize)> = Vec::new();
    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            let a = &fp_set[&nodes[i]];
            let b = &fp_set[&nodes[j]];
            if a.len() < MIRROR_MIN_OVERLAP || b.len() < MIRROR_MIN_OVERLAP {
                continue;
            }
            let (overlap, ratio) = contain(a, b);
            if overlap >= MIRROR_MIN_OVERLAP && ratio >= MIRROR_MIN_CONTAIN {
                add_edge(&mut adj, &nodes[i], &nodes[j], (overlap as f64 / 3.0).min(3.0));
                mirror_pairs.push((nodes[i].clone(), nodes[j].clone(), overlap));
            }
        }
    }

    // Community detection: weighted Louvain + the size invariant (see `communities`).
    let (resolved, unresolved, limit) = communities(&nodes, &adj);

    // Members keep `nodes` order inside each community, exactly as JS builds them.
    let mut comp: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, part) in resolved.iter().enumerate() {
        comp.insert(i, part.clone());
    }

    let sane = |v: &Value| -> f64 { v.as_f64().unwrap_or(0.0).min(2e9) };

    struct Cluster {
        key: String,
        members: Vec<Value>,
        assets: Vec<String>,
        asset_count: usize,
        combined_usd: f64,
        mirror_pairs: usize,
        shared_funders: Vec<Value>,
    }

    let mut clusters: Vec<Cluster> = comp
        .values()
        .filter(|c| c.len() >= 2)
        .map(|members| {
            let mut aset: IndexSet<String> = IndexSet::new();
            for m in members {
                for s in &in_assets[m] {
                    aset.insert(s.clone());
                }
            }
            let combined_usd: f64 = members.iter().map(|m| sane(&fingerprints[m]["totalUsd"])).sum();
            let shared_funders: Vec<Value> = funder_edges
                .iter()
                .filter(|(_, arr)| arr.iter().filter(|x| members.contains(x)).count() >= 2)
                .map(|(f, arr)| json!({"funder": f, "holders": arr.iter().filter(|x| members.contains(x)).count()}))
                .collect();
            let mp = mirror_pairs
                .iter()
                .filter(|(a, b, _)| members.contains(a) && members.contains(b))
                .count();

            let mut key_sorted = members.clone();
            key_sorted.sort();
            let key = key_sorted[0].clone();

            let mut mem_out: Vec<Value> = members
                .iter()
                .map(|m| {
                    json!({
                        "addr": m,
                        "totalUsd": num(sane(&fingerprints[m]["totalUsd"])),
                        "top": fingerprints[m]["top"].as_array().cloned().unwrap_or_default(),
                        "assets": in_assets[m].iter().cloned().collect::<Vec<_>>(),
                    })
                })
                .collect();
            mem_out.sort_by(|a, b| {
                let (ta, tb) = (a["totalUsd"].as_f64().unwrap_or(0.0), b["totalUsd"].as_f64().unwrap_or(0.0));
                tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
            });

            Cluster {
                key,
                members: mem_out,
                asset_count: aset.len(),
                assets: aset.into_iter().collect(),
                combined_usd,
                mirror_pairs: mp,
                shared_funders,
            }
        })
        .filter(|c| c.asset_count >= 2 || c.mirror_pairs >= 1 || !c.shared_funders.is_empty())
        .collect();

    clusters.sort_by(|a, b| b.combined_usd.partial_cmp(&a.combined_usd).unwrap_or(std::cmp::Ordering::Equal));

    let out: Vec<Value> = clusters
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let shared_funders: Vec<Value> = c.shared_funders.into_iter().take(8).collect();
            json!({
                "id": format!("WC-{}", i + 1),
                "label": format!("Whale Cluster #{}", i + 1),
                "key": c.key,
                "members": c.members,
                "assets": c.assets,
                "assetCount": c.asset_count,
                "combinedUsd": num(c.combined_usd),
                "evidence": {
                    "mirrorPairs": c.mirror_pairs,
                    "sharedFunders": shared_funders,
                    "spansAssets": c.asset_count,
                },
            })
        })
        .collect();

    // Communities that would not split even at the highest resolution are NOT served as whale
    // clusters — they go to diagnostics. Previously such a blob shipped as WC-1 and held 92% of all
    // clustered addresses.
    let largest = out.first().and_then(|c| c["members"].as_array()).map_or(0, Vec::len);
    let blobs: Vec<Value> = unresolved
        .iter()
        .map(|b| json!({"size": b.len(), "sample": b.iter().take(5).collect::<Vec<_>>()}))
        .collect();

    // `generated: Date.now()` is stamped by the caller; omitted here for determinism.
    json!({
        "count": out.len(),
        "clusters": out,
        "diagnostics": {
            "nodes": nodes.len(),
            "maxClusterSize": limit,
            "unresolvedBlobs": blobs,
            "largestCluster": largest,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn golden_fixture() {
        // Labels empty: fixture addresses are none of them infra, so the labels
        // DB is irrelevant here; an empty one keeps the test hermetic.
        let labels = Labels::load(Path::new("/nonexistent"));
        let input: Value = serde_json::from_str(&fixture!("wc-input.json")).unwrap();
        let want: Value = serde_json::from_str(&fixture!("wc-golden.json")).unwrap();
        let got = build_whale_clusters(&input, &labels);
        assert_eq!(got, want);
    }

    /// Зонд по ЖИВОМУ датасету (`cargo test live_probe -- --ignored --nocapture`): що поточний код
    /// РЕАЛЬНО зробить із тим, що зараз на диску. Писався, коли whale-clusters.json раптом став
    /// порожнім і треба було відрізнити «код зламався» від «на вхід прийшло не те» — за логами це
    /// не розрізняється, а тут відповідь за секунду.
    #[test]
    #[ignore]
    fn live_probe() {
        let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "..".into());
        let root = std::path::Path::new(&root).join("dataset");
        let af: Value = serde_json::from_slice(&std::fs::read(root.join("asset-forensics.json")).unwrap()).unwrap();
        let labels = crate::labels::Labels::load(&root);
        let assets = af["assets"].as_array().cloned().unwrap_or_default();
        let payload = json!({
            "assets": assets.clone(),
            "fingerprints": af["fingerprints"].clone(),
            "funders": af["funders"].clone(),
        });
        let n_funders = af["funders"].as_object().map_or(0, |o| o.len());
        let infra_funders = af["funders"]
            .as_object()
            .map_or(0, |o| o.keys().filter(|f| is_infra(&labels, f)).count());
        let members: Vec<String> = assets
            .iter()
            .flat_map(|a| a["groups"].as_array().cloned().unwrap_or_default())
            .flat_map(|g| g["members"].as_array().cloned().unwrap_or_default())
            .filter_map(|m| m["addr"].as_str().map(str::to_string))
            .collect();
        let infra_members = members.iter().filter(|m| is_infra(&labels, m)).count();
        let out = build_whale_clusters(&payload, &labels);
        println!("активів: {}", assets.len());
        println!("членів у групах: {}  з них infra: {infra_members}", members.len());
        println!("фандерів: {n_funders}  з них infra: {infra_funders}");
        println!("→ вузлів: {}  кластерів: {}", out["diagnostics"]["nodes"], out["count"]);
    }
}
