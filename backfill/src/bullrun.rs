//! bullrun — хто з групи насправді працював під час булрану, а хто спав.
//!
//! Період визначено за даними, не навмання: ETH ішов $1687 (квітень 2025) → $4325 (вересень
//! 2025), тобто +156%, і далі вниз до $1684 у червні 2026. Булран = **липень–жовтень 2025**.
//!
//! Рахуємо для кожного гаманця:
//!   * скільки операцій у булран проти решти часу;
//!   * чи він КУПУВАВ (нетто-приплив токенів) чи ПРОДАВАВ у цей період;
//!   * коли була перша й остання дія — щоб відрізнити «спав» від «ще не існував».
//!
//! Категорії:
//!   АКТИВНИЙ    — ≥ MIN_OPS операцій у вікні булрану;
//!   ЕПІЗОДИЧНИЙ — 1..MIN_OPS операцій;
//!   СПАВ        — існував до булрану, але жодної операції в ньому;
//!   ПІЗНІШИЙ    — перша операція вже після булрану (не міг брати участь).
//!
//! Вихід: dataset/backfill/bullrun.json. Args: ROOT [WALLET_FILE].
//! Env: FROM(2025-07-01) TO(2025-11-01) MIN_OPS(10) CHAIN(1)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn envs(k: &str, d: &str) -> String { std::env::var(k).unwrap_or_else(|_| d.to_string()) }
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let i = line.find(&pat)? + pat.len();
    let rest = &line[i..];
    let j = rest.find('"')?;
    Some(&rest[..j])
}
/// «2025-07-01» -> unix. Простий розбір без залежностей: дат тут дві, і обидві наші.
fn ymd_to_ts(s: &str) -> u64 {
    let p: Vec<i64> = s.split('-').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 3 { return 0 }
    let (y, m, d) = (p[0], p[1], p[2]);
    let mut days: i64 = 0;
    for yy in 1970..y {
        days += if (yy % 4 == 0 && yy % 100 != 0) || yy % 400 == 0 { 366 } else { 365 };
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let ml = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for mm in 0..(m - 1) as usize { days += ml[mm] }
    days += d - 1;
    (days * 86400) as u64
}

#[derive(Default)]
struct W {
    ops_bull: u32, ops_other: u32,
    tok_in_bull: f64, tok_out_bull: f64,   // у доларах за поточними цінами
    first: u64, last: u64,
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let min_ops: u32 = envn("MIN_OPS", 10);
    let from = ymd_to_ts(&envs("FROM", "2025-07-01"));
    let to = ymd_to_ts(&envs("TO", "2025-11-01"));
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();
    eprintln!("[bullrun] гаманців у списку: {} · вікно {}..{}",
        members.len(), envs("FROM", "2025-07-01"), envs("TO", "2025-11-01"));

    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).unwrap())).unwrap();
    let eth_usd = cur["eth"].as_f64().unwrap_or(0.0);
    let mut price: HashMap<String, f64> = HashMap::new();
    if let Some(o) = cur["tokens"].as_object() {
        for (c, v) in o { price.insert(c.to_lowercase(), v["usd"].as_f64().unwrap_or(0.0)); }
    }

    let mut w: HashMap<String, W> = HashMap::new();
    let rdr = BufReader::with_capacity(1 << 22,
        File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let mut n = 0u64;

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків") }
        let Some(a) = field(&line, "w") else { continue };
        if !members.contains(a) { continue }
        if a != cur_w { cur_w = a.to_string(); seen.clear() }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };

        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), t["k"].as_str().unwrap_or(""),
                  t["f"].as_str().unwrap_or(""), t["t"].as_str().unwrap_or(""),
                  t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211) }
        }
        if !seen.insert(h) { continue }

        let ts = t["ts"].as_u64().unwrap_or(0);
        if ts == 0 { continue }
        let e = w.entry(cur_w.clone()).or_default();
        if e.first == 0 || ts < e.first { e.first = ts }
        if ts > e.last { e.last = ts }
        let in_bull = ts >= from && ts < to;
        if in_bull { e.ops_bull += 1 } else { e.ops_other += 1 }

        if in_bull {
            let kind = t["k"].as_str().unwrap_or("");
            let frm = t["f"].as_str().unwrap_or("").to_lowercase();
            let to_a = t["t"].as_str().unwrap_or("").to_lowercase();
            let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let usd = if kind == "tok" {
                let c = t["tok"].as_str().unwrap_or("").to_lowercase();
                let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
                (raw / 10f64.powi(dec)) * price.get(&c).copied().unwrap_or(0.0)
            } else { (raw / 1e18) * eth_usd };
            if usd > 0.0 {
                if to_a == cur_w { e.tok_in_bull += usd } else if frm == cur_w { e.tok_out_bull += usd }
            }
        }
    }
    eprintln!("[bullrun] рядків {n} · гаманців із даними {}", w.len());

    let (mut act, mut epi, mut slept, mut later, mut nodata) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let (mut buy, mut sell) = (0u32, 0u32);
    let (mut in_usd, mut out_usd) = (0f64, 0f64);
    let mut rows: Vec<Value> = Vec::new();
    for m in &members {
        match w.get(m) {
            None => { nodata += 1 }
            Some(x) => {
                let cat = if x.ops_bull >= min_ops { act += 1; "АКТИВНИЙ" }
                    else if x.ops_bull > 0 { epi += 1; "ЕПІЗОДИЧНИЙ" }
                    else if x.first >= to { later += 1; "ПІЗНІШИЙ" }
                    else { slept += 1; "СПАВ" };
                if x.ops_bull > 0 {
                    in_usd += x.tok_in_bull; out_usd += x.tok_out_bull;
                    if x.tok_in_bull > x.tok_out_bull { buy += 1 } else { sell += 1 }
                }
                rows.push(json!({"addr": m, "opsBull": x.ops_bull, "opsOther": x.ops_other,
                    "inUsd": x.tok_in_bull, "outUsd": x.tok_out_bull, "cat": cat,
                    "netUsd": x.tok_in_bull - x.tok_out_bull}));
            }
        }
    }
    rows.sort_by(|a, b| b["opsBull"].as_u64().unwrap_or(0).cmp(&a["opsBull"].as_u64().unwrap_or(0)));

    let total = members.len() as f64;
    eprintln!("\n=== УЧАСТЬ У БУЛРАНІ (лип–жов 2025) ===");
    eprintln!("  АКТИВНИХ (≥{min_ops} операцій):  {:>5} ({:.1}%)", act, 100.0 * act as f64 / total);
    eprintln!("  епізодичних (1..{}):        {:>5} ({:.1}%)", min_ops, epi, 100.0 * epi as f64 / total);
    eprintln!("  СПАЛИ (існували, 0 оп):     {:>5} ({:.1}%)", slept, 100.0 * slept as f64 / total);
    eprintln!("  зʼявились пізніше:          {:>5} ({:.1}%)", later, 100.0 * later as f64 / total);
    eprintln!("  без даних у базі:           {:>5}", nodata);
    eprintln!("\n  у булран КУПУВАЛИ: {} гаманців · ПРОДАВАЛИ: {}", buy, sell);
    eprintln!("  обсяг: приплив ${:.0} · відплив ${:.0} · НЕТТО ${:.0}",
        in_usd, out_usd, in_usd - out_usd);
    eprintln!("\n=== ТОП-15 НАЙАКТИВНІШИХ У БУЛРАН ===");
    for r in rows.iter().take(15) {
        eprintln!("  {} {:>6} оп · приплив ${:>12.0} · відплив ${:>12.0}",
            &r["addr"].as_str().unwrap_or("")[..16], r["opsBull"].as_u64().unwrap_or(0),
            r["inUsd"].as_f64().unwrap_or(0.0), r["outUsd"].as_f64().unwrap_or(0.0));
    }

    let _ = File::create(format!("{bf}/bullrun.json")).unwrap().write_all(
        serde_json::to_string(&json!({"window": [envs("FROM","2025-07-01"), envs("TO","2025-11-01")],
            "active": act, "episodic": epi, "slept": slept, "later": later, "noData": nodata,
            "buyers": buy, "sellers": sell, "inUsd": in_usd, "outUsd": out_usd,
            "rows": rows})).unwrap().as_bytes());
    eprintln!("\n[bullrun] збережено bullrun.json");
}
