//! Прод-інтеграція: генератор ⚠️ AVOID-списку. Токени, де кити ЗАРАЗ у топ-децилі
//! накопичення по ЧАСТЦІ ЕМІСІЇ (historically ~66% таких недоперформують ринок за 14д).
//! Вихід: dataset/whale-avoid.json — rust-server віддає через passthrough-роут.
//!
//! Сигнал (найгостріший з пілоту): supply_share(t) = whale net-накопичення в token-одиницях
//! за останні W днів / total_supply. Прапорець, якщо ОСТАННЄ значення в топ-децилі ІСТОРІЇ токена.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

const DAY: i64 = 86_400;
const W: i64 = 7;
const MIN_HIST: usize = 30;
const PCTL: f64 = 0.9; // топ-децил

fn percentile_rank(sorted: &[f64], v: f64) -> f64 {
    if sorted.is_empty() {
        return 0.5;
    }
    let (mut lo, mut hi) = (0usize, sorted.len());
    while lo < hi {
        let m = (lo + hi) / 2;
        if sorted[m] < v {
            lo = m + 1;
        } else {
            hi = m;
        }
    }
    lo as f64 / sorted.len() as f64
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());

    // Валідні токени (мають ціну) + символ/supply зі supply.tsv.
    let mut has_price: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(rd) = fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
        for ent in rd.flatten() {
            let fname = ent.file_name().to_string_lossy().to_string();
            if let Some(stem) = fname.strip_suffix(".json") {
                if let Some((c, a)) = stem.split_once('_') {
                    has_price.insert(format!("{c}\t{a}"));
                }
            }
        }
    }
    let mut supply: HashMap<String, f64> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Ok(txt) = fs::read_to_string(format!("{root}/dataset/nextaction/supply.tsv")) {
        for line in txt.lines().skip(1) {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() >= 3 {
                if let Ok(s) = c[2].parse::<f64>() {
                    if s > 0.0 {
                        let key = format!("{}\t{}", c[0], c[1]);
                        supply.insert(key.clone(), s);
                        symbol.insert(key, c.get(3).unwrap_or(&"").to_string());
                    }
                }
            }
        }
    }
    eprintln!("[avoid] цінових токенів {}, supply відомий {}", has_price.len(), supply.len());

    // Whale net-flow (token units) per (token, day) — лише priced+supply токени.
    let f = BufReader::new(File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"));
    let mut flow: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for line in f.lines() {
        let Ok(line) = line else { continue };
        let mut left = line.splitn(6, ',');
        let _w = left.next();
        let chain = left.next().unwrap_or("");
        let dir = left.next().unwrap_or("?");
        let ts = left.next().unwrap_or("");
        let token = left.next().unwrap_or("");
        if dir != "i" && dir != "o" {
            continue;
        }
        let key = format!("{chain}\t{token}");
        if !supply.contains_key(&key) {
            continue;
        }
        let amount: f64 = line.rsplit(',').nth(1).and_then(|a| a.parse().ok()).unwrap_or(0.0);
        if amount <= 0.0 {
            continue;
        }
        let day = match chrono::DateTime::parse_from_rfc3339(ts) {
            Ok(dt) => dt.timestamp() / DAY,
            Err(_) => continue,
        };
        let signed = if dir == "i" { amount } else { -amount };
        *flow.entry(key).or_default().entry(day).or_insert(0.0) += signed;
    }

    // Для кожного токена: supply_share-історія, перцентиль ОСТАННЬОГО значення.
    let mut flagged: Vec<Value> = Vec::new();
    for (key, fdays) in &flow {
        let sup = supply[key];
        let mut days: Vec<i64> = fdays.keys().copied().collect();
        days.sort_unstable();
        if days.len() < MIN_HIST {
            continue;
        }
        let mut shares: Vec<f64> = Vec::new();
        for &t in &days {
            let acc: f64 = (t - W + 1..=t).filter_map(|d| fdays.get(&d)).sum();
            shares.push(wr_dict::emission_share(acc, sup)); // Фаза 0.2: словник (sup>0 гарант.), байт-паритет
        }
        let latest = *shares.last().unwrap();
        if latest <= 0.0 {
            continue;
        }
        let mut hist = shares.clone();
        hist.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pctl = percentile_rank(&hist, latest);
        if pctl >= PCTL {
            let (chain, token) = key.split_once('\t').unwrap();
            flagged.push(json!({
                "chain": chain,
                "token": token,
                "symbol": symbol.get(key).cloned().unwrap_or_default(),
                "supplySharePct": (latest * 100.0 * 1000.0).round() / 1000.0, // % емісії за W днів
                "percentile": (pctl * 100.0).round(),
                "lastDay": days.last().unwrap(),
            }));
        }
    }
    flagged.sort_by(|a, b| {
        b["supplySharePct"].as_f64().unwrap_or(0.0).partial_cmp(&a["supplySharePct"].as_f64().unwrap_or(0.0)).unwrap()
    });

    let out = json!({
        "generated": days_now(),
        "note": "Кити у топ-децилі накопичення по частці емісії. Історично ~66% таких недоперформують ринок за 14д. Це сигнал ОБЕРЕЖНОСТІ (AVOID), не гарантія.",
        "horizonDays": 14,
        "count": flagged.len(),
        "tokens": flagged,
    });
    let path = format!("{root}/dataset/whale-avoid.json");
    fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).expect("write");
    eprintln!("[avoid] flagged {} токенів -> {path}", out["count"]);
    println!("=== ТОП flagged (AVOID) ===");
    for t in out["tokens"].as_array().unwrap().iter().take(15) {
        println!("  {:<10} {:<9} share={:>7}% pctl={}", t["symbol"].as_str().unwrap_or(""), t["chain"].as_str().unwrap_or(""), t["supplySharePct"], t["percentile"]);
    }
}

// Реальний unix-timestamp генерації (epoch секунди), щоб фронт бачив свіжість списку.
fn days_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
