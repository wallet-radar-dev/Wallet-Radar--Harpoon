//! «Структура групи капіталу та пов'язаних гаманців» — the layout engine behind
//! `GET /api/capital-network`.
//!
//! The reference picture is a radial cluster network: a yellow capital cluster in
//! the centre (its hub in the middle, the directly connected wallets around it),
//! blue secondary clusters around it, red highlighted wallets inside the main
//! cluster and green specially-marked connection nodes.
//!
//! Nothing here is drawn by hand. Every position comes from the real group
//! structure: how many wallets a group holds, how they are linked in the crawl
//! graph (`dataset/graph-db.json`), how many hops separate a cluster from the
//! capital group, and how many groups a wallet takes part in. A cluster that is
//! two hops away from the capital group is drawn on an outer ring and linked
//! through the intermediate wallet — the picture follows the data, not the
//! other way round.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Wallets of the capital group drawn radially around its hub (the reference
/// picture shows "approximately 150").
pub const MAIN_MEMBERS: usize = 150;
/// Members shipped for a secondary cluster of the top tier.
pub const CLUSTER_MEMBERS_TOP: usize = 60;
/// Members shipped for an ordinary secondary cluster.
pub const CLUSTER_MEMBERS: usize = 18;
/// How far we look for a path between the capital group and a cluster.
pub const MAX_HOPS: usize = 5;
/// Radius of the capital cluster (normalised units).
const MAIN_R: f64 = 1.0;
/// Ring radii for secondary clusters, by hop distance from the capital hub.
const RING_R: [f64; 3] = [1.55, 2.0, 2.45];
/// Golden angle — the spiral that spreads a cluster's wallets evenly.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

/// Undirected adjacency of the crawl graph (`dataset/graph-db.json`).
#[derive(Default)]
pub struct Net {
    adj: HashMap<String, Vec<(String, u64, f64)>>,
}

impl Net {
    pub fn new(adj: HashMap<String, Vec<(String, u64, f64)>>) -> Self {
        Self { adj }
    }

    pub fn degree(&self, a: &str) -> usize {
        self.adj.get(a).map_or(0, |v| v.len())
    }

    /// Direct edge between two wallets: (tx count, usd volume).
    pub fn edge(&self, a: &str, b: &str) -> Option<(u64, f64)> {
        self.adj.get(a)?.iter().find(|(n, _, _)| n == b).map(|(_, n, u)| (*n, *u))
    }

    /// Hop distance from `src` plus the first step on the shortest path — the
    /// wallet the path goes through. Bounded by `max_hops`.
    pub fn hops_from(&self, src: &str, max_hops: usize) -> HashMap<String, (usize, String)> {
        let mut out: HashMap<String, (usize, String)> = HashMap::new();
        out.insert(src.to_string(), (0, src.to_string()));
        let Some(start) = self.adj.get(src) else { return out };
        let mut queue: Vec<(String, usize, String)> = start
            .iter()
            .map(|(n, _, _)| (n.clone(), 1usize, n.clone()))
            .collect();
        while let Some((node, hop, via)) = queue.pop() {
            if out.contains_key(&node) {
                continue;
            }
            out.insert(node.clone(), (hop, via.clone()));
            if hop >= max_hops {
                continue;
            }
            if let Some(next) = self.adj.get(&node) {
                for (n, _, _) in next {
                    if !out.contains_key(n) {
                        queue.push((n.clone(), hop + 1, via.clone()));
                    }
                }
            }
        }
        out
    }
}

/// A research group: a capital group, a whale cluster or a discovered group.
#[derive(Clone)]
struct Grp {
    id: String,
    label: String,
    /// "capital_group" | "whale_cluster" | "discovered_group"
    kind: &'static str,
    hub: String,
    members: Vec<String>,
    member_count: usize,
    total_usd: f64,
    assets: Vec<String>,
}

impl Grp {
    /// Capital groups are yellow, everything else is a separate cluster (blue).
    fn color(&self) -> &'static str {
        if self.kind == "capital_group" {
            "yellow"
        } else {
            "blue"
        }
    }
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str()).filter(|x| !x.is_empty())
}

fn f(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[])
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    arr(v, key).iter().filter_map(|x| x.as_str().map(String::from)).collect()
}

