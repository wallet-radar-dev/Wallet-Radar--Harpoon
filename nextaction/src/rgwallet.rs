//! rgwallet — портфель ОДНОГО гаманця: net-позиція по кожному токену + % емісії.
//! Рахує, у скількох монетах він кит (≥ WHALE_PCT% supply). Лише бекфіл. Env: WALLET, WHALE_PCT(1).

use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wallet = std::env::var("WALLET").expect("WALLET").to_lowercase();
    let whale_pct: f64 = std::env::var("WHALE_PCT").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);

    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv {
            let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
            let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); }
        }
    }

    // net + symbol + tx по токену з власних рядків гаманця
    let mut net: HashMap<String, f64> = HashMap::new();
    let mut sym: HashMap<String, String> = HashMap::new();
    let mut tx: HashMap<String, u32> = HashMap::new();
    let f = File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv");
    for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        if w.to_lowercase() != wallet { continue; }
        let _c = it.next();
        let dir = it.next().unwrap_or("");
        let _ts = it.next();
        let tok = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("");
        if dir != "i" && dir != "o" { continue; }
        let s = rest.split(',').next().unwrap_or("");
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        if amt <= 0.0 { continue; }
        let tl = tok.to_lowercase();
        *net.entry(tl.clone()).or_insert(0.0) += if dir == "i" { amt } else { -amt };
        *tx.entry(tl.clone()).or_insert(0) += 1;
        sym.entry(tl).or_insert_with(|| s.to_string());
    }

    // рядки: token, symbol, net, %supply
    let mut rows: Vec<(String, String, f64, f64, u32)> = Vec::new();
    for (t, n) in &net {
        if *n <= 0.0 { continue; }
        let (pct, symbol) = if let Some((hs, sy)) = supply.get(t) { ((n / hs).min(1.0) * 100.0, sy.clone()) } else { (-1.0, sym.get(t).cloned().unwrap_or_default()) };
        rows.push((t.clone(), symbol, *n, pct, tx.get(t).copied().unwrap_or(0)));
    }
    rows.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());

    let whale_in = rows.iter().filter(|r| r.3 >= whale_pct).count();
    let priced = rows.iter().filter(|r| r.3 >= 0.0).count();
    println!("\n===== ГАМАНЕЦЬ {} =====", wallet);
    println!("токенів із net>0: {} (з відомим supply: {})", rows.len(), priced);
    println!("🐋 КИТ (≥{}% емісії) у {} монетах", whale_pct, whale_in);
    println!("\n--- топ-30 позицій за % емісії ---");
    println!("{:>7}  {:<12} {:>12}  {:<44} tx", "%emis", "symbol", "net", "token");
    for (t, s, n, p, c) in rows.iter().take(30) {
        println!("{:>6.2}%  {:<12} {:>12.3e}  {:<44} {}", if *p >= 0.0 { *p } else { 0.0 }, if s.len() > 12 { &s[..12] } else { s }, n, t, c);
    }
}
