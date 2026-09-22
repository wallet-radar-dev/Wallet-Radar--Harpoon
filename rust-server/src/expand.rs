//! Port of server/expand-worker.js — the frontier crawler. Starting from our anchors
//! (whale clusters, forensics fingerprints) it walks outward along counterparties and
//! builds a PERSISTENT local graph that grows over time → dataset/graph-db.json.
//!
//! Deliberately gentle: one node per ~6s. The free explorer limits are a shared budget
//! with every other worker, and a crawler that burns them takes the whole product down
//! with it.
//!
//! Periodically it clusters what it has: groups of wallets that share ≥3 counterparties,
//! among the ones that are SIGNIFICANT by USD volume (or heavily connected). Weighting by
//! money rather than by transaction count is what keeps the result about capital instead
//! of about bots.

use crate::jsmath::{js_num, js_round};
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value};
use std::path::Path;

/// expand-worker.js:14-17
pub const CHAINS: [&str; 3] = ["eth", "base", "arbitrum"];
pub const TICK_MS: u64 = 6000;
const MAX_NODES: usize = 60_000;
const MAX_EDGES: usize = 200_000;
const MAX_FRONTIER: usize = 40_000;
pub const PERSIST_EVERY: u32 = 25;
pub const CLUSTER_EVERY: u32 = 50;
/// $5k of flow — the bar for a "significant" player.
const VOL_MIN: f64 = 5000.0;

/// Набір адрес із бази за тегом.
///
/// Шість модулів rust-server мали власну копію набору `KNOWN` з index.js, і копії
/// РОЗІЙШЛИСЬ: спільних 9 адрес, у трьох є ще FTX, у двох — ще одна біржа,
/// forensics не має жодної з двох. Тому в кожного свій тег: звести їх до одного
/// означало б мовчки розширити три модулі.
fn tag_has(tag: &str, addr: &str) -> bool {
    wr_store::snapshot().wallet_has_tag("eth", addr, tag)
}

pub fn is_cex(addr: &str) -> bool {
    tag_has("expand:is_cex", addr)
}

