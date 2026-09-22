//! Аналіз ETH-ротації-когорти + BTC-exit->ETH-entry темпоральний підпис (Rust-заміна
//! разового python-аналізу). Читає eth-native-net.json (net ETH з 09.2025), рахує
//! китову смугу 100-20k ETH (біржовий масштаб >20k відсічено), DePIN-нахил зі
//! transfers.csv, і BTC-роздачу (dir=o) по роках/місяцях зі transfers-btc.csv.
//! Вихід: dataset/backfill/{eth-rotation-cohort.json, btc-eth-signature.json} + звіт.

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

const DEPIN: &[&str] = &["ankr","rlc","grt","pha","audio","woo","link","render","rndr",
    "fil","hnt","iotx","theta","akt","dimo","nosana","io","gomining","peaq"];

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let d = format!("{root}/dataset/backfill");
    let dep: HashSet<&str> = DEPIN.iter().copied().collect();

    // native-ETH net, тільки позитивні
    let raw: HashMap<String, f64> = serde_json::from_reader(
        BufReader::new(File::open(format!("{d}/eth-native-net.json")).expect("eth-native-net.json"))
    ).expect("parse json");
    let eth: HashMap<String, f64> = raw.into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .filter(|(_, v)| *v > 0.0).collect();
    let band: HashMap<String, f64> = eth.iter()
        .filter(|(_, v)| **v >= 100.0 && **v <= 20000.0)
        .map(|(k, v)| (k.clone(), *v)).collect();
    let bset: HashSet<&String> = band.keys().collect();

    // DePIN-нахил + топ-купівлі смуги (dir=i у transfers.csv)
    let mut whales_hit: HashMap<String, HashSet<String>> = HashMap::new();
    let mut recv: HashMap<String, u64> = HashMap::new();
    let f = BufReader::new(File::open(format!("{d}/transfers.csv")).expect("transfers.csv"));
    for line in f.lines().map_while(Result::ok) {
        let mut left = line.splitn(6, ',');
        let w = left.next().unwrap_or("");
        let _c = left.next(); let dir = left.next().unwrap_or("?");
        if dir != "i" { continue; }
        let wl = w.to_lowercase();
        if !bset.contains(&wl) { continue; }
        let sym = line.split(',').nth(5).unwrap_or("").trim().to_lowercase();
        *recv.entry(sym.clone()).or_insert(0) += 1;
        if dep.contains(sym.as_str()) { whales_hit.entry(sym).or_default().insert(wl); }
    }
    let mut depin_any: HashSet<&String> = HashSet::new();
    for s in whales_hit.values() { for w in s { depin_any.insert(w); } }
    let mut depin_by: Vec<(String, usize)> = whales_hit.iter().map(|(s, w)| (s.clone(), w.len())).collect();
    depin_by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut top_buys: Vec<(String, u64)> = recv.into_iter().collect();
    top_buys.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // BTC-side: dir=o розподіл по роках + місяцях 2024-25
    let mut byyear: BTreeMap<String, f64> = BTreeMap::new();
    let mut byym: HashMap<String, f64> = HashMap::new();
    if let Ok(bf) = File::open(format!("{d}/transfers-btc.csv")) {
        for line in BufReader::new(bf).lines().map_while(Result::ok) {
            let mut left = line.splitn(6, ',');
            let _w = left.next(); let _c = left.next();
            let dir = left.next().unwrap_or("?");
            if dir != "o" { continue; }
            let ts = left.next().unwrap_or("");
            if ts.len() < 7 { continue; }
            let amt: f64 = line.rsplit(',').nth(1).and_then(|a| a.parse().ok()).unwrap_or(0.0);
            *byyear.entry(ts[0..4].to_string()).or_insert(0.0) += amt;
            let yr = &ts[0..4];
            if yr == "2024" || yr == "2025" {
                *byym.entry(ts[0..7].to_string()).or_insert(0.0) += amt;
            }
        }
    }
    let mut top_months: Vec<(String, f64)> = byym.into_iter().collect();
    top_months.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    top_months.truncate(6);

    let cov = 100.0 * depin_any.len() as f64 / band.len().max(1) as f64;
    println!("=== СМУГА (повний набір, {} накоп >0) ===", eth.len());
    println!("китова смуга 100-20k ETH: {}", band.len());
    println!("DePIN-охоплення: {}/{} = {:.0}%", depin_any.len(), band.len(), cov);
    println!("топ DePIN: {:?}", &depin_by[..depin_by.len().min(8)]);
    println!("топ-купівлі: {:?}", top_buys.iter().take(12).map(|x| &x.0).collect::<Vec<_>>());
    println!("BTC роздано по роках: {:?}", byyear.iter().map(|(y, v)| (y, v.round())).collect::<Vec<_>>());
    println!("топ-місяці роздачі 2024-25: {:?}", top_months.iter().map(|(m, v)| (m, v.round())).collect::<Vec<_>>());

    let mut top_band: Vec<(&String, &f64)> = band.iter().collect();
    top_band.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    let cohort = json!({
        "band_full": band.len(),
        "eth_pos_accum": eth.len(),
        "depin_cov_pct": cov.round(),
        "depin_by_token": depin_by.iter().map(|(s, c)| (s.clone(), *c)).collect::<HashMap<_,_>>(),
        "btc_distribution_by_year": byyear.iter().map(|(y, v)| (y.clone(), v.round())).collect::<HashMap<_,_>>(),
        "btc_top_dist_months_2425": top_months.iter().map(|(m, v)| json!([m, v.round()])).collect::<Vec<_>>(),
        "top_band": top_band.iter().take(30).map(|(w, v)| ((*w).clone(), **v)).collect::<HashMap<_,_>>(),
    });
    serde_json::to_writer_pretty(File::create(format!("{d}/eth-rotation-cohort.json")).unwrap(), &cohort).unwrap();
    let sig: Value = json!({
        "note": "BTC-exit(dist by year)/ETH-entry(09.2025+ by construction) темпоральний підпис; адреси не лінкуються (різні мережі)",
        "btc_dist_by_year": byyear.iter().map(|(y, v)| (y.clone(), v.round())).collect::<HashMap<_,_>>(),
        "btc_top_months_2425": top_months.iter().map(|(m, v)| json!([m, v.round()])).collect::<Vec<_>>(),
        "eth_entry": format!("усі {} накоп. — net ETH з 2025-09-01 (startblock 23264565)", eth.len()),
    });
    serde_json::to_writer_pretty(File::create(format!("{d}/btc-eth-signature.json")).unwrap(), &sig).unwrap();
    eprintln!("збережено: eth-rotation-cohort.json, btc-eth-signature.json");
}
