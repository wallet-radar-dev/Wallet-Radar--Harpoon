//! leadlag — вирішує суперечність між радою ШІ і нашим rgflow: що НАСПРАВДІ раніше —
//! DEX-злив альтів мережею чи депозит на CEX? Гіпотеза «двостадійної труби»:
//! альти → DEX за ETH/стейбли → консолідація → CEX-кешаут. Якщо так, DEX-нога ВИПЕРЕДЖАЄ CEX-ногу,
//! і ранній сигнал треба будувати на ній, а не на депозитах (як радила рада).
//!
//! Ряд A (DEX-злив): transfers.csv, dir=o, priced-альт, контрагент=КОНТРАКТ і не біржа → USD.
//! Ряд B (CEX-депозит): core-cex-events.jsonl (краул cexcrawl), dir=out → USD.
//! Обидва — по ОДНІЙ популяції (ядро rglead coverage>=COV), денні бакети unix-днів.
//! Крос-кореляція за лагами: r(лаг L) = corr(A[t], B[t+L]); L>0 = A ВИПЕРЕДЖАЄ B на L днів.
//! Нуль: 500 циклічних зсувів B (детермінований xorshift) → p для max|r| у вікні лагів.
//! Вихід: dataset/backfill/leadlag.json. Env: COV(5) AGG(7 днів у бакеті) MAXLAG(12 бакетів) ITERS(500)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn is_junk(a: &str) -> bool { a.is_empty() || wr_store::addrs::is_junk_addr(a) }
fn is_stable(sym: &str) -> bool { let s = sym.to_uppercase(); s.contains("USD") || s.contains("EUR") || s.contains("GBP") || matches!(s.as_str(), "DAI"|"FRAX"|"LUSD"|"TUSD"|"USTB"|"AUSD"|"GHO"|"MIM"|"WBTC"|"CBBTC"|"TBTC"|"WETH"|"WBETH"|"CBETH"|"WSTETH"|"STETH"|"RETH"|"WEETH"|"EETH") || s.ends_with("ON") }

/// unix-день з ISO-дати `YYYY-MM-DD...` (Хіннант)
fn day_from_iso(ts: &str) -> Option<i64> {
    if ts.len() < 10 { return None; }
    let y: i64 = ts[0..4].parse().ok()?;
    let m: i64 = ts[5..7].parse().ok()?;
    let d: i64 = ts[8..10].parse().ok()?;
    if !(2015..2100).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) { return None; }
    let yy = if m <= 2 { y - 1 } else { y };
    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
    let yoe = yy - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}
