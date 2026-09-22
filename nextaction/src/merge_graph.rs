//! Майстер-граф: зливає desk-network + c6-network [+ airdrop-network] у єдиний деду-
//! плікований граф, анотує кожен вузол приналежністю (359-когорта / C6-мега-кит-seed /
//! airdrop-фарм / tokenized-stocks / OTC-хаб / біржа) + інституційна мітка. Ребра з max-ETH,
//! опц. поріг MIN_EDGE. Вихід: dataset/backfill/master-graph.json. Env: MIN_EDGE(500).

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;

fn load(p: &str) -> Option<Value> {
    File::open(p).ok().and_then(|f| serde_json::from_reader(BufReader::new(f)).ok())
}
fn lc(v: &Value) -> Option<String> { v.as_str().map(|s| s.to_lowercase()) }
fn arr_lc(v: &Value, key: &str) -> HashSet<String> {
    v[key].as_array().map(|a| a.iter().filter_map(lc).collect()).unwrap_or_default()
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let d = format!("{root}/dataset/backfill");
    let min_edge: f64 = std::env::var("MIN_EDGE").ok().and_then(|s| s.parse().ok()).unwrap_or(500.0);

    let desk = load(&format!("{d}/desk-network-graph.json"));
    let c6 = load(&format!("{d}/c6-network-graph.json"));
    let air = load(&format!("{d}/airdrop-network-graph.json"));
    let groups = load(&format!("{d}/groups-universe.json")).expect("groups-universe.json");
    let cohort = load(&format!("{d}/cohort-analysis.json")).expect("cohort-analysis.json");
    let labels = load(&format!("{d}/network-labels.json")).expect("network-labels.json");

    // membership sets
    let set359 = arr_lc(&cohort, "band_members");
    let c6seed: HashSet<String> = groups["archetypes"].as_array().unwrap().iter()
        .max_by(|a, b| a["avg_eth"].as_f64().unwrap().partial_cmp(&b["avg_eth"].as_f64().unwrap()).unwrap())
        .and_then(|c| c["members"].as_array())
        .map(|a| a.iter().filter_map(lc).collect()).unwrap_or_default();
    let mut airdrop = HashSet::new();
    let mut tokstocks = HashSet::new();
    for c in groups["communities"].as_array().unwrap() {
        let k = c["k"].as_i64().unwrap_or(-1);
        let mem: HashSet<String> = c["members"].as_array().map(|a| a.iter().filter_map(lc).collect()).unwrap_or_default();
        if k == 0 || k == 1 { airdrop.extend(mem); }
        else if k == 2 { tokstocks.extend(mem); }
    }
    // labels + otc set
    let mut lab: HashMap<String, String> = HashMap::new();
    for grp in ["hubs_otc", "exchange_sinks"] {
        if let Some(o) = labels[grp].as_object() {
            for (k, v) in o { if let Some(s) = v.as_str() { lab.insert(k.to_lowercase(), s.to_string()); } }
        }
    }
    let otc: HashSet<String> = labels["hubs_otc"].as_object()
        .map(|o| o.keys().map(|k| k.to_lowercase()).collect()).unwrap_or_default();

    // union nodes, edges (max eth), leak (max per exchange)
    let mut idset: HashSet<String> = HashSet::new();
    let mut edges: HashMap<(String, String), f64> = HashMap::new();
    let mut leak: HashMap<String, f64> = HashMap::new();
    let mut srcflag: HashMap<String, u8> = HashMap::new(); // 1=desk 2=c6 4=air
    for (bit, g) in [(1u8, &desk), (2u8, &c6), (4u8, &air)] {
        let Some(v) = g else { continue };
        for n in v["internal_nodes"].as_array().map(|a| a.iter().filter_map(lc).collect::<Vec<_>>()).unwrap_or_default() {
            idset.insert(n.clone()); *srcflag.entry(n).or_insert(0) |= bit;
        }
        if let Some(es) = v["edges"].as_array() {
            for e in es {
                let (Some(f), Some(t), Some(w)) = (lc(&e[0]), lc(&e[1]), e[2].as_f64()) else { continue };
                idset.insert(f.clone()); idset.insert(t.clone());
                let en = edges.entry((f, t)).or_insert(0.0); if w > *en { *en = w; }
            }
        }
        if let Some(ls) = v["exchange_leak"].as_array() {
            for e in ls {
                let (Some(a), Some(w)) = (lc(&e[0]), e[1].as_f64()) else { continue };
                idset.insert(a.clone());
                let en = leak.entry(a).or_insert(0.0); if w > *en { *en = w; }
            }
        }
    }

    // Прунінг: лишаємо лише вузли, що беруть участь у значущих (>=min_edge) ребрах,
    // ПЛЮС мічені (біржі/OTC). Ізольовані (без потоку >=min_edge) відкидаємо — інакше
    // 2600+ вузлів дають нечитабельний/порожній хайрбол.
    let full_n = idset.len();
    let mut keep: HashSet<String> = HashSet::new();
    for ((f, t), w) in &edges {
        if *w >= min_edge { keep.insert(f.clone()); keep.insert(t.clone()); }
    }
    for a in lab.keys() { if idset.contains(a) { keep.insert(a.clone()); } }
    let mut order: Vec<String> = idset.into_iter().filter(|a| keep.contains(a)).collect();
    order.sort();
    let idx: HashMap<&String, usize> = order.iter().enumerate().map(|(i, a)| (a, i)).collect();

    // node annotations
    let nodes: Vec<Value> = order.iter().map(|a| {
        let role = if leak.contains_key(a) || (lab.contains_key(a) && !otc.contains(a)) { "exchange" }
            else if otc.contains(a) { "otc" } else { "eoa" };
        let grp = if tokstocks.contains(a) { "tokstocks" }
            else if airdrop.contains(a) { "airdrop" }
            else if c6seed.contains(a) { "megawhale" }
            else if set359.contains(a) { "cohort359" } else { "" };
        json!({
            "id": a, "src": srcflag.get(a).copied().unwrap_or(0), "role": role, "grp": grp,
            "label": lab.get(a), "leak": leak.get(a).map(|v| v.round()),
        })
    }).collect();

    // edges (filtered, indexed)
    let mut ej: Vec<Value> = Vec::new();
    for ((f, t), w) in &edges {
        if *w < min_edge { continue; }
        let (Some(&si), Some(&di)) = (idx.get(f), idx.get(t)) else { continue };
        let leaked = leak.contains_key(t);
        ej.push(json!([si, di, w.round(), if leaked {1} else {0}]));
    }

    let counts = json!({
        "total_nodes": full_n,
        "graph_nodes": order.len(),
        "in_desk": srcflag.values().filter(|f| *f & 1 != 0).count(),
        "in_c6": srcflag.values().filter(|f| *f & 2 != 0).count(),
        "in_air": srcflag.values().filter(|f| *f & 4 != 0).count(),
        "airdrop": airdrop.len(), "tokstocks": tokstocks.len(), "megawhale_seed": c6seed.len(),
        "exchanges": leak.len(), "otc": otc.len(), "edges_kept": ej.len(),
        "total_leak": leak.values().sum::<f64>().round(),
    });
    eprintln!("[merge] {counts}");
    serde_json::to_writer_pretty(
        File::create(format!("{d}/master-graph.json")).unwrap(),
        &json!({"nodes": nodes, "edges": ej, "counts": counts, "min_edge": min_edge}),
    ).unwrap();
    eprintln!("збережено: master-graph.json");
}
