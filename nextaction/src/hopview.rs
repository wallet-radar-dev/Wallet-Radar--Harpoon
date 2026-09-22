//! hopview — реальна картина групи китів ПО ХОПАХ: хто на якій відстані від ядра, скільки там
//! справжніх грошей і що змінив новий краул нативного ETH (eth-edges2).
//!
//! На кожен хоп: скільки гаманців, сума РЕАЛЬНИХ балансів ETH, скільки з них мають дані в бекфілі,
//! скільки відкрив новий краул, топ-гаманці з мітками. Плюс діагностика мережі: скільки гаманців
//! підтягнулись БЛИЖЧЕ до ядра (нові ребра скоротили відстань) і скільки прийшло вперше —
//! саме це показує, чи мережа насправді тісніша, ніж здавалась.
//!
//! Вхід: group-membership.json (+ .pre-edges2.json знімок для порівняння), eth-balances.json,
//!       eth-edges2.jsonl, transfers.csv (наявність даних), labels-cache.json
//! Вихід: dataset/backfill/hopview.json + друк таблиці. Args: ROOT. Env: GROUP(NET) TOPN(5)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    // GROUP і PREV приймають список через кому: після докраулу компоненти зливаються і група
    // може змінити ім'я (NET+HG7+HG8+HG12 → HG5), тож «до» і «після» треба зіставляти як множини
    let group = std::env::var("GROUP").unwrap_or_else(|_| "NET".into());
    let cur: HashSet<String> = group.split(',').map(|s| s.trim().to_string()).collect();
    let prevg: HashSet<String> = std::env::var("PREV").unwrap_or_else(|_| group.clone()).split(',').map(|s| s.trim().to_string()).collect();
    let topn: usize = std::env::var("TOPN").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let bf = format!("{root}/dataset/backfill");

    let memb: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/group-membership.json")).expect("membership"))).unwrap();
    let prev: Option<Value> = File::open(format!("{bf}/group-membership.pre-edges2.json")).ok()
        .and_then(|f| serde_json::from_reader(BufReader::new(f)).ok());

    // хто в групі та на якому хопі
    let mut hop_of: HashMap<String, i64> = HashMap::new();
    let mut via_of: HashMap<String, String> = HashMap::new();
    for (w, x) in memb["wallets"].as_object().cloned().unwrap_or_default() {
        if !x["group"].as_str().map(|g| cur.contains(g)).unwrap_or(false) { continue; }
        hop_of.insert(w.to_lowercase(), x["hop"].as_i64().unwrap_or(-1));
        via_of.insert(w.to_lowercase(), x["via"].as_str().unwrap_or("?").to_string());
    }
    if hop_of.is_empty() { eprintln!("[hopview] у групі {group} нікого"); return; }

    // попередній стан (до краулу) — для діагностики зрушень
    let (mut prev_hop, mut prev_in): (HashMap<String, i64>, HashSet<String>) = (HashMap::new(), HashSet::new());
    if let Some(p) = &prev {
        for (w, x) in p["wallets"].as_object().cloned().unwrap_or_default() {
            if !x["group"].as_str().map(|g| prevg.contains(g)).unwrap_or(false) { continue; }
            let w = w.to_lowercase();
            prev_hop.insert(w.clone(), x["hop"].as_i64().unwrap_or(-1));
            prev_in.insert(w);
        }
    }

    // реальні баланси
    let bal: HashMap<String, f64> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/eth-balances.json")).expect("eth-balances"))).ok()
        .and_then(|v| v["wallets"].as_object().map(|m| m.iter()
            .filter(|(_, x)| x["code"] != json!(true))
            .filter_map(|(k, x)| x["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect())).unwrap_or_default();

    // кого відкрив новий краул + у кого є дані в бекфілі
    let mut crawled: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/eth-edges2.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) { if let Some(w) = v["w"].as_str() { crawled.insert(w.to_lowercase()); } }
        }
    }
    let mut hasrows: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/transfers.csv")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Some(w) = line.split(',').next() { if w.len() == 42 { hasrows.insert(w.to_lowercase()); } }
        }
    }
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();

    // зведення по хопах
    let mut by_hop: HashMap<i64, Vec<String>> = HashMap::new();
    for (w, h) in &hop_of { by_hop.entry(*h).or_default().push(w.clone()); }
    let mut hops: Vec<i64> = by_hop.keys().copied().collect();
    hops.sort();

    let (mut moved_closer, mut joined_new) = (0usize, 0usize);
    for (w, h) in &hop_of {
        match prev_hop.get(w) {
            Some(ph) => { if *h >= 0 && (*ph < 0 || *h < *ph) { moved_closer += 1; } }
            None => { if !prev_in.contains(w) { joined_new += 1; } }
        }
    }

    let mut rows: Vec<Value> = Vec::new();
    println!("=== ГРУПА {group}: картина по хопах ===");
    println!("{:<8} {:>8} {:>16} {:>10} {:>10} {:>12}", "хоп", "гаманців", "реальний ETH $", "з даними", "докраул", "сер. $/гам");
    for h in &hops {
        let ws = &by_hop[h];
        let usd: f64 = ws.iter().filter_map(|w| bal.get(w)).sum();
        let with_data = ws.iter().filter(|w| hasrows.contains(*w)).count();
        let from_crawl = ws.iter().filter(|w| crawled.contains(*w)).count();
        let label = if *h < 0 { "поза графом".to_string() } else { format!("hop{h}") };
        println!("{:<8} {:>8} {:>16.0} {:>10} {:>10} {:>12.0}", label, ws.len(), usd, with_data, from_crawl,
            if ws.is_empty() { 0.0 } else { usd / ws.len() as f64 });
        let mut top: Vec<(&String, f64)> = ws.iter().map(|w| (w, bal.get(w).copied().unwrap_or(0.0))).collect();
        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        rows.push(json!({"hop": h, "wallets": ws.len(), "usd": usd, "withData": with_data, "fromCrawl": from_crawl,
            "top": top.iter().take(topn).map(|(w, u)| json!({"w": w, "usd": u,
                "label": lbl.get(*w).and_then(|x| x["name"].as_str()).unwrap_or(""),
                "via": via_of.get(*w).cloned().unwrap_or_default()})).collect::<Vec<Value>>()}));
    }
    let total_usd: f64 = hop_of.keys().filter_map(|w| bal.get(w)).sum();
    let core_usd: f64 = hop_of.iter().filter(|(_, h)| **h >= 0 && **h <= 2).filter_map(|(w, _)| bal.get(w)).sum();
    println!("\nразом {} гаманців · ${:.0} · ядро hop0-2: ${:.0} ({:.1}%)",
        hop_of.len(), total_usd, core_usd, if total_usd > 0.0 { core_usd / total_usd * 100.0 } else { 0.0 });
    if prev.is_some() {
        println!("зрушення після краулу нативного ETH: підтягнулись БЛИЖЧЕ до ядра {moved_closer} · приєднались уперше {joined_new} · було в групі {}", prev_in.len());
    }
    println!("\n--- топ-гаманці по хопах ---");
    for r in &rows {
        let h = r["hop"].as_i64().unwrap_or(-1);
        for t in r["top"].as_array().cloned().unwrap_or_default().iter().take(3) {
            let u = t["usd"].as_f64().unwrap_or(0.0);
            if u <= 0.0 { continue; }
            println!("  hop{h:<3} ${:>14.0}  {}  [{}] {}", u, t["w"].as_str().unwrap_or(""), t["via"].as_str().unwrap_or(""), t["label"].as_str().unwrap_or(""));
        }
    }

    std::fs::write(format!("{bf}/hopview.json"), serde_json::to_string_pretty(&json!({
        "group": group, "wallets": hop_of.len(), "totalUsd": total_usd, "coreUsd": core_usd,
        "movedCloser": moved_closer, "joinedNew": joined_new, "prevSize": prev_in.len(), "hops": rows})).unwrap()).ok();
    eprintln!("[hopview] збережено hopview.json");
}