fn is_addr(a: &str) -> bool {
    a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A node: degree, first-seen, expanded flag, USD volume through it.
#[derive(Clone, Default)]
pub struct Node {
    pub d: u64,
    pub t: i64,
    pub x: u8,
    pub v: f64,
}

/// An edge: transaction count and USD volume.
#[derive(Clone, Default)]
struct Edge {
    n: u64,
    u: f64,
}

#[derive(Default)]
pub struct Graph {
    pub nodes: IndexMap<String, Node>,
    edges: IndexMap<String, Edge>,
    pub frontier: Vec<String>,
    pub visited: IndexSet<String>,
    pub total_vol: f64,
    pub expanded: u64,
    pub groups: usize,
}

impl Graph {
    pub fn load(file: &Path) -> Self {
        let Some(j) = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            return Self::default();
        };
        let mut g = Self::default();
        if let Some(nodes) = j["nodes"].as_object() {
            for (a, n) in nodes {
                g.nodes.insert(
                    a.clone(),
                    Node {
                        d: n["d"].as_u64().unwrap_or(0),
                        t: n["t"].as_i64().unwrap_or(0),
                        x: n["x"].as_u64().unwrap_or(0) as u8,
                        v: n["v"].as_f64().unwrap_or(0.0),
                    },
                );
            }
        }
        for e in j["edges"].as_array().unwrap_or(&vec![]) {
            let Some(k) = e["k"].as_str() else { continue };
            // Legacy rows stored `n` as the whole object; new ones as a count.
            let (n, u) = if e["n"].is_object() {
                (e["n"]["n"].as_u64().unwrap_or(0), e["n"]["u"].as_f64().unwrap_or(0.0))
            } else {
                (e["n"].as_u64().unwrap_or(0), e["u"].as_f64().unwrap_or(0.0))
            };
            g.total_vol += u;
            g.edges.insert(k.to_string(), Edge { n, u });
        }
        g.frontier = j["frontier"].as_array().map_or(vec![], |a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
        if let Some(v) = j["visited"].as_array() {
            for a in v.iter().filter_map(|x| x.as_str()) {
                g.visited.insert(a.to_string());
            }
        }
        g
    }

    pub fn persist(&self, file: &Path, now: i64) {
        let nodes: serde_json::Map<String, Value> = self
            .nodes
            .iter()
            .map(|(a, n)| (a.clone(), json!({"d": js_num(n.d as f64), "t": js_num(n.t as f64), "x": js_num(n.x as f64), "v": js_num(n.v)})))
            .collect();
        let edges: Vec<Value> =
            self.edges.iter().map(|(k, e)| json!({"k": k, "n": js_num(e.n as f64), "u": js_num(js_round(e.u))})).collect();
        let visited: Vec<&String> = self.visited.iter().rev().take(50_000).collect::<Vec<_>>().into_iter().rev().collect();

        let out = json!({
            "generated": js_num(now as f64),
            "counts": {"nodes": js_num(self.nodes.len() as f64), "edges": js_num(self.edges.len() as f64), "volumeUsd": js_num(js_round(self.total_vol))},
            "nodes": nodes,
            "edges": edges,
            "frontier": self.frontier.iter().take(MAX_FRONTIER).collect::<Vec<_>>(),
            "visited": visited,
        });
        if let Ok(text) = serde_json::to_string(&out) {
            let _ = crate::atomicw::write(file, text);
        }
    }

    fn add_node(&mut self, addr: &str, now: i64) -> bool {
        if !self.nodes.contains_key(addr) {
            if self.nodes.len() >= MAX_NODES {
                return false;
            }
            self.nodes.insert(addr.to_string(), Node { d: 0, t: now, x: 0, v: 0.0 });
        }
        true
    }

    fn add_edge(&mut self, a: &str, b: &str, usd: f64) {
        if a == b {
            return;
        }
        let k = if a < b { format!("{a}|{b}") } else { format!("{b}|{a}") };
        if !self.edges.contains_key(&k) {
            if self.edges.len() >= MAX_EDGES {
                return;
            }
            self.edges.insert(k.clone(), Edge::default());
        }
        let e = self.edges.get_mut(&k).expect("just inserted");
        e.n += 1;
        e.u += usd;
        self.total_vol += usd;
    }

    /// expand-worker.js `expandNode`, minus the fetch. `per_chain` is one transfer list
    /// per chain. Contracts and exchanges are never crawled — they are hubs everything
    /// touches, and following them would drown the graph in noise.
    pub fn expand_from(&mut self, addr: &str, per_chain: &[Value], now: i64) -> usize {
        self.visited.insert(addr.to_string());
        if let Some(s) = self.nodes.get_mut(addr) {
            s.x = 1;
        }
        let mut added = 0;
        for trs in per_chain {
            let Some(arr) = trs.as_array() else { continue };
            for t in arr {
                let (from, to) = (t["from"].as_str().unwrap_or(""), t["to"].as_str().unwrap_or(""));
                let outgoing = from == addr;
                let other = if outgoing { to } else { from };
                let other_is_contract =
                    if outgoing { t["toContract"].as_bool().unwrap_or(false) } else { t["fromContract"].as_bool().unwrap_or(false) };
                if other.is_empty() || other == addr || other_is_contract || is_cex(other) || !is_addr(other) {
                    continue;
                }
                let usd = t["usd"].as_f64().unwrap_or(0.0);
                if !self.add_node(other, now) {
                    continue;
                }
                let d = {
                    let n = self.nodes.get_mut(other).expect("just added");
                    n.d += 1;
                    n.v += usd;
                    n.d
                };
                if let Some(s) = self.nodes.get_mut(addr) {
                    s.d += 1;
                    s.v += usd;
                }
                self.add_edge(addr, other, usd);
                // Only reasonably active new nodes enter the frontier — otherwise the
                // crawler drowns in one-transaction dust.
                if !self.visited.contains(other) && d >= 2 && self.frontier.len() < MAX_FRONTIER {
                    self.frontier.push(other.to_string());
                    added += 1;
                }
            }
        }
        added
    }

    /// expand-worker.js `clusterGraph` — union-find over SIGNIFICANT wallets that share
    /// ≥3 counterparties. Pure: no API calls, so it is cheap to re-run often.
    pub fn cluster(&mut self, now: i64) -> Value {
        let sig = |a: &str| self.nodes.get(a).map(|n| n.v >= VOL_MIN || n.d >= 6).unwrap_or(false);

        let mut adj: IndexMap<String, IndexSet<String>> = IndexMap::new();
        for k in self.edges.keys() {
            let Some((a, b)) = k.split_once('|') else { continue };
            if sig(a) && sig(b) {
                adj.entry(a.to_string()).or_default().insert(b.to_string());
                adj.entry(b.to_string()).or_default().insert(a.to_string());
            }
        }
        let hubs: Vec<String> = adj.keys().cloned().collect();
        let mut parent: IndexMap<String, String> = hubs.iter().map(|h| (h.clone(), h.clone())).collect();
        fn find(parent: &mut IndexMap<String, String>, x: &str) -> String {
            let mut x = x.to_string();
            while parent.get(&x).map(String::as_str) != Some(x.as_str()) {
                let p = parent[&x].clone();
                let gp = parent[&p].clone();
                parent.insert(x.clone(), gp);
                x = p;
            }
            x
        }

        for h in &hubs {
            let a_set = adj[h].clone();
            for b in &a_set {
                if b <= h {
                    continue;
                }
                let Some(b_set) = adj.get(b) else { continue };
                let shared = a_set.iter().filter(|x| b_set.contains(*x)).take(3).count();
                if shared >= 3 {
                    let (ra, rb) = (find(&mut parent, h), find(&mut parent, b));
                    parent.insert(ra, rb);
                }
            }
        }

        let mut comp: IndexMap<String, Vec<String>> = IndexMap::new();
        for h in &hubs {
            let r = find(&mut parent, h);
            comp.entry(r).or_default().push(h.clone());
        }

        let vol = |a: &String| self.nodes.get(a).map(|n| n.v).unwrap_or(0.0);
        let mut groups: Vec<Value> = comp
            .into_values()
            .filter(|g| g.len() >= 3)
            .map(|mut g| {
                let volume: f64 = g.iter().map(vol).sum();
                g.sort_by(|a, b| vol(b).partial_cmp(&vol(a)).unwrap_or(std::cmp::Ordering::Equal));
                json!({
                    "size": js_num(g.len() as f64),
                    "volumeUsd": js_num(js_round(volume)),
                    "members": g.into_iter().take(12).collect::<Vec<String>>(),
                })
            })
            .collect();
        groups.sort_by(|a, b| {
            let f = |v: &Value| v["volumeUsd"].as_f64().unwrap_or(0.0);
            f(b).partial_cmp(&f(a)).unwrap_or(std::cmp::Ordering::Equal)
        });
        groups.truncate(40);

        self.groups = groups.len();
        json!({"generated": js_num(now as f64), "count": js_num(groups.len() as f64), "groups": groups})
    }

    /// expand-worker.js `seed` — anchors: whale-cluster members, then every forensics
    /// fingerprint. Falls back to two known whales when we have nothing at all.
    pub fn seed(&mut self, clusters: &Value, forensics: &Value, now: i64) {
        let add = |g: &mut Self, a: &str| {
            let a = a.to_lowercase();
            if is_addr(&a) && !g.visited.contains(&a) && !is_cex(&a) {
                g.add_node(&a, now);
                if !g.frontier.contains(&a) {
                    g.frontier.push(a);
                }
            }
        };
        for c in clusters["clusters"].as_array().unwrap_or(&vec![]) {
            for m in c["members"].as_array().unwrap_or(&vec![]) {
                if let Some(a) = m["addr"].as_str() {
                    add(self, a);
                }
            }
        }
        if let Some(fp) = forensics["fingerprints"].as_object() {
            for a in fp.keys() {
                add(self, a);
            }
        }
        if self.frontier.is_empty() {
            // Стартові точки розгортання — тег `expand_bootstrap` у базі.
            let boot: Vec<String> = wr_store::open()
                .ok()
                .and_then(|c| {
                    let mut q = c
                        .prepare("SELECT address FROM wallet_tags WHERE tag='expand_bootstrap'")
                        .ok()?;
                    let v: Vec<String> =
                        q.query_map([], |r| r.get(0)).ok()?.filter_map(|r| r.ok()).collect();
                    Some(v)
                })
                .unwrap_or_default();
            if boot.is_empty() {
                tracing::error!("[expand] немає тегу expand_bootstrap — граф не з чого розгортати");
            }
            for a in &boot {
                add(self, a);
            }
        }
    }

    /// The next node to expand: the most connected one in the frontier (hubs first).
    pub fn next_node(&mut self) -> Option<String> {
        if self.frontier.is_empty() {
            return None;
        }
        let deg = |a: &String| self.nodes.get(a).map(|n| n.d).unwrap_or(0);
        self.frontier.sort_by_key(deg);
        let addr = self.frontier.pop()?;
        // Already expanded → skip this tick, exactly as the JS does.
        (self.nodes.get(&addr).map(|n| n.x) != Some(1)).then_some(addr)
    }

    pub fn status(&self) -> Value {
        json!({
            "running": true,
            "nodes": js_num(self.nodes.len() as f64),
            "edges": js_num(self.edges.len() as f64),
            "frontier": js_num(self.frontier.len() as f64),
            "expanded": js_num(self.expanded as f64),
            "groups": js_num(self.groups as f64),
            "volumeUsd": js_num(js_round(self.total_vol)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0xaaaa000000000000000000000000000000000001";
    const B: &str = "0xbbbb000000000000000000000000000000000002";
    const C: &str = "0xcccc000000000000000000000000000000000003";
    const BINANCE: &str = "0x28c6c06298d514db089934071355e5743bf21d60";

    fn tr(from: &str, to: &str, usd: f64, to_contract: bool) -> Value {
        json!({"from": from, "to": to, "usd": usd, "fromContract": false, "toContract": to_contract})
    }

    #[test]
    fn the_crawler_never_follows_contracts_or_exchanges() {
        let mut g = Graph::default();
        g.add_node(A, 1);
        let transfers = json!([[
            tr(A, B, 100.0, false),          // a real counterparty
            tr(A, BINANCE, 9999.0, false),   // an exchange — a hub everything touches
            tr(A, C, 500.0, true),           // a contract
            tr(A, A, 1.0, false),            // self
        ]]);
        g.expand_from(A, transfers.as_array().unwrap(), 1);

        assert!(g.nodes.contains_key(B));
        assert!(!g.nodes.contains_key(BINANCE), "following exchanges would drown the graph");
        assert!(!g.nodes.contains_key(C), "contracts are not wallets");
        assert_eq!(g.nodes[A].x, 1, "the source is marked expanded");
    }

    /// A node enters the frontier only on its SECOND sighting — one transaction is dust.
    #[test]
    fn a_node_needs_two_edges_before_it_is_worth_crawling() {
        let mut g = Graph::default();
        g.add_node(A, 1);
        g.expand_from(A, json!([[tr(A, B, 100.0, false)]]).as_array().unwrap(), 1);
        assert!(g.frontier.is_empty(), "degree 1 → not yet");

        g.expand_from(A, json!([[tr(B, A, 100.0, false)]]).as_array().unwrap(), 1);
        assert_eq!(g.frontier, vec![B.to_string()], "degree 2 → crawl it");
    }

    #[test]
    fn edges_are_undirected_and_accumulate_volume() {
        let mut g = Graph::default();
        g.add_node(A, 1);
        g.expand_from(A, json!([[tr(A, B, 100.0, false), tr(B, A, 50.0, false)]]).as_array().unwrap(), 1);

        assert_eq!(g.edges.len(), 1, "a|b and b|a are the same edge");
        assert_eq!(g.total_vol, 150.0);
        assert_eq!(g.nodes[B].v, 150.0);
    }

    /// Clustering is USD-weighted: wallets below $5k (and with few edges) are not
    /// significant, so a crowd of dust addresses can never form a "cartel".
    #[test]
    fn only_significant_wallets_cluster() {
        let mut g = Graph::default();
        // Four hubs, each sharing 3+ counterparties, but all of them dust.
        for a in [A, B, C] {
            g.nodes.insert(a.to_string(), Node { d: 1, t: 0, x: 0, v: 10.0 });
        }
        for (i, k) in [format!("{A}|{B}"), format!("{A}|{C}"), format!("{B}|{C}")].iter().enumerate() {
            g.edges.insert(k.clone(), Edge { n: 1, u: 10.0 * (i + 1) as f64 });
        }
        let out = g.cluster(0);
        assert_eq!(out["count"], json!(0), "dust wallets are not a cluster");
    }

    /// Clustering links a PAIR only when the two are counterparties of each other AND
    /// share ≥3 further counterparties. A mere star (many wallets funded by one hub)
    /// is not a cartel — which is exactly the distinction the product cares about.
    #[test]
    fn mutually_connected_wallets_sharing_three_counterparties_form_a_cluster() {
        let mut g = Graph::default();
        let big = |v: f64| Node { d: 9, t: 0, x: 0, v };
        let core = ["0x1111111111111111111111111111111111111111", "0x2222222222222222222222222222222222222222", "0x3333333333333333333333333333333333333333"];
        let shared = ["0x4444444444444444444444444444444444444444", "0x5555555555555555555555555555555555555555", "0x6666666666666666666666666666666666666666"];
        let edge = |g: &mut Graph, a: &str, b: &str, u: f64| {
            let k = if a < b { format!("{a}|{b}") } else { format!("{b}|{a}") };
            g.edges.insert(k, Edge { n: 1, u });
        };
        for (i, a) in core.iter().enumerate() {
            g.nodes.insert(a.to_string(), big(50_000.0 - i as f64 * 1000.0));
        }
        for s in shared {
            g.nodes.insert(s.to_string(), big(10_000.0));
        }
        // The core wallets all deal with each other…
        for i in 0..core.len() {
            for j in (i + 1)..core.len() {
                edge(&mut g, core[i], core[j], 1000.0);
            }
        }
        // …and each of them with the same three counterparties.
        for a in core {
            for s in shared {
                edge(&mut g, a, s, 1000.0);
            }
        }
        let out = g.cluster(0);

        assert_eq!(out["count"], json!(1));
        let grp = &out["groups"][0];
        assert_eq!(grp["size"], json!(3), "the three mutually-connected core wallets");
        assert_eq!(grp["members"][0], json!(core[0]), "richest first");
    }

    /// The star that must NOT cluster: three wallets share counterparties but never
    /// transact with one another.
    #[test]
    fn a_star_around_a_common_counterparty_is_not_a_cluster() {
        let mut g = Graph::default();
        let big = Node { d: 9, t: 0, x: 0, v: 50_000.0 };
        let leaves = ["0x1111111111111111111111111111111111111111", "0x2222222222222222222222222222222222222222"];
        let hubs = ["0x4444444444444444444444444444444444444444", "0x5555555555555555555555555555555555555555", "0x6666666666666666666666666666666666666666"];
        for a in leaves.iter().chain(hubs.iter()) {
            g.nodes.insert(a.to_string(), big.clone());
        }
        for l in leaves {
            for h in hubs {
                let k = if l < h { format!("{l}|{h}") } else { format!("{h}|{l}") };
                g.edges.insert(k, Edge { n: 1, u: 1000.0 });
            }
        }
        assert_eq!(g.cluster(0)["count"], json!(0), "no wallet-to-wallet link → no cartel");
    }

    #[test]
    fn the_frontier_pops_the_most_connected_node_first() {
        let mut g = Graph::default();
        g.nodes.insert(A.to_string(), Node { d: 1, t: 0, x: 0, v: 0.0 });
        g.nodes.insert(B.to_string(), Node { d: 9, t: 0, x: 0, v: 0.0 });
        g.frontier = vec![A.to_string(), B.to_string()];

        assert_eq!(g.next_node().as_deref(), Some(B), "hubs first");
    }
}
