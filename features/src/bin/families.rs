//! Whale-family grouping: union-find over the funding/interaction graph
//! (graph-db.json), EXCLUDING hubs (high-degree = CEX / router / bridge) so a
//! whale -> pre-pump -> pump cascade collapses into ONE family instead of the
//! whole market collapsing into one blob through Binance.
//!
//! Each connected component = one operation's wallet network (can be thousands).
//! Group-level aggregation (volume, later emission share) is the accurate unit —
//! per-wallet undercounts because one entity spreads across many addresses.
//!
//! In:  dataset/graph-db.json  (nodes {addr:{d,v}}, edges [{k:"a|b",n}])
//! Out: dataset/families.json  {generated, hubDegree, families:[{size,members,volumeUsd,edges}]}

use std::collections::HashMap;
use std::io::Write;

use serde_json::Value;

fn find(p: &mut HashMap<u32, u32>, mut x: u32) -> u32 {
    while p[&x] != x {
        let g = p[&p[&x]];
        p.insert(x, g);
        x = p[&x];
    }
    x
}
fn union(p: &mut HashMap<u32, u32>, a: u32, b: u32) {
    let (ra, rb) = (find(p, a), find(p, b));
    if ra != rb {
        p.insert(ra, rb);
    }
}

fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let d: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{root}/dataset/graph-db.json")).expect("read graph-db"),
    )
    .expect("parse graph-db");

    // index addresses -> u32 id; keep degree + volume
    let nodes = d["nodes"].as_object().expect("nodes");
    let mut id: HashMap<String, u32> = HashMap::with_capacity(nodes.len());
    let mut deg: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut vol: Vec<f64> = Vec::with_capacity(nodes.len());
    let mut addr: Vec<String> = Vec::with_capacity(nodes.len());
    for (a, v) in nodes {
        let i = id.len() as u32;
        id.insert(a.clone(), i);
        deg.push(v["d"].as_u64().unwrap_or(0) as u32);
        vol.push(v["v"].as_f64().unwrap_or(0.0));
        addr.push(a.clone());
    }

    // Two knobs (env) to stop the whole market collapsing into one blob:
    //   HUB_PCT  — exclude the top (1-HUB_PCT) by degree as hubs (CEX/routers/bridges)
    //   MIN_EDGE — only union on edges seen >= this many times (drop one-off noise)
    let hub_pct: f64 = std::env::var("HUB_PCT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.95);
    let min_edge: u64 = std::env::var("MIN_EDGE").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
    let mut sorted = deg.clone();
    sorted.sort_unstable();
    let p = ((sorted.len() as f64) * hub_pct) as usize;
    let hub_deg = sorted.get(p).copied().unwrap_or(u32::MAX).max(20);

    // union-find over non-hub edges
    let mut parent: HashMap<u32, u32> = (0..id.len() as u32).map(|i| (i, i)).collect();
    let mut edge_ct: HashMap<u32, u32> = HashMap::new(); // per-root edge count
    let empty = vec![];
    let mut kept = 0u64;
    for e in d["edges"].as_array().unwrap_or(&empty) {
        let Some(k) = e["k"].as_str() else { continue };
        if e["n"].as_u64().unwrap_or(1) < min_edge { continue; }
        let Some((a, b)) = k.split_once('|') else { continue };
        let (Some(&ia), Some(&ib)) = (id.get(a), id.get(b)) else { continue };
        if deg[ia as usize] >= hub_deg || deg[ib as usize] >= hub_deg {
            continue; // route through hubs is not kinship
        }
        union(&mut parent, ia, ib);
        kept += 1;
    }
    // count edges per component
    for e in d["edges"].as_array().unwrap_or(&empty) {
        let Some(k) = e["k"].as_str() else { continue };
        if e["n"].as_u64().unwrap_or(1) < min_edge { continue; }
        let Some((a, b)) = k.split_once('|') else { continue };
        let (Some(&ia), Some(&ib)) = (id.get(a), id.get(b)) else { continue };
        if deg[ia as usize] >= hub_deg || deg[ib as usize] >= hub_deg {
            continue;
        }
        let r = find(&mut parent, ia);
        if find(&mut parent, ib) == r {
            *edge_ct.entry(r).or_default() += 1;
        }
    }

    // assemble components
    let mut comp: HashMap<u32, Vec<u32>> = HashMap::new();
    for i in 0..id.len() as u32 {
        let r = find(&mut parent, i);
        comp.entry(r).or_default().push(i);
    }
    let mut fams: Vec<(usize, f64, u32, Vec<u32>)> = comp
        .into_iter()
        .filter(|(_, m)| m.len() >= 3)
        .map(|(r, m)| {
            let v: f64 = m.iter().map(|&i| vol[i as usize]).sum();
            (m.len(), v, *edge_ct.get(&r).unwrap_or(&0), m)
        })
        .collect();
    // sort by member count desc, then volume desc
    fams.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)));

    // Full member -> family map (family id = top-volume member) for group features.
    {
        let mut mw = std::io::BufWriter::new(
            std::fs::File::create(format!("{root}/dataset/families-map.csv")).unwrap(),
        );
        writeln!(mw, "wallet,family").unwrap();
        for (_s, _v, _e, m) in &fams {
            let mut mm = m.clone();
            mm.sort_by(|&a, &b| vol[b as usize].partial_cmp(&vol[a as usize]).unwrap_or(std::cmp::Ordering::Equal));
            let fam = addr[mm[0] as usize].clone();
            for &i in &mm {
                writeln!(mw, "{},{}", addr[i as usize], fam).unwrap();
            }
        }
        mw.flush().unwrap();
    }

    let total_grouped: usize = fams.iter().map(|f| f.0).sum();
    let out_fams: Vec<Value> = fams
        .iter()
        .take(200)
        .map(|(size, v, edges, m)| {
            // members sorted by volume desc, top 25 shown
            let mut mem: Vec<u32> = m.clone();
            mem.sort_by(|&a, &b| vol[b as usize].partial_cmp(&vol[a as usize]).unwrap_or(std::cmp::Ordering::Equal));
            let members: Vec<Value> = mem.iter().take(25).map(|&i| {
                serde_json::json!({"address": addr[i as usize], "volumeUsd": vol[i as usize].round(), "degree": deg[i as usize]})
            }).collect();
            serde_json::json!({
                "size": size,
                "volumeUsd": (v).round(),
                "edges": edges,
                "root": addr[mem[0] as usize],
                "members": members,
            })
        })
        .collect();

    let out = serde_json::json!({
        "generated": 0,
        "hubDegree": hub_deg,
        "nodes": id.len(),
        "edgesKept": kept,
        "familiesCount": fams.len(),
        "groupedWallets": total_grouped,
        "families": out_fams,
    });
    let p = format!("{root}/dataset/families.json");
    std::fs::write(&p, serde_json::to_string(&out).unwrap()).unwrap();
    let mut w = std::io::stderr();
    let _ = writeln!(
        w,
        "[families] {} nodes, hubDeg={}, {} families (>=3), {} wallets grouped, largest={} -> {}",
        id.len(), hub_deg, fams.len(), total_grouped, fams.first().map(|f| f.0).unwrap_or(0), p
    );
}
