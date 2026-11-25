//! «Структура групи капіталу» — the layout engine behind
//! `GET /api/capital-network`.
//!
//! The screen shows **one capital group** as a single flower: a hub in the
//! middle, the group's wallets around it and thin spokes from the hub to each
//! of them. Inside the group every wallet has a role, and the roles are what
//! the colours and the edges mean:
//!
//!   * gold  `#E8B85A` — the hub of the group and its ordinary wallets
//!     («проміжні»);
//!   * red   `#E85A5A` — the core of the group («ядро»): the wallets that hold
//!     the group's assets, the ones that pump them;
//!   * green `#5CE08A` — the wallets that came before the pump
//!     («передпампи»): the funders of those core wallets.
//!
//! Nothing is drawn by hand. The group, its hub, the core wallets and the
//! pre-pump wallets all come from the real data: the capital groups
//! (`dataset/capital-groups.json`), the asset scan with shares and common
//! funders (`dataset/wallet-asset-scan.json`), the holdings
//! (`dataset/asset-forensics.json`) and the crawl graph
//! (`dataset/graph-db.json`).

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Wallets of the capital group drawn around its hub — the whole group, not a
/// sample (the biggest groups hold a thousand wallets).
pub const MAIN_MEMBERS: usize = 7000;
/// How many wallets are marked as the core («ядро») of the group.
pub const MAX_CORE: usize = 3;
/// How many pre-pump wallets («передпампи») are drawn.
pub const MAX_PREPUMP: usize = 3;
/// How many highlight edges a single core wallet draws.
pub const MAX_HIGHLIGHT_LINKS: usize = 48;

const C_HUB: &str = "#E8B85A";
const C_HUB_STROKE: &str = "#C99A3E";
/// The wallets around the hub — a shade deeper than the hub, as in the reference.
const C_SAT: &str = "#D9A441";
const C_SAT_STROKE: &str = "#B8862B";
const C_CORE: &str = "#E85A5A";
const C_CORE_STROKE: &str = "#B83E3E";
const C_PREPUMP: &str = "#5CE08A";
const C_PREPUMP_STROKE: &str = "#3DA865";
/// Spokes of the flower.
const C_SPOKE: &str = "#4A6FA5";

