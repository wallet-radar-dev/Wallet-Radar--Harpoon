//! mmfind — хто в групі поводиться як МАРКЕТ-МЕЙКЕР і на скільки рухається ціна навколо його дій.
//!
//! Ознака ММ (не «багато торгує», а саме мейкер):
//!   * обіг великий, а ЧИСТА позиція мала — скільки зайшло, стільки й вийшло;
//!   * операцій багато й вони двосторонні;
//!   * строк утримання короткий.
//! Формально: `balance = |in−out| / (in+out)`. У мейкера це ~0, у інвестора ~1.
//! Скор = (1 − balance) × ln(операцій) — щоб одна випадкова пара «купив-продав» не пролізла.
//!
//! ВПЛИВ НА ЦІНУ рахуємо чесно й консервативно: беремо дні, коли ММ купував найбільше
//! (топ-децила за обсягом), і дивимось медіанну зміну ціни через 1/7/30 днів ПОРІВНЯНО зі
//! звичайним днем того ж токена. Якщо різниці нема — «роздування ціни» не підтверджується.
//! Це не доказ причинності: збіг у часі може означати, що ММ купує НА зростанні, а не спричиняє його.
//!
//! Вихід: dataset/backfill/mmfind.json. Args: ROOT [WALLET_FILE]. Env: CHAIN(1) TOP(30) MIN_OPS(20)
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
fn price_at(s: &[(u64, f64)], ts: u64) -> Option<f64> {
    if s.is_empty() { return None }
    match s.binary_search_by_key(&ts, |p| p.0) {
        Ok(i) => Some(s[i].1), Err(0) => None, Err(i) => Some(s[i - 1].1),
    }
}
fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0 }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[derive(Default, Clone)]
struct P { inn: f64, out: f64, n: u32, first: u64, last: u64 }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let top: usize = envn("TOP", 30);
    let min_ops: u32 = envn("MIN_OPS", 20);
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();

    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).unwrap())).unwrap();
    let mut price: HashMap<String, f64> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Some(o) = cur["tokens"].as_object() {
        for (c, v) in o {
            price.insert(c.to_lowercase(), v["usd"].as_f64().unwrap_or(0.0));
            symbol.insert(c.to_lowercase(), v["symbol"].as_str().unwrap_or("").to_string());
        }
    }
    let hist: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices-hist.json")).unwrap())).unwrap();
    let hroot = if hist.get("tokens").is_some() { &hist["tokens"] } else { &hist };
    let mut series: HashMap<String, Vec<(u64, f64)>> = HashMap::new();
    if let Some(o) = hroot.as_object() {
        for (c, v) in o {
            let mut p: Vec<(u64, f64)> = v["prices"].as_array().cloned().unwrap_or_default().iter()
                .filter_map(|x| Some((x[0].as_u64()?, x[1].as_f64()?))).collect();
            p.sort_by_key(|x| x.0);
            if p.len() >= 30 { series.insert(c.to_lowercase(), p); }
        }
    }
    // Контракти з ММ-списку геть: половина топу виявилась роутерами й пулами (1inch
    // `0x1111111254fb` з 137k операцій і 5094 оп/день — це інфраструктура, а не мейкер).
    // Прапорці накопичує `codecheck`; з 914 перевірених ММ-адрес контрактами були 175.
    let mut is_contract: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/contract-flags.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m { if v.as_bool() == Some(true) { is_contract.insert(a.to_lowercase()); } }
        }
    }
    eprintln!("[mmfind] членів {} · цін {} · рядів {} · відомих контрактів {}",
        members.len(), price.len(), series.len(), is_contract.len());

    let mut pair: HashMap<(String, String), P> = HashMap::new();
    // денні купівлі ММ-кандидатів: (токен, день) -> обсяг, для оцінки впливу на ціну
    let mut buys: HashMap<(String, u64), f64> = HashMap::new();
    let mut tok_in: HashMap<String, f64> = HashMap::new();
    let mut tok_out: HashMap<String, f64> = HashMap::new();

    let rdr = BufReader::with_capacity(1 << 22,
        File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let mut n = 0u64;

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків"); }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear(); }
        let Some(tok) = field(&line, "tok") else { continue };
        if tok.is_empty() { continue }
        let tok = tok.to_lowercase();
        if !series.contains_key(&tok) { continue }          // без історії ціни сенсу нема
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
        if t["k"].as_str() != Some("tok") { continue }

        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), t["f"].as_str().unwrap_or(""),
                  t["t"].as_str().unwrap_or(""), t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211); }
        }
        if !seen.insert(h) { continue }

        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        let ts = t["ts"].as_u64().unwrap_or(0);
        let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
        let amt = t["v"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 10f64.powi(dec);
        if amt <= 0.0 || ts == 0 { continue }
        // всередині групи — не ринкова дія
        if members.contains(frm.as_str()) && members.contains(to.as_str()) { continue }

        let e = pair.entry((cur_w.clone(), tok.clone())).or_default();
        e.n += 1;
        if e.first == 0 || ts < e.first { e.first = ts }
        if ts > e.last { e.last = ts }
        if to == cur_w {
            e.inn += amt;
            *tok_in.entry(tok.clone()).or_insert(0.0) += amt;
            *buys.entry((tok, ts / 86400)).or_insert(0.0) += amt;
        } else if frm == cur_w {
            e.out += amt;
            // повний обіг групи = вхід + вихід. Раніше рахувався лише вхід і множився на 2
            // (припущення in≈out) — через це частка мейкерів виходила понад 100%
            // (REQ 106%, gomining 101%), що неможливо за визначенням.
            *tok_out.entry(tok.clone()).or_insert(0.0) += amt;
        }
    }
    eprintln!("[mmfind] рядків {n} · пар {}", pair.len());

    // --- скоринг ММ ---
    let mut mm: Vec<Value> = Vec::new();
    let mut mm_by_tok: HashMap<String, (f64, u32)> = HashMap::new();
    for ((w, tok), p) in &pair {
        if p.n < min_ops { continue }
        if is_contract.contains(w) { continue }
        let turn = p.inn + p.out;
        if turn <= 0.0 { continue }
        let bal = (p.inn - p.out).abs() / turn;
        let score = (1.0 - bal) * (p.n as f64).ln();
        if bal > 0.25 { continue }                      // лишилось надто багато — це не мейкер
        let px = price.get(tok).copied().unwrap_or(0.0);
        let days = ((p.last - p.first) / 86400).max(1);
        mm.push(json!({
            "wallet": w, "token": tok, "symbol": symbol.get(tok).cloned().unwrap_or_default(),
            "in": p.inn, "out": p.out, "turnover": turn, "turnoverUsd": turn * px,
            "netPct": 100.0 * bal, "ops": p.n, "days": days,
            "opsPerDay": p.n as f64 / days as f64, "score": score,
        }));
        let e = mm_by_tok.entry(tok.clone()).or_insert((0.0, 0));
        e.0 += turn; e.1 += 1;
    }
    mm.sort_by(|a, b| b["turnoverUsd"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["turnoverUsd"].as_f64().unwrap_or(0.0)).unwrap());

    // --- частка ММ в обігу кожного токена ---
    let mut share: Vec<Value> = Vec::new();
    for (tok, (turn, cnt)) in &mm_by_tok {
        let all_turn = tok_in.get(tok).copied().unwrap_or(0.0)
            + tok_out.get(tok).copied().unwrap_or(0.0);
        if all_turn <= 0.0 { continue }
        share.push(json!({
            "token": tok, "symbol": symbol.get(tok).cloned().unwrap_or_default(),
            "mmTurnover": turn, "groupTurnover": all_turn,
            "mmSharePct": (100.0 * turn / all_turn).min(100.0),  // обіг ММ проти повного обігу групи
            "mmWallets": cnt,
            "usd": turn * price.get(tok).copied().unwrap_or(0.0),
        }));
    }
    share.sort_by(|a, b| b["usd"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["usd"].as_f64().unwrap_or(0.0)).unwrap());

    // --- вплив на ціну: великі дні купівлі проти звичайних ---
    let mut impact: Vec<Value> = Vec::new();
    for (tok, s) in &series {
        let mut days: Vec<(u64, f64)> = buys.iter()
            .filter(|((t, _), _)| t == tok).map(|((_, d), v)| (*d, *v)).collect();
        if days.len() < 20 { continue }
        days.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let cut = (days.len() / 10).max(3);
        let big: Vec<u64> = days.iter().take(cut).map(|d| d.0).collect();
        let rest: Vec<u64> = days.iter().skip(cut).map(|d| d.0).collect();
        let f = |set: &Vec<u64>, horizon: u64| -> f64 {
            let mut v: Vec<f64> = Vec::new();
            for d in set {
                let t0 = d * 86400;
                if let (Some(a), Some(b)) = (price_at(s, t0), price_at(s, t0 + horizon * 86400)) {
                    if a > 0.0 { v.push(100.0 * (b - a) / a) }
                }
            }
            median(&mut v)
        };
        let (b1, b7, b30) = (f(&big, 1), f(&big, 7), f(&big, 30));
        let (r1, r7, r30) = (f(&rest, 1), f(&rest, 7), f(&rest, 30));
        // ТЕСТ НА ВИПЕРЕДЖЕННЯ: що робила ціна ДО купівлі. Якщо вона вже росла так само —
        // група приєднується до руху, а не створює його. Якщо до купівлі рух ~0, а після
        // додатний — це вже схоже на вплив (але й тоді не доказ: могла бути спільна причина).
        let pre = |set: &Vec<u64>, back: u64| -> f64 {
            let mut v: Vec<f64> = Vec::new();
            for d in set {
                let t0 = d * 86400;
                if t0 < back * 86400 { continue }
                if let (Some(a), Some(b)) = (price_at(s, t0 - back * 86400), price_at(s, t0)) {
                    if a > 0.0 { v.push(100.0 * (b - a) / a) }
                }
            }
            median(&mut v)
        };
        let (p7, p30) = (pre(&big, 7), pre(&big, 30));
        impact.push(json!({
            "token": tok, "symbol": symbol.get(tok).cloned().unwrap_or_default(),
            "bigDays": big.len(), "otherDays": rest.len(),
            "big1d": b1, "big7d": b7, "big30d": b30,
            "base1d": r1, "base7d": r7, "base30d": r30,
            "excess7d": b7 - r7, "excess30d": b30 - r30,
            "pre7d": p7, "pre30d": p30,
            // ключове число: наскільки рух ПІСЛЯ сильніший за рух ДО тієї самої купівлі
            "leadGap7d": b7 - p7,
        }));
    }
    impact.sort_by(|a, b| b["excess30d"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["excess30d"].as_f64().unwrap_or(0.0)).unwrap());

    let outp = format!("{bf}/mmfind.json");
    let _ = File::create(&outp).unwrap().write_all(serde_json::to_string(
        &json!({"mm": mm, "share": share, "impact": impact})).unwrap().as_bytes());

    eprintln!("\n=== ММ-ПОВЕДІНКА: топ-{top} пар (гаманець×токен) ===");
    eprintln!("{:<14} {:<8} {:>14} {:>7} {:>7} {:>8}", "ГАМАНЕЦЬ", "ТОКЕН", "ОБІГ $", "ЛИШИВ%", "ОПЕР", "ОП/ДЕНЬ");
    for r in mm.iter().take(top) {
        eprintln!("{:<14} {:<8} {:>14.0} {:>6.1}% {:>7} {:>8.2}",
            &r["wallet"].as_str().unwrap_or("")[..14.min(r["wallet"].as_str().unwrap_or("").len())],
            r["symbol"].as_str().unwrap_or(""), r["turnoverUsd"].as_f64().unwrap_or(0.0),
            r["netPct"].as_f64().unwrap_or(0.0), r["ops"].as_u64().unwrap_or(0),
            r["opsPerDay"].as_f64().unwrap_or(0.0));
    }
    eprintln!("\n=== ЧАСТКА ММ В ОБІГУ ТОКЕНА ===");
    for r in share.iter().take(20) {
        eprintln!("  {:<10} обіг ММ ${:>13.0} · {:.1}% усього обігу групи · мейкерів {}",
            r["symbol"].as_str().unwrap_or(""), r["usd"].as_f64().unwrap_or(0.0),
            r["mmSharePct"].as_f64().unwrap_or(0.0), r["mmWallets"].as_u64().unwrap_or(0));
    }
    eprintln!("\n=== ЧИ РОЗДУВАЄ ЦІНУ (медіана зміни після найбільших днів купівлі) ===");
    eprintln!("{:<10} {:>9} {:>9} {:>9} {:>9} {:>10}", "ТОКЕН", "+7д ММ", "+7д звич", "НАДЛИШОК", "+30д ММ", "НАДЛ.30д");
    for r in impact.iter().take(20) {
        eprintln!("{:<10} {:>8.1}% {:>8.1}% {:>8.1}% {:>8.1}% {:>9.1}%",
            r["symbol"].as_str().unwrap_or(""), r["big7d"].as_f64().unwrap_or(0.0),
            r["base7d"].as_f64().unwrap_or(0.0), r["excess7d"].as_f64().unwrap_or(0.0),
            r["big30d"].as_f64().unwrap_or(0.0), r["excess30d"].as_f64().unwrap_or(0.0));
    }
    eprintln!("\n[mmfind] збережено {outp}");
}