/// Read the member addresses of a group — the shape differs per source:
/// capital groups store `{address, hop}`, whale clusters `{addr, …}`, discovered
/// groups plain strings.
fn member_addresses(v: &Value) -> Vec<String> {
    arr(v, "members")
        .iter()
        .filter_map(|m| {
            m.as_str()
                .map(String::from)
                .or_else(|| s(m, "address").map(String::from))
                .or_else(|| s(m, "addr").map(String::from))
        })
        .collect()
}

fn parse_capital_groups(v: &Value) -> Vec<Grp> {
    arr(v, "groups")
        .iter()
        .filter_map(|g| {
            let id = s(g, "id")?.to_string();
            let members = member_addresses(g);
            if members.is_empty() {
                return None;
            }
            let mut assets = str_list(g, "commonAssets");
            if assets.is_empty() {
                assets = str_list(g, "assets");
            }
            Some(Grp {
                label: s(g, "name").unwrap_or(&id).to_string(),
                id,
                kind: "capital_group",
                hub: members[0].clone(),
                members,
                member_count: g.get("size").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
                total_usd: f(g, "totalUsd"),
                assets,

            })
        })
        .collect()
}

fn parse_whale_clusters(v: &Value) -> Vec<Grp> {
    arr(v, "clusters")
        .iter()
        .filter_map(|c| {
            let id = s(c, "id")?.to_string();
            let members = member_addresses(c);
            if members.is_empty() {
                return None;
            }
            let hub = s(c, "key").map(String::from).unwrap_or_else(|| members[0].clone());
            let member_count = members.len();
            Some(Grp {
                label: s(c, "label").unwrap_or(&id).to_string(),
                id,
                kind: "whale_cluster",
                hub,
                members,
                member_count,
                total_usd: f(c, "combinedUsd"),
                assets: str_list(c, "assets"),

            })
        })
        .collect()
}

fn parse_discovered_groups(v: &Value) -> Vec<Grp> {
    arr(v, "groups")
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            let members = member_addresses(g);
            if members.is_empty() {
                return None;
            }
            let id = s(g, "id").map(String::from).unwrap_or_else(|| format!("DG-{i}"));
            Some(Grp {
                label: s(g, "label").map(String::from).unwrap_or_else(|| id.clone()),
                id,
                kind: "discovered_group",
                hub: members[0].clone(),
                members,
                member_count: g.get("size").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
                total_usd: f(g, "volumeUsd"),
                assets: vec![],

            })
        })
        .collect()
}