/// Seed anchor of the capital group (normalised, y down).
const MAIN_ANCHOR: (f64, f64) = (0.45, 0.50);
/// The ring of wallets around the hub is stretched sideways: the flower reads
/// as a wide fan with the core in the middle, not as a narrow circle.
const RING_RX: f64 = 0.45;
const RING_RY: f64 = 0.16;
/// Radius of the tight core bundle (the pump wallets).
const CORE_R: f64 = 0.045;
/// How far the second flower hangs below the core — right under it, so the
/// whole picture stays flat and wide.
const COHORT_DROP: f64 = 0.06;
/// The cohort bunch is a small wide ellipse of its own.
const COHORT_RX: f64 = 0.13;
const COHORT_RY: f64 = 0.045;

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

    /// Neighbours of a wallet with the number of transfers and the usd volume.
    pub fn neighbours(&self, a: &str) -> &[(String, u64, f64)] {
        self.adj.get(a).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// A research group: a capital group, a whale cluster or a discovered group.
#[derive(Clone)]
struct Grp {
    id: String,
    label: String,
    /// "capital_group" | "whale_cluster" | "discovered_group"
    kind: &'static str,
    members: Vec<String>,
    /// Відстань від ядра групи (BFS): 0 — ядро, 1 і далі — проміжні.
    hops: HashMap<String, usize>,
    member_count: usize,
    total_usd: f64,
    assets: Vec<String>,
}

impl Grp {
    /// Does the group have a core — the BFS roots (hop 0)?
    fn has_core(&self) -> bool {
        self.hops.values().any(|h| *h == 0)
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

/// The full membership of the groups — `dataset/backfill/group-membership.json`.
///
/// The capital groups file keeps only a sample of the wallets (a thousand per
/// group), this one has every wallet with the BFS hop from the group core.
fn membership_index(v: &Value) -> HashMap<String, Vec<(String, Option<usize>)>> {
    let mut index: HashMap<String, Vec<(String, Option<usize>)>> = HashMap::new();
    let wallets = match v.get("wallets").and_then(|x| x.as_object()) {
        Some(w) => w,
        None => return index,
    };
    for (addr, e) in wallets {
        let group = match e.get("group").and_then(|x| x.as_str()) {
            Some(g) => g.to_string(),
            None => continue,
        };
        let hop = e.get("hop").and_then(|x| x.as_u64()).map(|h| h as usize);
        index.entry(group).or_default().push((addr.to_string(), hop));
    }
    index
}

fn parse_capital_groups(v: &Value, membership: &HashMap<String, Vec<(String, Option<usize>)>>) -> Vec<Grp> {
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
            let mut hops: HashMap<String, usize> = HashMap::new();
            for m in arr(g, "members") {
                if let Some(addr) = s(m, "address") {
                    if let Some(h) = m.get("hop").and_then(|x| x.as_u64()) {
                        hops.insert(addr.to_string(), h as usize);
                    }
                }
            }
            // The full membership of the group — the capital groups file only
            // keeps a sample of the wallets.
            let mut members = members;
            if let Some(full) = membership.get(&id) {
                if full.len() > members.len() {
                    members = full.iter().map(|(a, _)| a.clone()).collect();
                    for (a, h) in full {
                        if let Some(h) = h {
                            hops.insert(a.clone(), *h);
                        }
                    }
                }
            }
            Some(Grp {
                label: s(g, "name").unwrap_or(&id).to_string(),
                id,
                kind: "capital_group",
                members,
                hops,
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
            Some(Grp {
                label: s(c, "label").unwrap_or(&id).to_string(),
                id,
                kind: "whale_cluster",
                members,
                hops: HashMap::new(),
                member_count: 0,
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
                members,
                hops: HashMap::new(),
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
fn internal_links(net: &Net, addr: &str, members: &HashSet<String>) -> usize {
    net.adj.get(addr).map_or(0, |v| v.iter().filter(|(n, _, _)| members.contains(n)).count())
}

/// The set of the group's wallets — for fast membership checks.
fn all_members_set(members: &[String]) -> HashSet<String> {
    members.iter().cloned().collect()
}

/// The angles of `n` points spread evenly along the arc of an ellipse.
///
/// The points have to be even along the *arc*, not along the angle — on a
/// wide ellipse an even angle crowds them at the two ends.
fn ellipse_angles(rx: f64, ry: f64, n: usize) -> Vec<f64> {
    const STEPS: usize = 720;
    let mut cum: Vec<f64> = Vec::with_capacity(STEPS + 1);
    let mut total = 0.0;
    let mut prev = (rx, 0.0);
    cum.push(0.0);
    for k in 1..=STEPS {
        let t = k as f64 / STEPS as f64 * std::f64::consts::TAU;
        let p = (rx * t.cos(), ry * t.sin());
        total += ((p.0 - prev.0).powi(2) + (p.1 - prev.1).powi(2)).sqrt();
        cum.push(total);
        prev = p;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let target = (i as f64 + 0.5) / n.max(1) as f64 * total;
        let idx = cum.partition_point(|&v| v < target).clamp(1, STEPS);
        out.push((idx - 1) as f64 / STEPS as f64 * std::f64::consts::TAU);
    }
    out
}

/// Seed position of the `i`-th of `n` wallets around a centre — a golden-angle
/// spiral on an ellipse, so the client simulation starts from an even wide
/// ring instead of a pile.
fn seed_ring(i: usize, n: usize, cx: f64, cy: f64, rx: f64, ry: f64) -> (f64, f64) {
    let a = i as f64 * 2.399_963_229_728_653;
    let r = 0.62 + 0.38 * ((i as f64 + 0.5) / n.max(1) as f64).sqrt();
    (cx + rx * r * a.cos(), cy + ry * r * a.sin())
}

/// Where the core stands — under the hub, at the heart of the flower, as in
/// the reference picture (the red node is a little below the hub).
const CORE_OFFSET: (f64, f64) = (-0.005, 0.035);
/// The centre of the core bundle.
fn core_center() -> (f64, f64) {
    (MAIN_ANCHOR.0 + CORE_OFFSET.0, MAIN_ANCHOR.1 + CORE_OFFSET.1)
}

/// Where a pre-pump wallet stands — outside the core bundle, in three slots.
fn prepump_slot(i: usize) -> (f64, f64) {
    let a = match i {
        0 => std::f64::consts::FRAC_PI_2,          // top
        1 => -std::f64::consts::FRAC_PI_4,          // bottom-right
        _ => std::f64::consts::PI * 0.75,           // top-left
    };
    let (cx, cy) = core_center();
    let r = 0.095;
    (cx + r * a.cos(), cy + r * a.sin())
}

/// Roles inside one capital group.
struct Roles {
    /// «ядро» — the wallets that hold the group's assets and pump them.
    core: Vec<(String, f64)>,
    /// «передпампи» — the funders that came before the pump.
    prepump: Vec<(String, u64, f64)>,
}

/// How many co-holders of one token are drawn around a core wallet.
const MAX_COHORT: usize = 14;

/// Read the roles out of the asset scan: who of the group holds a share of
/// which asset, who funded those holders and who holds the same tokens
/// together with them (the cohort — the dense bundle of the core).
fn read_roles(
    asset_scan: &Value,
    members: &HashSet<String>,
    core_pool: &HashSet<String>,
) -> (
    Roles,
    HashMap<String, f64>,
    HashMap<String, Vec<(String, f64)>>,
) {
    let mut share: HashMap<String, f64> = HashMap::new();
    let mut assets_of: HashMap<String, HashSet<String>> = HashMap::new();
    // wallet -> the wallets that hold the same tokens (with their shares)
    let mut cohort: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    // funder -> (holders of ours it funded, money it moved, our core wallets)
    let mut funders: HashMap<String, (HashSet<String>, f64, u64, u64)> = HashMap::new();

    for asset in arr(asset_scan, "assets") {
        let symbol = s(asset, "symbol").unwrap_or("").to_string();
        for grp in arr(asset, "groups") {
            // Every holder of this token — the cohort of the core wallet.
            let all: Vec<(String, f64)> = arr(grp, "members")
                .iter()
                .filter_map(|m| {
                    let addr = s(m, "addr").or_else(|| s(m, "address"))?;
                    Some((addr.to_string(), f(m, "share")))
                })
                .collect();
            let holders: Vec<(String, f64)> = all
                .iter()
                .filter(|(a, _)| members.contains(a))
                .cloned()
                .collect();
            if holders.is_empty() {
                continue;
            }
            for (addr, sh) in &holders {
                *share.entry(addr.clone()).or_insert(0.0) += *sh;
                assets_of.entry(addr.clone()).or_default().insert(symbol.clone());
                // The co-holders of the same token: they move in the same
                // basket, so they hang around the core wallet.
                let entry = cohort.entry(addr.clone()).or_default();
                for (a, s) in &all {
                    if a == addr || entry.iter().any(|(x, _)| x == a) {
                        continue;
                    }
                    entry.push((a.clone(), *s));
                }
            }
            // The funders of these holders — the wallets that were there before.
            for funder in arr(grp, "commonFunders") {
                let f_addr = match s(funder, "funder") {
                    Some(a) => a.to_string(),
                    None => continue,
                };
                let entry = funders
                    .entry(f_addr.clone())
                    .or_insert_with(|| (HashSet::new(), 0.0, 0, 0));
                for (addr, _) in &holders {
                    if entry.0.insert(addr.clone()) {
                        entry.2 += 1;
                    }
                    if core_pool.contains(addr) {
                        entry.3 += 1;
                    }
                }
                entry.1 += f(funder, "avgUsd");
            }
        }
    }

    // The cohort of every wallet, the biggest shares first.
    for list in cohort.values_mut() {
        list.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        list.truncate(MAX_COHORT);
    }

    // The core: the wallets of the group that hold its assets and pump them
    // (the biggest shares) plus the BFS roots of the group — when the asset
    // scan knows nothing about it.
    let mut core: Vec<(String, f64)> = if share.is_empty() {
        core_pool
            .iter()
            .map(|a| (a.clone(), 0.0))
            .collect()
    } else {
        let mut all: Vec<(String, f64)> = share.iter().map(|(a, s)| (a.clone(), *s)).collect();
        // The roots of the group come along, so a group with a BFS structure
        // shows its core even when the assets are not scanned.
        for a in core_pool.iter() {
            if !all.iter().any(|(x, _)| x == a) {
                all.push((a.clone(), share.get(a).copied().unwrap_or(0.0)));
            }
        }
        all
    };
    core.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    core.truncate(MAX_CORE);

    // The pre-pump wallets: the funders that came before the core wallets.
    // The closer a funder is to the core (it funded the core wallet itself),
    // the earlier it stands in the chain.
    let core_addrs: HashSet<String> = core.iter().map(|(a, _)| a.clone()).collect();
    let mut prepump: Vec<(String, u64, f64)> = funders
        .into_iter()
        .filter(|(addr, e)| !core_addrs.contains(addr) && !e.0.is_empty())
        .map(|(addr, e)| {
            let funded_core = e.0.iter().filter(|h| core_addrs.contains(*h)).count() as u64;
            (addr, funded_core * 10 + e.0.len() as u64, e.1)
        })
        .collect();
    prepump.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.0.cmp(&b.0))
    });
    prepump.truncate(MAX_PREPUMP);

    (Roles { core, prepump }, share, cohort)
}

/// Build the picture of one capital group.
///
/// `capital` — `dataset/capital-groups.json`, `whale` — `dataset/whale-clusters.json`,
/// `discovered` — `dataset/discovered-groups.json`, `conductors` — `dataset/conductors.json`,
/// `labels` — `dataset/labels-cache.json`, `fingerprints` — `dataset/asset-forensics.json`,
/// `asset_scan` — `dataset/wallet-asset-scan.json`.
///
/// `wanted_group` — id of the group to draw; the largest capital group when empty.
pub fn capital_network(
    capital: &Value,
    whale: &Value,
    discovered: &Value,
    conductors: &Value,
    labels: &Value,
    fingerprints: &Value,
    asset_scan: &Value,
    net: &Net,
    wanted_group: Option<&str>,
    membership: &Value,
) -> Value {
    let _ = (whale, discovered, conductors);

    let membership = membership_index(membership);
    let mut groups = parse_capital_groups(capital, &membership);
    if groups.is_empty() {
        groups = parse_whale_clusters(whale);
    }
    if groups.is_empty() {
        groups = parse_discovered_groups(discovered);
    }
    if groups.is_empty() {
        return empty_payload();
    }

    // The group to draw: the requested one, or — by default — the capital
    // group with the largest capital. The screen shows one group at a time,
    // so the biggest one is the natural default.
    let main_idx = wanted_group
        .and_then(|id| groups.iter().position(|g| g.id == id))
        .unwrap_or_else(|| {
            let by_capital = |a: &Grp, b: &Grp| {
                a.total_usd
                    .partial_cmp(&b.total_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.member_count.cmp(&b.member_count))
            };
            groups
                .iter()
                .enumerate()
                .filter(|(_, g)| g.kind == "capital_group" && g.total_usd > 0.0)
                .max_by(|(_, a), (_, b)| by_capital(a, b))
                .or_else(|| {
                    groups
                        .iter()
                        .enumerate()
                        .filter(|(_, g)| g.total_usd > 0.0)
                        .max_by(|(_, a), (_, b)| by_capital(a, b))
                })
                .map(|(i, _)| i)
                .unwrap_or(0)
        });

    let group = groups[main_idx].clone();

    // The core of the group — the BFS roots (hop 0), if the group has them.
    let core_pool: HashSet<String> = group
        .members
        .iter()
        .filter(|m| group.hops.get(*m) == Some(&0))
        .cloned()
        .collect();

    // All the wallets of the group; the drawn ones are capped, but the core
    // and the pre-pump wallets are always in the picture.
    let mut all_members: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for m in &group.members {
        if seen.insert(m.clone()) {
            all_members.push(m.clone());
        }
    }

    // Roles inside the group: the core (the biggest holders of the group's
    // assets), the cohort that holds the same tokens with them and the
    // pre-pump wallets that funded the core.
    let (mut roles, shares, cohorts) =
        read_roles(asset_scan, &all_members_set(&all_members), &core_pool);
    if roles.core.is_empty() {
        // No asset scan for this group — fall back to the most active wallets.
        let mut active: Vec<(String, usize)> = all_members
            .iter()
            .map(|m| (m.clone(), internal_links(net, m, &all_members_set(&all_members))))
            .filter(|(_, d)| *d > 0)
            .collect();
        if active.is_empty() {
            // Not a single transfer inside the group — the most connected
            // wallets of the whole graph become the core.
            active = all_members
                .iter()
                .map(|m| (m.clone(), net.degree(m)))
                .collect();
        }
        active.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        active.truncate(MAX_CORE);
        roles.core = active.into_iter().map(|(a, _)| (a, 0.0)).collect();
    }

    let core_set: HashSet<String> = roles.core.iter().map(|(a, _)| a.clone()).collect();
    let prepump_set: HashSet<String> = roles.prepump.iter().map(|(a, _, _)| a.clone()).collect();

    // The cohort of the core: the wallets that hold the same tokens as the
    // core wallet. They are the dense bundle of red edges around the core.
    let mut cohort_wallets: Vec<String> = Vec::new();
    for (core_addr, _) in &roles.core {
        if let Some(list) = cohorts.get(core_addr) {
            for (a, _) in list {
                if !core_set.contains(a)
                    && !prepump_set.contains(a)
                    && !cohort_wallets.contains(a)
                {
                    cohort_wallets.push(a.clone());
                }
            }
        }
    }
    let cohort_set: HashSet<String> = cohort_wallets.iter().cloned().collect();

    // The hub: the wallet the whole cluster hangs on — the most links inside
    // the group, ties broken by the overall degree.
    let member_set = all_members_set(&all_members);
    let mut hub_candidates: Vec<&String> = all_members.iter().collect();
    hub_candidates.sort_by(|a, b| {
        let ka = (internal_links(net, a, &member_set), net.degree(a));
        let kb = (internal_links(net, b, &member_set), net.degree(b));
        kb.cmp(&ka).then_with(|| a.cmp(b))
    });
    let hub = hub_candidates
        .first()
        .map(|m| (*m).clone())
        .unwrap_or_else(|| all_members[0].clone());

    // The wallets that are drawn: the hub, the core, the pre-pump wallets, the
    // cohort around the core and the rest of the group, up to the cap.
    let mut members: Vec<String> = Vec::new();
    let mut pushed: HashSet<String> = HashSet::new();
    for a in core_set.iter() {
        if pushed.insert(a.clone()) {
            members.push(a.clone());
        }
    }
    for (a, _, _) in &roles.prepump {
        if pushed.insert(a.clone()) {
            members.push(a.clone());
        }
    }
    for a in &cohort_wallets {
        if pushed.insert(a.clone()) {
            members.push(a.clone());
        }
    }
    for m in &all_members {
        if pushed.insert(m.clone()) {
            members.push(m.clone());
        }
        if members.len() >= MAIN_MEMBERS {
            break;
        }
    }
    members.truncate(MAIN_MEMBERS);

    let middle: Vec<String> = members
        .iter()
        .filter(|m| **m != hub && !core_set.contains(*m) && !prepump_set.contains(*m))
        .cloned()
        .collect();

    // --- nodes -------------------------------------------------------------
    let mut nodes: Vec<Value> = Vec::new();
    let mut links: Vec<Value> = Vec::new();
    let mut id_of: HashMap<String, String> = HashMap::new();

    let (hub_tokens, hub_balance) = holdings_of(fingerprints, &hub);
    let hub_id = format!("hub_{}", group.id);
    nodes.push(json!({
        "id": hub_id,
        "address": hub,
        "label": label_of(labels, &hub),
        "type": "hub",
        "role": "hub",
        "color": C_HUB,
        "stroke": C_HUB_STROKE,
        "radius": 8.0,
        "cluster": group.id,
        "position": {"x": MAIN_ANCHOR.0, "y": MAIN_ANCHOR.1},
        "tokens": hub_tokens,
        "balanceUsd": hub_balance,
        "degree": net.degree(&hub),
        "internalDegree": internal_links(net, &hub, &member_set),
        "totalUsd": group.total_usd,
        "assets": group.assets,
    }));
    id_of.insert(hub.clone(), hub_id.clone());

    // The ring of the ordinary wallets («проміжні») — навколо хаба.
    let outer_middle: Vec<String> = middle
        .iter()
        .filter(|m| !cohort_set.contains(*m))
        .cloned()
        .collect();

    // The ring is spread evenly along the arc of the ellipse. With thousands
    // of spokes the fan turns into a solid wedge — the lines have to be much
    // fainter there.
    let spoke_opacity = match outer_middle.len() {
        n if n > 2000 => 0.1,
        n if n > 800 => 0.22,
        _ => 0.55,
    };
    let spoke_width = if outer_middle.len() > 800 { 0.8 } else { 1.2 };
    let ring_angles = ellipse_angles(RING_RX, RING_RY, outer_middle.len());
    for (i, addr) in outer_middle.iter().enumerate() {
        let id = format!("sat_{}_{}", group.id, i);
        let a = ring_angles[i];
        let (px, py) = (
            MAIN_ANCHOR.0 + RING_RX * a.cos(),
            MAIN_ANCHOR.1 + RING_RY * a.sin(),
        );
        let (tokens, balance) = holdings_of(fingerprints, addr);
        nodes.push(json!({
            "id": id,
            "address": addr,
            "label": label_of(labels, addr),
            "type": "satellite",
            "role": "middle",
            "relation": "group",
            "color": C_SAT,
            "stroke": C_SAT_STROKE,
            "radius": 4.0,
            "cluster": group.id,
            "position": {"x": px, "y": py},
            "tokens": tokens,
            "balanceUsd": balance,
            "degree": net.degree(addr),
            "internalDegree": internal_links(net, addr, &member_set),
            "txWithHub": net.edge(&hub, addr).map_or(0, |(n, _)| n),
        }));
        id_of.insert(addr.clone(), id.clone());
        // The spoke: hub -> wallet.
        links.push(json!({
            "source": hub_id,
            "target": id,
            "type": "standard",
            "weight": 1,
            "color": C_SPOKE,
            "width": spoke_width,
            "opacity": spoke_opacity,
            "tx": net.edge(&hub, addr).map_or(0, |(n, _)| n),
        }));
    }

    // --- the second flower: the cohort of the core ------------------------
    // У референсі під червоним ядромом стоїть окрема квітка: її хаб — найбільш
    // зв'язаний гаманець когорти, а самі когорти мають сині промені від нього.
    // Вага 7 у ребрі = «ближче» (за формулою схеми 60 - weight*5).
    let cohort_hub: Option<String> = cohort_wallets
        .iter()
        .max_by(|a, b| net.degree(a).cmp(&net.degree(b)).then_with(|| b.cmp(a)))
        .cloned();
    let cohort_satellites: Vec<String> = cohort_wallets
        .iter()
        .filter(|a| Some(*a) != cohort_hub.as_ref())
        .cloned()
        .collect();

    let mut cohort_hub_id = String::new();
    if let Some(ch) = cohort_hub.as_ref() {
        let id = format!("chub_{}", group.id);
        let (cx, cy) = core_center();
        let (tokens, balance) = holdings_of(fingerprints, ch);
        nodes.push(json!({
            "id": id,
            "address": ch,
            "label": label_of(labels, ch),
            "type": "hub",
            "role": "cohort-hub",
            "color": C_HUB,
            "stroke": C_HUB_STROKE,
            "radius": 7.0,
            "cluster": group.id,
            "position": {"x": cx, "y": cy + COHORT_DROP},
            "tokens": tokens,
            "balanceUsd": balance,
            "degree": net.degree(ch),
            "relation": "asset",
        }));
        id_of.insert(ch.clone(), id.clone());
        cohort_hub_id = id.clone();
    }

    for (i, addr) in cohort_satellites.iter().enumerate() {
        let id = format!("csat_{}_{}", group.id, i);
        let (cx, cy) = core_center();
        let (px, py) = seed_ring(i, cohort_satellites.len(), cx, cy + COHORT_DROP, COHORT_RX, COHORT_RY);
        let (tokens, balance) = holdings_of(fingerprints, addr);
        nodes.push(json!({
            "id": id,
            "address": addr,
            "label": label_of(labels, addr),
            "type": "satellite",
            "role": "middle",
            "relation": "asset",
            "color": C_SAT,
            "stroke": C_SAT_STROKE,
            "radius": 4.0,
            "cluster": group.id,
            "position": {"x": px, "y": py},
            "tokens": tokens,
            "balanceUsd": balance,
            "degree": net.degree(addr),
        }));
        id_of.insert(addr.clone(), id.clone());
        // Синій промінь від хаба когорти — вага 7, тож під-квітка щільніша.
        if !cohort_hub_id.is_empty() {
            links.push(json!({
                "source": cohort_hub_id,
                "target": id,
                "type": "standard",
                "weight": 6,
                "color": C_SPOKE,
                "width": 1.0,
                "opacity": 0.5,
                "tx": net.edge(cohort_hub.as_deref().unwrap_or_default(), addr)
                    .map_or(0, |(n, _)| n),
            }));
        }
    }

    // The core («ядро») — the wallets that hold the group's assets.
    for (i, (addr, _share)) in roles.core.iter().enumerate() {
        let share = shares.get(addr).copied().unwrap_or(0.0);
        let id = format!("core_{}_{}", group.id, i);
        let a = std::f64::consts::FRAC_PI_2 + i as f64 * 2.1;
        let (ccx, ccy) = core_center();
        let (px, py) = (
            ccx + CORE_R * a.cos(),
            ccy + CORE_R * a.sin(),
        );
        let (tokens, balance) = holdings_of(fingerprints, addr);
        nodes.push(json!({
            "id": id,
            "address": addr,
            "label": label_of(labels, addr),
            "type": "special-red",
            "role": "core",
            "color": C_CORE,
            "stroke": C_CORE_STROKE,
            "radius": 9.0,
            "cluster": group.id,
            "position": {"x": px, "y": py},
            "tokens": tokens,
            "balanceUsd": balance,
            "degree": net.degree(addr),
            "internalDegree": internal_links(net, addr, &member_set),
            "share": share,
        }));
        id_of.insert(addr.clone(), id.clone());

        // The core hangs on the heart of the flower — the hub (вага 8 — щільно).
        links.push(json!({
            "source": hub_id,
            "target": id,
            "type": "highlight",
            "weight": 8,
            "color": C_CORE,
            "width": 1.5,
            "opacity": 0.8,
            "tx": net.edge(&hub, addr).map_or(0, |(n, _)| n),
        }));

        // Red edges: to the cohort — the wallets that hold the same tokens and
        // move together with the core. In the reference the red fan goes down
        // from the red node to the second flower.
        let cohort: Vec<(String, f64)> = cohorts.get(addr).cloned().unwrap_or_default();

        let mut targets: Vec<(String, u64, f64)> = if cohort.is_empty() {
            net.neighbours(addr)
                .iter()
                .filter(|(n, _, _)| member_set.contains(n))
                .cloned()
                .collect()
        } else {
            cohort
                .into_iter()
                .map(|(n, _share)| {
                    let (tx, usd) = net.edge(addr, &n).unwrap_or((0, 0.0));
                    (n, tx, usd)
                })
                .collect()
        };
        targets.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        targets.truncate(MAX_HIGHLIGHT_LINKS);
        for (k, (n, tx, usd)) in targets.iter().enumerate() {
            if let Some(tid) = id_of.get(n) {
                let op = 0.8 - (k as f64 * 0.2 / MAX_HIGHLIGHT_LINKS as f64).min(0.2);
                links.push(json!({
                    "source": id,
                    "target": tid,
                    "type": "highlight",
                    "weight": 1,
                    "color": C_CORE,
                    "width": 1.5,
                    "opacity": op.max(0.5),
                    "tx": tx,
                    "usd": usd,
                }));
            }
        }
    }

    // The pre-pump wallets («передпампи») — the funders that came first.
    for (i, (addr, _n, _usd)) in roles.prepump.iter().enumerate() {
        let id = format!("prepump_{}_{}", group.id, i);
        let (px, py) = prepump_slot(i);
        let (tokens, balance) = holdings_of(fingerprints, addr);
        nodes.push(json!({
            "id": id,
            "address": addr,
            "label": label_of(labels, addr),
            "type": "special-green",
            "role": "prepump",
            "color": C_PREPUMP,
            "stroke": C_PREPUMP_STROKE,
            "radius": 6.0,
            "cluster": group.id,
            "position": {"x": px, "y": py},
            "tokens": tokens,
            "balanceUsd": balance,
            "degree": net.degree(addr),
        }));
        id_of.insert(addr.clone(), id.clone());
        // The pre-pump wallet stands on the ring of the group (green_1 of the
        // scheme hangs on the hub) and is tied to the core with a green edge.
        links.push(json!({
            "source": hub_id,
            "target": id,
            "type": "standard",
            "weight": 1,
            "color": C_SPOKE,
            "width": 1.0,
            "opacity": 0.4,
            "tx": net.edge(&hub, addr).map_or(0, |(n, _)| n),
        }));
        // Green edge: the funder -> the core wallet it funded.
        for (core_addr, _) in &roles.core {
            let target = id_of.get(core_addr).cloned();
            if let Some(tid) = target {
                let (tx, usd) = net.edge(addr, core_addr).unwrap_or((0, 0.0));
                links.push(json!({
                    "source": id,
                    "target": tid,
                    "type": "special-green",
                    "weight": 2,
                    "color": C_PREPUMP,
                    "width": 1.2,
                    "opacity": 0.6,
                    "tx": tx,
                    "usd": usd,
                }));
            }
        }
    }

    // The list of capital groups — the screen shows one at a time.
    let group_list: Vec<Value> = groups
        .iter()
        .filter(|g| g.kind == "capital_group" && g.total_usd > 0.0)
        .map(|g| {
            json!({
                "id": g.id,
                "label": g.label,
                "totalUsd": g.total_usd,
                "size": g.member_count,
                "core": g.has_core(),
            })
        })
        .collect();

    let stats = json!({
        "group": group.id,
        "groupLabel": group.label,
        "members": members.len(),
        "core": roles.core.len(),
        "prepump": roles.prepump.len(),
        "middle": middle.len(),
        "nodes": nodes.len(),
        "links": links.len(),
        "totalUsd": group.total_usd,
        "graphNodes": net.adj.len(),
    });

    json!({
        "generated": now_ms(),
        "title": "Capital Group Connection Graph",
        "metadata": {
            "layout": "force-directed",
            "theme": "dark",
            "group": group.id,
            "groupLabel": group.label,
            // Поля метаданих — точно як у схемі (див. «граф зв'язків»).
            "address": hub,
            "tokens": tokens_text(&hub_tokens),
            "balance": balance_text(hub_balance),
            "avgBuy": 0,
            "avgSell": Value::Null,
        },
        "nodes": nodes,
        "links": links,
        "groups": group_list,
        "styles": styles(),
        "stats": stats,
        "legend": legend(),
    })
}

fn empty_payload() -> Value {
    json!({
        "generated": now_ms(),
        "title": "Capital Group Connection Graph",
        "metadata": {"layout": "force-directed", "theme": "dark"},
        "nodes": [], "links": [],
        "styles": styles(),
        "stats": {"nodes": 0, "links": 0},
        "legend": legend(),
    })
}

/// Canvas and node styles — exactly the reference styles.
fn styles() -> Value {
    json!({
        "background": {
            "color": "#1A1D21",
            "grid_pattern": {"type": "dots", "color": "#2A2E35", "size": 1, "spacing": 20}
        },
        "node": {
            "shape": "circle",
            "shadow": "0 0 6px rgba(0,0,0,0.4)",
            "z_index": "hub > special > satellite"
        },
        "link": {"shape": "line", "mix_blend_mode": "screen"}
    })
}

fn legend() -> Value {
    json!([
        {"color": C_HUB, "key": "hub", "label": "Хаб групи"},
        {"color": C_HUB, "key": "cohort-hub", "label": "Хаб когорти ядра"},
        {"color": C_SAT, "key": "middle", "label": "Проміжні гаманці групи"},
        {"color": C_CORE, "key": "core", "label": "Ядро групи — пампові гаманці"},
        {"color": C_PREPUMP, "key": "prepump", "label": "Передпампові гаманці"},
    ])
}

/// «Tokens» of the metadata panel — a short list of the tickers.
fn tokens_text(tokens: &[String]) -> String {
    if tokens.is_empty() {
        return "—".to_string();
    }
    let head: Vec<&str> = tokens.iter().take(4).map(|t| t.as_str()).collect();
    let mut text = head.join(", ");
    if tokens.len() > 4 {
        text.push_str(&format!(" +{}", tokens.len() - 4));
    }
    text
}

/// «Balance» of the metadata panel — in dollars, as in the scheme.
fn balance_text(usd: f64) -> String {
    if usd <= 0.0 {
        return "$0".to_string();
    }
    if usd >= 1e9 {
        format!("{:.1}B USD", usd / 1e9)
    } else if usd >= 1e6 {
        format!("{:.1}M USD", usd / 1e6)
    } else if usd >= 1e3 {
        format!("{:.1}K USD", usd / 1e3)
    } else {
        format!("{:.0} USD", usd)
    }
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
      {"id":"CAP1","name":"Capital one","size":3,"totalUsd":1000.0,"commonAssets":["EV"],
       "members":[{"address":"0xaaa"},{"address":"0xbbb"},{"address":"0xccc"}]},
      {"id":"CAP2","name":"Capital two","size":2,"totalUsd":500.0,"members":[{"address":"0xddd"},{"address":"0xeee"}]}
    ]}"#;

    const SCAN: &str = r#"{"assets":[
      {"symbol":"EV","groups":[{"members":[{"addr":"0xaaa","share":40.0},{"addr":"0xbbb","share":12.0}],
        "commonFunders":[{"funder":"0xpre","avgUsd":5000,"holders":2}]}]}
    ]}"#;

    fn out(cap: &str, scan: &str, net: &Net) -> Value {
        let c: Value = serde_json::from_str(cap).unwrap();
        let sc: Value = serde_json::from_str(scan).unwrap();
        capital_network(
            &c,
            &json!({}),
            &json!({}),
            &json!({}),
            &json!({}),
            &json!({}),
            &sc,
            net,
            None,
            &json!({"wallets": {}}),
        )
    }

    #[test]
    fn payload_follows_the_reference_shape() {
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xaaa", "0xccc")]);
        let o = out(CAP, SCAN, &net);
        assert!(o["nodes"].is_array() && o["links"].is_array());
        assert_eq!(o["metadata"]["layout"], "force-directed");
        assert_eq!(o["styles"]["background"]["color"], "#1A1D21");
        assert_eq!(o["styles"]["link"]["mix_blend_mode"], "screen");
        for n in o["nodes"].as_array().unwrap() {
            assert!(n["id"].is_string() && n["type"].is_string() && n["role"].is_string());
            assert!(n["color"].is_string() && n["radius"].is_number());
            assert!(n["position"]["x"].is_number() && n["position"]["y"].is_number());
        }
    }

    #[test]
    fn one_group_only_and_no_links_between_groups() {
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xbbb", "0xddd")]);
        let o = out(CAP, SCAN, &net);
        assert_eq!(o["metadata"]["group"], "CAP1");
        // no node of another group is drawn
        let groups: HashSet<String> = o["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["cluster"].as_str().map(String::from))
            .collect();
        assert_eq!(groups.len(), 1, "на екрані одна група: {groups:?}");
        // and no link points outside the group
        assert!(o["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["type"] == "standard" || l["type"] == "highlight" || l["type"] == "special-green"));
    }

    #[test]
    fn the_hub_is_the_most_linked_wallet() {
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xaaa", "0xccc"), ("0xbbb", "0xccc")]);
        let o = out(CAP, SCAN, &net);
        let hub = o["nodes"].as_array().unwrap().iter().find(|n| n["type"] == "hub").unwrap();
        assert_eq!(hub["address"], "0xaaa");
        assert_eq!(hub["color"], C_HUB);
    }

    #[test]
    fn roles_come_from_the_asset_scan() {
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xaaa", "0xccc")]);
        let o = out(CAP, SCAN, &net);
        let nodes = o["nodes"].as_array().unwrap();
        // 0xaaa holds 40% of EV -> it is the core
        let core = nodes.iter().find(|n| n["role"] == "core").expect("є ядро");
        assert_eq!(core["address"], "0xaaa");
        assert_eq!(core["color"], C_CORE);
        // 0xpre funded the holders -> pre-pump
        let pre = nodes.iter().find(|n| n["role"] == "prepump").expect("є передпампи");
        assert_eq!(pre["address"], "0xpre");
        assert_eq!(pre["color"], C_PREPUMP);
        // the green edge points at the core wallet
        assert!(o["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["type"] == "special-green" && l["target"] == core["id"]));
    }

    #[test]
    fn spokes_go_from_the_hub_to_every_ordinary_wallet() {
        let net = net_from(&[("0xaaa", "0xbbb"), ("0xaaa", "0xccc")]);
        let o = out(CAP, SCAN, &net);
        let spokes = o["links"].as_array().unwrap().iter().filter(|l| {
            l["source"] == "hub_CAP1"
                && l["type"] == "standard"
                && l["opacity"] == 0.55
                && l["target"].as_str().unwrap_or("").starts_with("sat_")
        }).count();
        assert_eq!(spokes, 1, "промінь до кожного проміжного гаманця");
    }
}