fn iso_from_day(mut z: i64) -> String {
    z += 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n < 4 { return 0.0; }
    let (ma, mb) = (a[..n].iter().sum::<f64>() / n as f64, b[..n].iter().sum::<f64>() / n as f64);
    let (mut sab, mut sa, mut sb) = (0.0, 0.0, 0.0);
    for i in 0..n { let (x, y) = (a[i] - ma, b[i] - mb); sab += x * y; sa += x * x; sb += y * y; }
    if sa <= 0.0 || sb <= 0.0 { return 0.0; }
    sab / (sa.sqrt() * sb.sqrt())
}
struct Rng(u64);
impl Rng { fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x } }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let cov: u64 = std::env::var("COV").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let agg: i64 = std::env::var("AGG").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
    let maxlag: i64 = std::env::var("MAXLAG").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
    let iters: usize = std::env::var("ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    let bf = format!("{root}/dataset/backfill");

    // --- популяція: ядро (та сама, що в cexcrawl) ---
    let lead: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/rglead.json")).expect("rglead"))).expect("rglead parse");
    let mut cores: HashSet<String> = HashSet::new();
    for w in lead["wallets"].as_array().cloned().unwrap_or_default() {
        if w["coverage"].as_u64().unwrap_or(0) >= cov && w["kind"].as_str() != Some("contract") {
            if let Some(a) = w["w"].as_str() { cores.insert(a.to_lowercase()); }
        }
    }

    // --- довідники ---
    let mut supply: HashMap<String, String> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { supply.insert(c.to_lowercase(), v["symbol"].as_str().unwrap_or("").to_string()); }
    }
    // ринкові ціни (tokprices/DefiLlama) + осудний відкіт на ончейн-своп-ціну
    let tp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices.json")).expect("token-prices.json — спершу tokprices"))).expect("token-prices parse");
    let mut px: HashMap<String, f64> = tp["tokens"].as_object().map(|m| m.iter().filter_map(|(k, v)| v["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect()).unwrap_or_default();
    let onchain: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())).unwrap_or_default();
    for (k, v) in &onchain {
        let k = k.to_lowercase();
        if px.contains_key(&k) { continue; }
        if let Some(u) = v["usd"].as_f64() { if u > 0.0 && u < 10_000.0 { px.insert(k, u); } }
    }
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc"))
        || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase();
            ["binance","crypto.com","okx","gate","coinbase","kraken","bybit","kucoin","bitget","bitfinex","huobi","mexc","gemini","korbit","upbit","bithumb","uphold","bittrex","poloniex","paribu","btcturk","bingx","gopax","deposit"].iter().any(|c| l.contains(c)) }).unwrap_or(false);
    let is_contract = |a: &str| kind.get(a).map(|s| s == "contract").unwrap_or(false);
    let priced_alt = |t: &str| px.contains_key(t) && supply.get(t).map(|s| !is_stable(s)).unwrap_or(false);

    // --- РЯД A: DEX-злив ядра (transfers.csv) ---
    let mut a_day: HashMap<i64, f64> = HashMap::new();
    let f = BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).expect("transfers"));
    let mut rows = 0u64;
    for line in f.lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or("");
        if w == "wallet" || d != "o" { continue; }
        let wl = w.to_lowercase();
        if !cores.contains(&wl) { continue; }
        let tl = tok.to_lowercase();
        if !priced_alt(&tl) { continue; }
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 || is_junk(&cp) { continue; }
        if !is_contract(&cp) || is_cex(&cp) { continue; }          // саме DEX/DeFi-контракт, не біржа
        let Some(day) = day_from_iso(ts) else { continue };
        *a_day.entry(day).or_insert(0.0) += amt * px.get(&tl).copied().unwrap_or(0.0);
        rows += 1;
    }

    // --- РЯД B: депозити ядра на CEX (краул) ---
    let mut b_day: HashMap<i64, f64> = HashMap::new();
    let mut b_rows = 0u64;
    if let Ok(f) = File::open(format!("{bf}/core-cex-events.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["dir"] != "out" { continue; }
            let usd = v["usd"].as_f64().unwrap_or(0.0);
            if usd <= 0.0 { continue; }
            let ts = v["ts"].as_u64().unwrap_or(0) as i64;
            if ts <= 0 { continue; }
            *b_day.entry(ts / 86400).or_insert(0.0) += usd;
            b_rows += 1;
        }
    }
    if b_day.is_empty() { eprintln!("[leadlag] ПОРОЖНІЙ ряд B — спершу прогнати cexcrawl"); return; }

    // --- спільний діапазон, бакети по AGG днів ---
    let mut lo = *a_day.keys().min().unwrap_or(&0).max(b_day.keys().min().unwrap_or(&0));
    let hi = *a_day.keys().max().unwrap_or(&0).min(b_day.keys().max().unwrap_or(&0));
    // FROM=YYYY-MM-DD — обмежити вікно (рання історія розріджена, нулі розмивають кореляцію)
    if let Some(f) = std::env::var("FROM").ok().and_then(|s| day_from_iso(&s)) { if f > lo { lo = f; } }
    if hi - lo < agg * 8 { eprintln!("[leadlag] замалий спільний діапазон ({} днів) — кореляція недоречна", hi - lo); return; }
    let nb = ((hi - lo) / agg + 1) as usize;
    let (mut a, mut b) = (vec![0.0; nb], vec![0.0; nb]);
    for (d, v) in &a_day { if *d >= lo && *d <= hi { a[((d - lo) / agg) as usize] += v; } }
    for (d, v) in &b_day { if *d >= lo && *d <= hi { b[((d - lo) / agg) as usize] += v; } }
    // log1p — гасить одиничні мега-викиди, лишає форму сплесків
    let la: Vec<f64> = a.iter().map(|x| (x.max(0.0) + 1.0).ln()).collect();
    let lb: Vec<f64> = b.iter().map(|x| (x.max(0.0) + 1.0).ln()).collect();

    // r(лаг): A[t] проти B[t+L]
    let corr_at = |lag: i64, bb: &Vec<f64>| -> f64 {
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for t in 0..nb as i64 {
            let u = t + lag;
            if u < 0 || u >= nb as i64 { continue; }
            xs.push(la[t as usize]); ys.push(bb[u as usize]);
        }
        pearson(&xs, &ys)
    };
    let mut curve: Vec<Value> = Vec::new();
    let (mut best_lag, mut best_r) = (0i64, 0.0f64);
    for lag in -maxlag..=maxlag {
        let r = corr_at(lag, &lb);
        curve.push(json!({"lagBuckets": lag, "lagDays": lag * agg, "r": r}));
        if r.abs() > best_r.abs() { best_r = r; best_lag = lag; }
    }

    // нуль: циклічний зсув B (зберігає автокореляцію B, руйнує вирівнювання з A)
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut ge = 0usize;
    for _ in 0..iters {
        let sh = (rng.next() % nb as u64) as usize;
        let bs: Vec<f64> = (0..nb).map(|i| lb[(i + sh) % nb]).collect();
        let mut m = 0.0f64;
        for lag in -maxlag..=maxlag { let r = corr_at(lag, &bs); if r.abs() > m.abs() { m = r; } }
        if m.abs() >= best_r.abs() { ge += 1; }
    }
    let p = (ge as f64 + 1.0) / (iters as f64 + 1.0);

    let verdict = if p > 0.05 {
        "лід-лаг НЕ значущий: ряди не вирівнюються стабільно — жоден із двох сигналів не доведено раннішим"
    } else if best_lag > 0 {
        "DEX-злив ВИПЕРЕДЖАЄ CEX-депозит → двостадійна труба підтверджена, ранній сигнал = DEX-нога (рада вказала на пізню стадію)"
    } else if best_lag < 0 {
        "CEX-депозит ВИПЕРЕДЖАЄ DEX-злив → порада ради вірна, депозит на біржу є раннішим маркером"
    } else {
        "сигнали синхронні (лаг 0) — переваги в часі жоден не дає"
    };

    let out = json!({
        "population": {"cores": cores.len(), "coverageMin": cov},
        "aggDays": agg, "buckets": nb,
        "rangeFrom": iso_from_day(lo), "rangeTo": iso_from_day(hi),
        "seriesA": {"what": "DEX-злив ядра (USD)", "rows": rows, "total": a.iter().sum::<f64>(), "nonZeroBuckets": a.iter().filter(|x| **x > 0.0).count()},
        "seriesB": {"what": "депозити ядра на CEX (USD)", "rows": b_rows, "total": b.iter().sum::<f64>(), "nonZeroBuckets": b.iter().filter(|x| **x > 0.0).count()},
        "bestLagBuckets": best_lag, "bestLagDays": best_lag * agg, "bestR": best_r,
        "pPermutation": p, "iters": iters,
        "curve": curve,
        "verdict": verdict,
        "seriesADaily": a_day.iter().map(|(d, v)| json!({"d": iso_from_day(*d), "usd": v})).collect::<Vec<Value>>(),
        "seriesBDaily": b_day.iter().map(|(d, v)| json!({"d": iso_from_day(*d), "usd": v})).collect::<Vec<Value>>(),
    });
    std::fs::write(format!("{bf}/leadlag.json"), serde_json::to_string_pretty(&out).unwrap()).ok();

    eprintln!("[leadlag] ===== ЛІД-ЛАГ: DEX-злив проти CEX-депозиту =====");
    eprintln!("[leadlag] ядро: {} гаманців · діапазон {} … {} · бакет {agg} дн ({nb} бакетів)", cores.len(), iso_from_day(lo), iso_from_day(hi));
    eprintln!("[leadlag] A (DEX-злив): {rows} переказів, ${:.0}, ненульових бакетів {}", a.iter().sum::<f64>(), a.iter().filter(|x| **x > 0.0).count());
    eprintln!("[leadlag] B (CEX-депозит): {b_rows} переказів, ${:.0}, ненульових бакетів {}", b.iter().sum::<f64>(), b.iter().filter(|x| **x > 0.0).count());
    for c in curve.iter() {
        let r = c["r"].as_f64().unwrap_or(0.0);
        let bar = "█".repeat(((r.abs() * 40.0) as usize).min(40));
        eprintln!("   лаг {:>4} дн  r={:+.3} {}{}", c["lagDays"].as_i64().unwrap_or(0), r, bar, if c["lagBuckets"] == best_lag { "  ← max" } else { "" });
    }
    eprintln!("[leadlag] найкращий лаг: {} днів, r={:+.3}, p(перестановка)={:.4}", best_lag * agg, best_r, p);
    eprintln!("[leadlag] ВЕРДИКТ: {verdict}");
    eprintln!("[leadlag] збережено leadlag.json");
}