/// Name of a wallet — from the labels cache, else nothing.
fn label_of(labels: &Value, addr: &str) -> Option<String> {
    let e = labels.get(addr)?;
    let name = s(e, "name").unwrap_or("");
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Holdings of a wallet — from the forensics fingerprints (`tokens`, `totalUsd`).
fn holdings_of(fingerprints: &Value, addr: &str) -> (Vec<String>, f64) {
    let Some(e) = fingerprints.get(addr) else { return (vec![], 0.0) };
    (str_list(e, "tokens"), f(e, "totalUsd"))
}

/// How many of the group's own wallets a wallet is linked to.
fn internal_links(net: &Net, addr: &str, members: &HashSet<&String>) -> usize {
    net.adj.get(addr).map_or(0, |v| v.iter().filter(|(n, _, _)| members.contains(n)).count())
}

/// Spiral position of the `i`-th of `n` wallets around a centre.
/// `hollow` leaves an empty ring around the hub — the reference picture shows
/// the hub in the middle with the wallets around it, not a solid disc.
fn spiral(i: usize, n: usize, radius: f64, hollow: f64) -> (f64, f64) {
    let a = i as f64 * GOLDEN_ANGLE;
    let r = radius * (hollow + (1.0 - hollow) * ((i as f64 + 0.5) / n.max(1) as f64).sqrt());
    (r * a.cos(), r * a.sin())
}

/// The five slots of the reference picture (math angles, y up).
const SLOTS: [(&str, f64); 5] = [
    ("left", std::f64::consts::PI),
    ("upper_right", std::f64::consts::FRAC_PI_4),
    ("middle_right", 0.0),
    ("lower_right", -std::f64::consts::FRAC_PI_4),
    ("bottom_center", -std::f64::consts::FRAC_PI_2),
];

/// The three slots of the green connection nodes.
const SPECIAL_SLOTS: [(&str, f64); 3] = [
    ("top_center", std::f64::consts::FRAC_PI_2),
    ("lower_center", -std::f64::consts::FRAC_PI_2),
    ("lower_right", -std::f64::consts::FRAC_PI_4),
];

/// Build the whole capital-network picture.
///
/// `capital` — `dataset/capital-groups.json`, `whale` — `dataset/whale-clusters.json`,
/// `discovered` — `dataset/discovered-groups.json`, `conductors` — `dataset/conductors.json`,
/// `labels` — `dataset/labels-cache.json`, `fingerprints` — `dataset/asset-forensics.json`.
pub fn capital_network(
    capital: &Value,
    whale: &Value,
    discovered: &Value,
    conductors: &Value,
    labels: &Value,
    fingerprints: &Value,
    net: &Net,
) -> Value {
    let mut groups: Vec<Grp> = Vec::new();
    groups.extend(parse_capital_groups(capital));
    groups.extend(parse_whale_clusters(whale));
    groups.extend(parse_discovered_groups(discovered));

    // How many groups does each wallet take part in? That is the "function" a
    // wallet plays in the structure — a wallet in three groups is a bridge.
    let mut participation: HashMap<String, HashSet<String>> = HashMap::new();
    for g in &groups {
        for m in &g.members {
            participation.entry(m.clone()).or_default().insert(g.id.clone());
        }
    }

    // The capital group: the capital group with the largest capital. Whale
    // clusters are the fallback — they are "separate clusters" in the legend.
    let main_idx = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.kind == "capital_group" && g.total_usd > 0.0)
        .max_by(|(_, a), (_, b)| {
            a.total_usd
                .partial_cmp(&b.total_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.member_count.cmp(&b.member_count))
        })
        .map(|(i, _)| i)
        .or_else(|| {
            groups
                .iter()
                .enumerate()
                .filter(|(_, g)| g.kind == "whale_cluster")
                .max_by(|(_, a), (_, b)| {
                    a.total_usd.partial_cmp(&b.total_usd).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
        });

    let Some(main_idx) = main_idx else {
        return json!({
            "generated": 0,
            "error": "no groups",
            "groups": [], "edges": [], "crossLinks": [],
            "special": {"highlighted": [], "connectionNodes": []},
            "legend": legend(),
        });
    };

    // The hub of the capital group: the member with the most links in the
    // crawl graph — the wallet the whole cluster hangs on. Links inside the
    // group break the tie, so the hub is stable and not an arbitrary wallet.
    let main = groups[main_idx].clone();
    let member_set: HashSet<&String> = main.members.iter().collect();
    let mut hub_candidates: Vec<&String> = main.members.iter().collect();
    hub_candidates.sort_by(|a, b| {
        // Спершу зв'язки ВСЕРЕДИНІ групи — саме на такому гаманці тримається
        // кластер; загальна кількість зв'язків лише розв'язує рівність.
        let ka = (internal_links(net, a, &member_set), net.degree(a));
        let kb = (internal_links(net, b, &member_set), net.degree(b));
        kb.cmp(&ka).then_with(|| a.cmp(b))
    });
    let hub = hub_candidates
        .first()
        .map(|m| (*m).clone())
        .unwrap_or_else(|| main.members[0].clone());

    // Hops from the capital hub over the whole crawl graph.
    let hops = net.hops_from(&hub, MAX_HOPS);

    // Radial members of the capital group: the group's own wallets plus the
    // wallets directly linked to the hub — "directly connected wallets".
    let mut radial: Vec<(String, u64)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for m in &main.members {
        if seen.insert(m.clone()) {
            radial.push((m.clone(), net.edge(&hub, m).map_or(0, |(n, _)| n)));
        }
    }
    if let Some(nbrs) = net.adj.get(&hub) {
        for (n, tx, _) in nbrs {
            if seen.insert(n.clone()) {
                radial.push((n.clone(), *tx));
            }
        }
    }
    radial.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| net.degree(&b.0).cmp(&net.degree(&a.0))));
    radial.truncate(MAIN_MEMBERS);

    // Internal degree inside the capital group — how many other wallets of the
    // cluster this one is linked to. That is what makes a node "highlighted".
    let radial_set: HashSet<&String> = radial.iter().map(|(a, _)| a).collect();
    let internal_deg: HashMap<&str, usize> = radial
        .iter()
        .map(|(a, _)| {
            let d = net.adj.get(a).map_or(0, |v| {
                v.iter().filter(|(n, _, _)| radial_set.contains(n)).count()
            });
            (a.as_str(), d)
        })
        .collect();

    let mut members_json: Vec<Value> = Vec::with_capacity(radial.len());
    let mut highlighted: Vec<Value> = Vec::new();
    for (i, (addr, tx)) in radial.iter().enumerate() {
        let (x, y) = spiral(i, radial.len(), MAIN_R, 0.45);
        let role = if *addr == hub {
            "hub"
        } else if *tx > 0 {
            "core"
        } else {
            "peripheral"
        };
        let idist = internal_deg.get(addr.as_str()).copied().unwrap_or(0);
        let part = participation.get(addr).map_or(0, HashSet::len);
        let is_hub = *addr == hub;
        let (tokens, balance) = holdings_of(fingerprints, addr);
        let node = json!({
            "address": addr,
            "label": label_of(labels, addr),
            "role": role,
            "degree": net.degree(addr),
            "internalDegree": idist,
            "participation": part,
            "txWithHub": *tx,
            "hops": hops.get(addr).map(|(h, _)| *h),
            "tokens": tokens,
            "balanceUsd": balance,
            "pos": {"x": x, "y": y, "z": 0.0},
        });
        if idist >= 3 || part >= 2 {
            // Хаб — і так центральний вузол; червоніми лишаємо решту виділених.
            if *addr != hub {
                highlighted.push(json!({
                    "address": addr,
                    "label": label_of(labels, addr),
                    "reason": if part >= 2 { "multi_group" } else { "multi_link" },
                    "internalDegree": idist,
                    "participation": part,
                    "degree": net.degree(addr),
                    "balanceUsd": balance,
                }));
            }
        }
        if is_hub {
            let (tokens, balance) = holdings_of(fingerprints, addr);
            members_json.insert(0, json!({
                "address": addr,
                "label": label_of(labels, addr),
                "role": "hub",
                "degree": net.degree(addr),
                "internalDegree": idist,
                "participation": part,
                "txWithHub": 0,
                "hops": 0,
                "tokens": tokens,
                "balanceUsd": balance,
                "pos": {"x": 0.0, "y": 0.0, "z": 0.0},
            }));
        } else {
            members_json.push(node);
        }
    }
    highlighted.sort_by(|a, b| {
        f(b, "internalDegree")
            .partial_cmp(&f(a, "internalDegree"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    highlighted.truncate(12);

    // --- secondary clusters -------------------------------------------------
    let mut others: Vec<(usize, Grp)> = groups
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != main_idx)
        .map(|(i, g)| (i, g.clone()))
        .collect();

    // Hop distance from the capital group: the closest member decides.
    // Three ways to be close: a direct edge to a wallet of the capital group,
    // a wallet that belongs to both groups, or a path through the crawl graph.
    let main_members: HashSet<&String> = main.members.iter().collect();
    let mut group_hops: Vec<(usize, usize, String, String)> = Vec::with_capacity(others.len());
    for (i, g) in others.iter() {
        // Пряме ребро: член кластера зв'язаний із гаманцем групи капіталу.
        let direct = g.members.iter().find_map(|m| {
            net.adj
                .get(m)?
                .iter()
                .find(|(n, _, _)| radial_set.contains(n))
                .map(|(_, tx, _)| (m.clone(), *tx))
        });
        if let Some((m, tx)) = direct {
            group_hops.push((*i, 1, m, tx.to_string()));
            continue;
        }
        // Спільний гаманець: той самий кошелєк у двох групах.
        if g.members.iter().any(|m| main_members.contains(m)) {
            group_hops.push((*i, 1, String::new(), String::new()));
            continue;
        }
        let best = g
            .members
            .iter()
            .filter_map(|m| hops.get(m).map(|(h, via)| (*h, m.clone(), via.clone())))
            .min_by_key(|(h, _, _)| *h);
        match best {
            Some((h, m, via)) => group_hops.push((*i, h, m, via)),
            None => group_hops.push((*i, MAX_HOPS + 1, String::new(), String::new())),
        }
    }

    // Order: the closer the cluster, the more central it sits.
    others.sort_by(|a, b| {
        let ha = group_hops.iter().find(|(i, _, _, _)| *i == a.0).map(|(_, h, _, _)| *h).unwrap_or(9);
        let hb = group_hops.iter().find(|(i, _, _, _)| *i == b.0).map(|(_, h, _, _)| *h).unwrap_or(9);
        ha.cmp(&hb)
            .then_with(|| b.1.total_usd.partial_cmp(&a.1.total_usd).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| b.1.member_count.cmp(&a.1.member_count))
    });

    // Слоти референсної картинки: чотири сині — «окремі кластери пов'язаних
    // гаманців» (whale-кластери), п'ятий жовтий — інша група капіталу.
    let mut slot_plan: HashMap<usize, usize> = HashMap::new();
    {
        let hop_of = |i: usize| group_hops.iter().find(|(x, _, _, _)| *x == i).map(|(_, h, _, _)| *h).unwrap_or(9);
        let mut linked: Vec<&(usize, Grp)> = others
            .iter()
            .filter(|(i, _)| hop_of(*i) <= 1)
            .collect();
        linked.sort_by(|a, b| b.1.total_usd.partial_cmp(&a.1.total_usd).unwrap_or(std::cmp::Ordering::Equal));
        let mut used: HashSet<usize> = HashSet::new();
        // Слоти 0..3 — whale-кластери (сині), слот 4 — група капіталу (жовта).
        for slot in 0..4usize {
            if let Some((i, _)) = linked.iter().find(|(i, g)| g.kind == "whale_cluster" && !used.contains(i)) {
                used.insert(*i);
                slot_plan.insert(*i, slot);
            }
        }
        // Жовтий слот — найкапітальніша інша група капіталу.
        if let Some((i, _)) = linked.iter().find(|(i, g)| g.kind == "capital_group" && !used.contains(i)) {
            used.insert(*i);
            slot_plan.insert(*i, 4);
        }
        // Якщо слотів не вистачило — добираємо рештою за капіталом.
        let mut rest: Vec<usize> = linked
            .iter()
            .filter(|(i, _)| !used.contains(i))
            .map(|(i, _)| *i)
            .collect();
        for slot in 0..SLOTS.len() {
            if slot_plan.values().any(|s| *s == slot) {
                continue;
            }
            if let Some(i) = rest.first().copied() {
                rest.remove(0);
                slot_plan.insert(i, slot);
            }
        }
    }

    // Rings: hop 1 on the inner ring, hop 2 in the middle, the rest outside.
    let mut ring_counts = [0usize; 3];
    let mut clusters_json: Vec<Value> = Vec::with_capacity(others.len());
    let mut cross_links: Vec<Value> = Vec::new();
    let mut hub_of_cluster: Vec<(String, String)> = Vec::new(); // (cluster hub, main hub)

    for (rank, (idx, g)) in others.iter().enumerate() {
        let hop = group_hops
            .iter()
            .find(|(i, _, _, _)| i == idx)
            .map(|(_, h, _, _)| *h)
            .unwrap_or(MAX_HOPS + 1);
        // Rings: a cluster that touches the capital hub (0 or 1 hop) sits on the
        // inner ring, two hops away in the middle, the rest outside.
        let ring = match hop {
            0 | 1 => 0,
            2 => 1,
            _ => 2,
        };
        // Слот із плану (референсна картинка) — на внутрішньому кільці й під
        // своїм кутом; решта внутрішнього кільця ділить його рівномірно.
        let (angle, radius, slot_name) = if let Some(slot) = slot_plan.get(idx) {
            let s = SLOTS[*slot];
            (s.1, RING_R[0], s.0.to_string())
        } else {
            let n = ring_counts[ring];
            ring_counts[ring] += 1;
            let count = (group_hops.iter().filter(|(_, h, _, _)| match h { 0 | 1 => 0, 2 => 1, _ => 2 } == ring).count()).max(1);
            let a = std::f64::consts::TAU * (n as f64) / (count as f64);
            (a, RING_R[ring], format!("ring_{ring}"))
        };
        let (gx, gy) = (radius * angle.cos(), radius * angle.sin());
        let gz = ring as f64 * 0.45;

        // Members of the cluster, drawn around their own hub. A cluster with no
        // visible path to the capital group gets only a few members — it is
        // far away, and a full fan of them would bury the actual structure.
        let linked = hop <= MAX_HOPS;
        let shown = if !linked {
            4
        } else if rank < 10 {
            CLUSTER_MEMBERS_TOP
        } else {
            CLUSTER_MEMBERS
        };
        let mut mlist: Vec<&String> = g.members.iter().collect();
        mlist.sort_by_key(|m| -(net.degree(m) as i64));
        mlist.truncate(shown);
        let member_r = radius * 0.23;
        let mut members: Vec<Value> = Vec::with_capacity(mlist.len());
        for (i, m) in mlist.iter().enumerate() {
            let (mx, my) = spiral(i, mlist.len(), member_r, 0.12);
            let (tokens, balance) = holdings_of(fingerprints, m);
            members.push(json!({
                "address": m,
                "label": label_of(labels, m),
                "role": if **m == g.hub { "hub" } else { "member" },
                "degree": net.degree(m),
                "participation": participation.get(*m).map_or(0, HashSet::len),
                "hops": hops.get(*m).map(|(h, _)| *h),
                "tokens": tokens,
                "balanceUsd": balance,
                "pos": {"x": gx + mx, "y": gy + my, "z": gz},
            }));
        }

        // Cross-cluster link: the real edge (or path) that leads to the capital
        // group. A cluster with no path within MAX_HOPS stays unlinked — we
        // do not draw a connection we cannot see in the data.
        let mut link: Option<Value> = None;
        if hop <= MAX_HOPS {
            let (m_addr, via_addr) = group_hops
                .iter()
                .find(|(i, _, _, _)| *i == *idx)
                .map(|(_, _, m, v)| (m.clone(), v.clone()))
                .unwrap_or_default();
            let weight = net.edge(&m_addr, &hub).map_or(0, |(n, _)| n);
            let mut l = json!({
                "from": g.hub, "to": hub, "weight": weight,
                "hops": hop, "kind": "cross_cluster",
            });
            if !m_addr.is_empty() {
                l["via"] = json!(m_addr);
            }
            if !via_addr.is_empty() && hop > 1 {
                l["through"] = json!(via_addr);
            }
            link = Some(l);
        }
        if let Some(l) = link {
            cross_links.push(l);
            hub_of_cluster.push((g.hub.clone(), hub.clone()));
        }

        clusters_json.push(json!({
            "id": g.id,
            "label": g.label,
            "kind": g.kind,
            "color": g.color(),
            "size": g.member_count,
            "memberCount": g.members.len(),
            "shownMembers": members.len(),
            "totalUsd": g.total_usd,
            "assets": g.assets,
            "hops": hop,
            "linked": linked,
            "position": slot_name,
            "hub": {
                "address": g.hub,
                "label": label_of(labels, &g.hub),
                "degree": net.degree(&g.hub),
                "balanceUsd": holdings_of(fingerprints, &g.hub).1,
                "pos": {"x": gx, "y": gy, "z": gz},
            },
            "pos": {"x": gx, "y": gy, "z": gz},
            "members": members,
        }));
    }

    // --- edges -------------------------------------------------------------
    let mut edges: Vec<Value> = Vec::new();
    for m in radial.iter() {
        if let Some((n, u)) = net.edge(&hub, &m.0) {
            edges.push(json!({
                "from": hub, "to": m.0, "weight": n, "volumeUsd": u, "kind": "direct_wallet",
            }));
        }
    }
    // Wallet-to-wallet links inside the capital group (the "multiple
    // interconnected lines" of the reference picture).
    let mut inner: Vec<(u64, &str, &str)> = Vec::new();
    for (a, _) in radial.iter() {
        if let Some(nbrs) = net.adj.get(a) {
            for (b, n, _u) in nbrs {
                if a < b && radial_set.contains(b) {
                    inner.push((*n, a.as_str(), b.as_str()));
                }
            }
        }
    }
    inner.sort_by(|a, b| b.0.cmp(&a.0));
    inner.truncate(600);
    for (n, a, b) in inner {
        edges.push(json!({"from": a, "to": b, "weight": n, "kind": "inter_wallet"}));
    }

    // --- green connection nodes -------------------------------------------
    let mut connection_nodes: Vec<Value> = Vec::new();
    let mut conn_edges: Vec<Value> = Vec::new();
    let mut cands: Vec<(String, Vec<String>, usize, usize, f64)> = Vec::new();
    for c in arr(conductors, "conductors") {
        let Some(funder) = s(c, "funder") else { continue };
        let holders = str_list(c, "holders");
        let mut groups_of: Vec<String> = Vec::new();
        for h in &holders {
            if let Some(gs) = participation.get(h) {
                for g in gs {
                    if !groups_of.contains(g) {
                        groups_of.push(g.clone());
                    }
                }
            }
        }
        if groups_of.len() >= 2 {
            cands.push((
                funder.to_string(),
                groups_of,
                c.get("holderCount").and_then(|x| x.as_u64()).unwrap_or(holders.len() as u64) as usize,
                c.get("assetCount").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
                f(c, "maxBackerUsd"),
            ));
        }
    }
    cands.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    for (i, (funder, groups_of, holders, asset_count, usd)) in cands.iter().enumerate().take(24) {
        let (angle, radius) = if i < SPECIAL_SLOTS.len() {
            (SPECIAL_SLOTS[i].1, 1.25)
        } else {
            let n = i - SPECIAL_SLOTS.len();
            (std::f64::consts::TAU * (n as f64) / 12.0, 1.25)
        };
        let x = radius * angle.cos();
        let y = radius * angle.sin();
        connection_nodes.push(json!({
            "address": funder,
            "label": label_of(labels, funder),
            "groups": groups_of,
            "holderCount": holders,
            "assetCount": asset_count,
            "maxBackerUsd": usd,
            "position": if i < SPECIAL_SLOTS.len() { SPECIAL_SLOTS[i].0 } else { "ring" },
            "pos": {"x": x, "y": y, "z": 0.6},
        }));
        conn_edges.push(json!({"from": funder, "to": hub, "weight": holders, "kind": "special_connection"}));
    }

    let (hub_tokens, hub_balance) = holdings_of(fingerprints, &hub);
    let max_hops = group_hops.iter().map(|(_, h, _, _)| *h).max().unwrap_or(0);
    let linked_groups = group_hops.iter().filter(|(_, h, _, _)| *h <= MAX_HOPS).count();
    let stats = json!({
        "groups": groups.len(),
        "capitalGroups": groups.iter().filter(|g| g.kind == "capital_group").count(),
        "whaleClusters": groups.iter().filter(|g| g.kind == "whale_cluster").count(),
        "discoveredGroups": groups.iter().filter(|g| g.kind == "discovered_group").count(),
        "nodes": members_json.len() + clusters_json.len(),
        "edges": edges.len() + cross_links.len() + conn_edges.len(),
        "mainMembers": members_json.len(),
        "graphNodes": net.adj.len(),
    });

    json!({
        "generated": now_ms(),
        "blockchain": "Ethereum",
        "visualization": "radial_cluster_network",
        "main": {
            "id": main.id,
            "label": main.label,
            "kind": main.kind,
            "color": "yellow",
            "position": "center",
            "size": main.member_count,
            "memberCount": main.members.len(),
            "shownMembers": members_json.len(),
            "totalUsd": main.total_usd,
            "assets": main.assets,
            "hub": {
                "address": hub,
                "label": label_of(labels, &hub),
                "degree": net.degree(&hub),
                "tokens": hub_tokens,
                "balanceUsd": hub_balance,
                "pos": {"x": 0.0, "y": 0.0, "z": 0.0},
            },
            "pos": {"x": 0.0, "y": 0.0, "z": 0.0},
            "members": members_json,
        },
        "groups": clusters_json,
        "edges": edges,
        "crossLinks": cross_links,
        "special": {
            "highlighted": highlighted,
            "connectionNodes": connection_nodes,
        },
        "specialEdges": conn_edges,
        "legend": legend(),
        "stats": stats,
        "analysis": json!({
            "mainObservation": "Велика центральна група гаманців має численні зв'язки з декількома меншими групами гаманців.",
            "networkTopology": "hub_and_spoke_with_cross_cluster_connections",
            "hierarchy": [
                "Основна група капіталу",
                "Центральний гаманець",
                "Безпосередньо пов'язані гаманці",
                "Вторинні групи гаманців",
                "Зовнішні пов'язані гаманці",
            ],
            "purpose": "Візуалізація структури групи капіталу, розподілу гаманців та взаємозв'язків між ними",
            "linkedGroups": linked_groups,
            "maxHops": max_hops,
        }),
        "dataReliability": json!({
            "visibleData": "Частково доступні",
            "walletOwnership": "Не підтверджено",
            "transactionDirection": "Не встановлено",
            "relationships": "Відображені графічно, але їхня економічна природа невідома",
            "exactWalletCount": "Пораховано з оригінальних даних",
            "note": "Кількість гаманців у групі — фактична; зв'язки — реальні ребра крафт-графа (dataset/graph-db.json). Хомільність груп визначається за спільними гаманцями та спільними фундерами.",
        }),
    })
}

fn legend() -> Value {
    json!({
        "yellow": "Гаманці основної групи капіталу",
        "blue": "Окремі кластери пов'язаних гаманців",
        "red": "Виділені вузли та зв'язки",
        "green": "Спеціально позначені вузли",
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net_from(pairs: &[(&str, &str)]) -> Net {
        let mut adj: HashMap<String, Vec<(String, u64, f64)>> = HashMap::new();
        for (a, b) in pairs {
            adj.entry((*a).to_string()).or_default().push(((*b).to_string(), 1, 10.0));
            adj.entry((*b).to_string()).or_default().push(((*a).to_string(), 1, 10.0));
        }
        Net::new(adj)
    }

    const CAP: &str = r#"{"groups":[
      {"id":"CAP1","name":"Capital one","size":3,"totalUsd":1000.0,"commonAssets":["ETH"],
       "members":[{"address":"0xaaa"},{"address":"0xbbb"},{"address":"0xccc"}]},
      {"id":"CAP2","name":"Capital two","size":2,"totalUsd":500.0,"members":[{"address":"0xddd"},{"address":"0xeee"}]}
    ]}"#;

    #[test]
    fn capital_group_becomes_the_centre() {
        let cap: Value = serde_json::from_str(CAP).unwrap();
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xaaa", "0xccc"), ("0xbbb", "0xddd")]);
        let out = capital_network(&cap, &json!({}), &json!({}), &json!({}), &json!({}), &json!({}), &net);
        assert_eq!(out["main"]["id"], "CAP1");
        assert_eq!(out["main"]["color"], "yellow");
        assert_eq!(out["main"]["hub"]["address"], "0xaaa");
        // the second capital group is a secondary cluster and stays yellow
        let sec = out["groups"].as_array().unwrap();
        assert_eq!(sec.len(), 1);
        assert_eq!(sec[0]["id"], "CAP2");
        assert_eq!(sec[0]["color"], "yellow");
    }

    #[test]
    fn whale_clusters_are_blue_and_linked() {
        let cap: Value = serde_json::from_str(CAP).unwrap();
        let whale = json!({"clusters":[{"id":"WC-1","label":"Whale","key":"0xbbb","combinedUsd":900.0,
            "members":[{"addr":"0xbbb"},{"addr":"0xccc"}],"evidence":{"sharedFunders":[{"funder":"0xfunder","holders":2}]}}]});
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xaaa", "0xccc")]);
        let out = capital_network(&cap, &whale, &json!({}), &json!({}), &json!({}), &json!({}), &net);
        let wc = out["groups"].as_array().unwrap().iter().find(|g| g["id"] == "WC-1").unwrap();
        assert_eq!(wc["color"], "blue");
        assert_eq!(wc["hops"], 1);
        assert_eq!(out["crossLinks"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn farther_clusters_land_on_outer_rings() {
        let cap: Value = serde_json::from_str(CAP).unwrap();
        let whale = json!({"clusters":[
            {"id":"WC-1","label":"W1","key":"0xbbb","combinedUsd":900.0,"members":[{"addr":"0xbbb"}]},
            {"id":"WC-2","label":"W2","key":"0xeee","combinedUsd":800.0,"members":[{"addr":"0xeee"}]}
        ]});
        // 0xeee is two hops away from the hub 0xaaa
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xaaa", "0xccc"), ("0xbbb", "0xddd"), ("0xddd", "0xeee")]);
        let out = capital_network(&cap, &whale, &json!({}), &json!({}), &json!({}), &json!({}), &net);
        let groups = out["groups"].as_array().unwrap();
        let w1 = groups.iter().find(|g| g["id"] == "WC-1").unwrap();
        let w2 = groups.iter().find(|g| g["id"] == "WC-2").unwrap();
        assert_eq!(w1["hops"], 1);
        assert_eq!(w2["hops"], 3);
        // the outer ring is further from the centre
        let r1 = w1["pos"]["x"].as_f64().unwrap().abs() + w1["pos"]["y"].as_f64().unwrap().abs();
        let r2 = w2["pos"]["x"].as_f64().unwrap().abs() + w2["pos"]["y"].as_f64().unwrap().abs();
        assert!(r2 > r1, "farther cluster must sit further out: {r1} vs {r2}");
    }

    #[test]
    fn highlighted_wallets_are_the_ones_with_many_links_inside() {
        let cap: Value = serde_json::from_str(CAP).unwrap();
        let net = net_from(&[
            ("0xaaa", "0xbbb"), ("0xaaa", "0xccc"), ("0xbbb", "0xccc"), ("0xbbb", "0xddd"), ("0xccc", "0xddd"),
        ]);
        let out = capital_network(&cap, &json!({}), &json!({}), &json!({}), &json!({}), &json!({}), &net);
        let hl = out["special"]["highlighted"].as_array().unwrap();
        assert!(!hl.is_empty());
        assert!(hl.iter().any(|h| h["address"] == "0xbbb"));
    }
}
