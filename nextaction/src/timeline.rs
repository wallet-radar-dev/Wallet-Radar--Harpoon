//! Часовий аналіз: коли активувався кожен деск. Для іменованих груп (359-когорта,
//! мега-кити, airdrop-фарм, tokenized-stocks, meta-DePIN/AI, meta-Arbitrum) рахує
//! помісячну ERC-20 активність (dir=i receipts, ts[0:7]) з transfers.csv + медіану
//! першого місяця. Вихід: dataset/backfill/timeline.json + звіт.

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn load(p: &str) -> Option<Value> {
    File::open(p).ok().and_then(|f| serde_json::from_reader(BufReader::new(f)).ok())
}
fn lc(v: &Value) -> Option<String> { v.as_str().map(|s| s.to_lowercase()) }
fn arr(v: &Value) -> HashSet<String> {
    v.as_array().map(|a| a.iter().filter_map(lc).collect()).unwrap_or_default()
}
fn community(v: &Value, k: i64) -> HashSet<String> {
    v["communities"].as_array().map(|cs| cs.iter()
        .filter(|c| c["k"].as_i64() == Some(k))
        .flat_map(|c| c["members"].as_array().map(|m| m.iter().filter_map(lc).collect::<Vec<_>>()).unwrap_or_default())
        .collect()).unwrap_or_default()
}

struct Grp { label: String, mem: HashSet<String>, months: HashMap<String, u64>, first: HashMap<String, String> }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let d = format!("{root}/dataset/backfill");
    let gu = load(&format!("{d}/groups-universe.json")).expect("groups-universe.json");
    let co = load(&format!("{d}/cohort-analysis.json")).expect("cohort-analysis.json");
    let ms = load(&format!("{d}/meta-subnets.json")).unwrap_or(json!({}));

    let mega: HashSet<String> = gu["archetypes"].as_array().unwrap().iter()
        .max_by(|a, b| a["avg_eth"].as_f64().unwrap().partial_cmp(&b["avg_eth"].as_f64().unwrap()).unwrap())
        .and_then(|c| c["members"].as_array()).map(|a| a.iter().filter_map(lc).collect()).unwrap_or_default();

    let mut groups: Vec<Grp> = vec![
        ("359-когорта (ETH-band)", arr(&co["band_members"])),
        ("мега-кити (C6)", mega),
        ("airdrop-фарм", { let mut s = community(&gu, 0); s.extend(community(&gu, 1)); s }),
        ("tokenized-stocks", community(&gu, 2)),
        ("meta: DePIN/AI", community(&ms, 0)),
        ("meta: Arbitrum-DeFi", community(&ms, 1)),
    ].into_iter().map(|(l, m)| Grp { label: l.to_string(), mem: m, months: HashMap::new(), first: HashMap::new() }).collect();

    let f = BufReader::new(File::open(format!("{d}/transfers.csv")).expect("transfers.csv"));
    for line in f.lines().map_while(Result::ok) {
        let mut l = line.splitn(6, ',');
        let w = l.next().unwrap_or("");
        let _chain = l.next();
        let dir = l.next().unwrap_or("?");
        let ts = l.next().unwrap_or("");
        if dir != "i" || ts.len() < 7 { continue; }
        let wl = w.to_lowercase();
        let ym = ts[0..7].to_string();
        for g in groups.iter_mut() {
            if g.mem.contains(&wl) {
                *g.months.entry(ym.clone()).or_insert(0) += 1;
                let e = g.first.entry(wl.clone()).or_insert_with(|| ym.clone());
                if ym < *e { *e = ym.clone(); }
            }
        }
    }

    let mut out = Vec::new();
    println!("=== ЧАСОВИЙ АНАЛІЗ: помісячна ERC-20 активність по десках ===");
    for g in &groups {
        if g.mem.is_empty() { continue; }
        let total: u64 = g.months.values().sum();
        let mut ordered: BTreeMap<&String, &u64> = g.months.iter().collect();
        // top months by activity
        let mut top: Vec<(&String, &u64)> = g.months.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        // median first-active month
        let mut firsts: Vec<&String> = g.first.values().collect();
        firsts.sort();
        let med = firsts.get(firsts.len() / 2).map(|s| s.as_str()).unwrap_or("-");
        println!("\n[{}] членів={} з активністю={} | всього receipts={} | медіана 1-го міс={}",
            g.label, g.mem.len(), g.first.len(), total, med);
        println!("  топ-місяці: {:?}", top.iter().take(6).map(|(m, c)| format!("{m}:{c}")).collect::<Vec<_>>());
        out.push(json!({
            "group": g.label, "members": g.mem.len(), "active": g.first.len(),
            "total_receipts": total, "median_first_month": med,
            "top_months": top.iter().take(8).map(|(m, c)| json!([m, c])).collect::<Vec<_>>(),
            "monthly": ordered.iter().map(|(m, c)| json!([m, c])).collect::<Vec<_>>(),
        }));
        ordered.clear();
    }
    serde_json::to_writer_pretty(File::create(format!("{d}/timeline.json")).unwrap(),
        &json!({"groups": out})).unwrap();
    eprintln!("\nзбережено: timeline.json");
}
