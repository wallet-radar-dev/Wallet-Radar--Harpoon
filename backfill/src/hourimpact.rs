//! hourimpact — чи рухає група ціну ВСЕРЕДИНІ ДНЯ (годинна роздільність).
//!
//! Денний тест дав нуль: медіана «до» +0.00%, «після» +0.06%, і лише 51% токенів мали
//! рух угору — тобто підкидання монети. Але маркет-мейкер працює в масштабі годин, і денна
//! свічка зливає купівлю з подальшим рухом. Тут те саме питання, але вікна в ГОДИНАХ.
//!
//! Подія — година, коли група купувала найбільше (топ-децила годин за обсягом).
//! Для кожної: ціна за 24/6/1 годину ДО і через 1/6/24 години ПІСЛЯ.
//! Ключове число — `gap = після(+6h) − до(−6h)`: наскільки рух після купівлі сильніший
//! за рух перед нею. Якщо ціна вже росла до входу, група приєднується, а не штовхає.
//!
//! Вихід: dataset/backfill/hourimpact.json. Args: ROOT [WALLET_FILE]. Env: CHAIN(1) TOP(25)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let i = line.find(&pat)? + pat.len();
    let rest = &line[i..];
    let j = rest.find('"')?;
    Some(&rest[..j])
}
/// Ціна на момент ts. Беремо найближчу зліва, але не далі години — інакше «ціна» була б
/// із зовсім іншого моменту й тест став би фікцією.
fn price_at(s: &[(u64, f64)], ts: u64) -> Option<f64> {
    if s.is_empty() { return None }
    let i = match s.binary_search_by_key(&ts, |p| p.0) { Ok(i) => i, Err(0) => return None, Err(i) => i - 1 };
    if ts - s[i].0 > 5400 { return None }        // розрив > 1.5 год — пропускаємо
    Some(s[i].1)
}
fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return f64::NAN }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let top: usize = envn("TOP", 25);
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();

    let hv: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices-hourly.json")).expect("спершу hourprices"))).unwrap();
    let mut series: HashMap<String, Vec<(u64, f64)>> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Some(o) = hv["tokens"].as_object() {
        for (c, v) in o {
            let mut p: Vec<(u64, f64)> = v["prices"].as_array().cloned().unwrap_or_default().iter()
                .filter_map(|x| Some((x[0].as_u64()?, x[1].as_f64()?))).collect();
            p.sort_by_key(|x| x.0);
            if p.len() >= 100 {
                symbol.insert(c.to_lowercase(), v["sym"].as_str().unwrap_or("").to_string());
                series.insert(c.to_lowercase(), p);
            }
        }
    }
    let t_min = series.values().filter_map(|s| s.first().map(|p| p.0)).min().unwrap_or(0);
    eprintln!("[hourimpact] токенів із погодинним рядом: {} · дані від {}", series.len(), t_min);

    // купівлі групи по ГОДИНАХ
    let mut buys: HashMap<(String, u64), f64> = HashMap::new();
    let rdr = BufReader::with_capacity(1 << 22,
        File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let mut n = 0u64;

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків") }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear() }
        let Some(tok) = field(&line, "tok") else { continue };
        if tok.is_empty() { continue }
        let tok = tok.to_lowercase();
        if !series.contains_key(&tok) { continue }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        if t["k"].as_str() != Some("tok") { continue }
        let ts = t["ts"].as_u64().unwrap_or(0);
        if ts < t_min { continue }                        // поза покриттям цін
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if to != cur_w { continue }                       // цікавлять КУПІВЛІ (вхід)
        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        if members.contains(frm.as_str()) { continue }    // внутрішній переказ — не купівля

        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), frm.as_str(), to.as_str(), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211) }
        }
        if !seen.insert(h) { continue }

        let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
        let amt = t["v"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 10f64.powi(dec);
        if amt <= 0.0 { continue }
        *buys.entry((tok, ts / 3600)).or_insert(0.0) += amt;
    }
    eprintln!("[hourimpact] рядків {n} · годин із купівлями {}", buys.len());

    let mut rows: Vec<Value> = Vec::new();
    for (tok, s) in &series {
        let mut hrs: Vec<(u64, f64)> = buys.iter()
            .filter(|((t, _), _)| t == tok).map(|((_, h), v)| (*h, *v)).collect();
        if hrs.len() < 30 { continue }
        hrs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let cut = (hrs.len() / 10).max(5);
        let big: Vec<u64> = hrs.iter().take(cut).map(|x| x.0).collect();
        let rest: Vec<u64> = hrs.iter().skip(cut).map(|x| x.0).collect();

        let mv = |set: &Vec<u64>, from_h: i64, to_h: i64| -> f64 {
            let mut v: Vec<f64> = Vec::new();
            for h in set {
                let t0 = *h as i64 * 3600;
                let (ta, tb) = (t0 + from_h * 3600, t0 + to_h * 3600);
                if ta < 0 || tb < 0 { continue }
                if let (Some(a), Some(b)) = (price_at(s, ta as u64), price_at(s, tb as u64)) {
                    if a > 0.0 { v.push(100.0 * (b - a) / a) }
                }
            }
            median(&mut v)
        };
        let pre6 = mv(&big, -6, 0);
        let post6 = mv(&big, 0, 6);
        let pre24 = mv(&big, -24, 0);
        let post24 = mv(&big, 0, 24);
        let post1 = mv(&big, 0, 1);
        let base6 = mv(&rest, 0, 6);
        if pre6.is_nan() || post6.is_nan() { continue }
        rows.push(json!({
            "token": tok, "symbol": symbol.get(tok).cloned().unwrap_or_default(),
            "events": big.len(), "otherHours": rest.len(),
            "pre24h": pre24, "pre6h": pre6, "post1h": post1, "post6h": post6, "post24h": post24,
            "base6h": base6,
            "gap6h": post6 - pre6,          // рух після мінус рух до — головне число
            "excess6h": post6 - base6,      // проти звичайної години
        }));
    }
    rows.sort_by(|a, b| b["gap6h"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["gap6h"].as_f64().unwrap_or(0.0)).unwrap());

    let gaps: Vec<f64> = rows.iter().filter_map(|r| r["gap6h"].as_f64()).collect();
    let pres: Vec<f64> = rows.iter().filter_map(|r| r["pre6h"].as_f64()).collect();
    let posts: Vec<f64> = rows.iter().filter_map(|r| r["post6h"].as_f64()).collect();
    let pos = gaps.iter().filter(|g| **g > 0.0).count();

    eprintln!("\n=== ВПЛИВ НА ЦІНУ ПО ГОДИНАХ (подія = година найбільших купівель) ===");
    eprintln!("{:<10} {:>8} {:>8} {:>8} {:>8} {:>9} {:>7}", "ТОКЕН", "-24г", "-6г ДО", "+1г", "+6г", "РІЗНИЦЯ", "ПОДІЙ");
    for r in rows.iter().take(top) {
        eprintln!("{:<10} {:>7.2}% {:>7.2}% {:>7.2}% {:>7.2}% {:>8.2}% {:>7}",
            r["symbol"].as_str().unwrap_or(""), r["pre24h"].as_f64().unwrap_or(0.0),
            r["pre6h"].as_f64().unwrap_or(0.0), r["post1h"].as_f64().unwrap_or(0.0),
            r["post6h"].as_f64().unwrap_or(0.0), r["gap6h"].as_f64().unwrap_or(0.0),
            r["events"].as_u64().unwrap_or(0));
    }
    let mut g2 = gaps.clone(); let mut p2 = pres.clone(); let mut q2 = posts.clone();
    eprintln!("\n=== ПІДСУМОК по {} токенах ===", rows.len());
    eprintln!("  медіана руху за 6 год ДО купівлі:    {:+.3}%", median(&mut p2));
    eprintln!("  медіана руху за 6 год ПІСЛЯ:         {:+.3}%", median(&mut q2));
    eprintln!("  медіана різниці (після − до):        {:+.3}%", median(&mut g2));
    eprintln!("  токенів, де ПІСЛЯ > ДО:              {} із {} ({:.0}%)",
        pos, gaps.len(), 100.0 * pos as f64 / gaps.len().max(1) as f64);

    let outp = format!("{bf}/hourimpact.json");
    let _ = File::create(&outp).unwrap().write_all(
        serde_json::to_string(&json!({"rows": rows, "tokens": rows.len(),
            "positiveGap": pos})).unwrap().as_bytes());
    eprintln!("\n[hourimpact] збережено {outp}");
}
