//! srctest — ТЕСТ РАДИ (питання C): чи адреси, що живлять скарбницю, самі фінансуються
//! з малого спільного набору? Якщо так — версія «незалежні клієнти заводять гроші» відпадає.
//!
//! ЧОМУ ПОТРІБНА НУЛЬ-МОДЕЛЬ: будь-яка вибірка гаманців має «спільні джерела» просто тому, що
//! всі беруть ETH із кількох великих бірж. Тому концентрацію порівнюємо з випадковими наборами
//! такого ж розміру, і ОКРЕМО рахуємо джерела-біржі й немічені EOA: спільний Binance — не доказ,
//! спільний немічений EOA — доказ.
//!
//! Вхід: eth-edges2.jsonl (ребра з сумами), список цілей (файл), labels-cache, wallet-kind.
//! Вихід: dataset/backfill/srctest.json + друк. Args: ROOT TARGET_FILE. Env: TOPK(10) ITERS(200) MIN_ETH(0.01)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

struct Rng(u64);
impl Rng { fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x } }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let tfile = std::env::args().nth(2).expect("потрібен файл зі списком цілей");
    let topk: usize = std::env::var("TOPK").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let iters: usize = std::env::var("ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let min_eth: f64 = std::env::var("MIN_ETH").ok().and_then(|s| s.parse().ok()).unwrap_or(0.01);
    let bf = format!("{root}/dataset/backfill");

    // джерела кожного гаманця (хто йому СЛАВ ETH)
    let mut src_of: HashMap<String, Vec<String>> = HashMap::new();
    for line in BufReader::new(File::open(format!("{bf}/eth-edges2.jsonl")).expect("eth-edges2")).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let w = v["w"].as_str().unwrap_or("").to_lowercase();
        let mut ss: Vec<String> = Vec::new();
        for c in v["cps"].as_array().cloned().unwrap_or_default() {
            if c["i"].as_f64().unwrap_or(0.0) >= min_eth {
                if let Some(a) = c["a"].as_str() { ss.push(a.to_lowercase()); }
            }
        }
        if !ss.is_empty() { src_of.insert(w, ss); }
    }
    let targets: Vec<String> = std::fs::read_to_string(&tfile).expect("target file").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && src_of.contains_key(l)).collect();
    let tset: HashSet<&String> = targets.iter().collect();
    let pool: Vec<&String> = src_of.keys().filter(|w| !tset.contains(*w)).collect();
    eprintln!("[srctest] цілей із даними: {} · пул для нуль-моделі: {}", targets.len(), pool.len());
    if targets.len() < 20 || pool.len() < 50 { eprintln!("[srctest] замало даних"); return; }

    // мітки
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let is_exch = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc"))
        || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase();
            ["binance","coinbase","okx","gate","kraken","bybit","kucoin","bitget","huobi","mexc","bitfinex","upbit","bithumb","paribu","btcturk","bingx","gopax","crypto.com","exchange","hot wallet","deposit"].iter().any(|c| l.contains(c)) }).unwrap_or(false);

    // покриття: скільки цілей мають джерело з топ-K
    let coverage = |set: &[&String], only_non_exch: bool| -> (usize, Vec<(String, usize)>) {
        let mut cnt: HashMap<&str, usize> = HashMap::new();
        for w in set {
            let mut seen: HashSet<&str> = HashSet::new();
            for s in &src_of[*w] {
                if only_non_exch && is_exch(s) { continue; }
                if seen.insert(s.as_str()) { *cnt.entry(s.as_str()).or_insert(0) += 1; }
            }
        }
        let mut v: Vec<(&str, usize)> = cnt.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let top: Vec<&str> = v.iter().take(topk).map(|(a, _)| *a).collect();
        let tops: HashSet<&str> = top.iter().copied().collect();
        let covered = set.iter().filter(|w| src_of[**w].iter().any(|s| tops.contains(s.as_str()))).count();
        (covered, v.into_iter().take(topk).map(|(a, c)| (a.to_string(), c)).collect())
    };

    let tref: Vec<&String> = targets.iter().collect();
    let (cov_all, top_all) = coverage(&tref, false);
    let (cov_eoa, top_eoa) = coverage(&tref, true);

    // нуль: випадкові набори такого ж розміру
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    let (mut ge_all, mut ge_eoa) = (0usize, 0usize);
    let (mut sum_all, mut sum_eoa) = (0.0f64, 0.0f64);
    for _ in 0..iters {
        let mut pick: Vec<&String> = Vec::with_capacity(targets.len());
        let mut used: HashSet<usize> = HashSet::new();
        while pick.len() < targets.len() {
            let i = (rng.next() % pool.len() as u64) as usize;
            if used.insert(i) { pick.push(pool[i]); }
        }
        let (ca, _) = coverage(&pick, false);
        let (ce, _) = coverage(&pick, true);
        sum_all += ca as f64; sum_eoa += ce as f64;
        if ca >= cov_all { ge_all += 1; }
        if ce >= cov_eoa { ge_eoa += 1; }
    }
    let (mean_all, mean_eoa) = (sum_all / iters as f64, sum_eoa / iters as f64);
    let p_all = (ge_all as f64 + 1.0) / (iters as f64 + 1.0);
    let p_eoa = (ge_eoa as f64 + 1.0) / (iters as f64 + 1.0);

    let name = |a: &str| lbl.get(a).and_then(|x| x["name"].as_str()).unwrap_or("").to_string();
    eprintln!("[srctest] ===== ЧИ ЖИВЛЯТЬСЯ ЦІЛІ ЗІ СПІЛЬНОГО ДЖЕРЕЛА? =====");
    eprintln!("[srctest] цілей {} · топ-{topk} джерел покривають {} ({:.0}%); у випадкових наборах — {:.0}; p={:.3}",
        targets.len(), cov_all, 100.0 * cov_all as f64 / targets.len() as f64, mean_all, p_all);
    eprintln!("[srctest] БЕЗ БІРЖ: покривають {} ({:.0}%); у випадкових — {:.0}; p={:.3}",
        cov_eoa, 100.0 * cov_eoa as f64 / targets.len() as f64, mean_eoa, p_eoa);
    eprintln!("[srctest] --- топ джерел (усі) ---");
    for (a, c) in top_all.iter().take(8) { eprintln!("     {c:>4} цілей ← {a} {} {}", if is_exch(a) { "[БІРЖА]" } else { "[EOA]" }, name(a)); }
    eprintln!("[srctest] --- топ джерел, НЕ біржі (це і є доказова частина) ---");
    for (a, c) in top_eoa.iter().take(8) { eprintln!("     {c:>4} цілей ← {a} {}", name(a)); }
    let verdict = if p_eoa <= 0.01 && cov_eoa * 4 >= targets.len() {
        "спільне НЕбіржове джерело значуще й масове → «незалежні клієнти» відпадає, це один оператор"
    } else if p_eoa <= 0.05 {
        "спільне НЕбіржове джерело значуще, але покриває меншість — часткова координація"
    } else {
        "концентрація джерел НЕ вища за випадкову → тест НЕ підтверджує єдиного оператора"
    };
    eprintln!("[srctest] ВЕРДИКТ: {verdict}");
    std::fs::write(format!("{bf}/srctest.json"), serde_json::to_string_pretty(&json!({
        "targets": targets.len(), "topK": topk, "iters": iters,
        "coverageAll": cov_all, "nullMeanAll": mean_all, "pAll": p_all,
        "coverageNonExchange": cov_eoa, "nullMeanNonExchange": mean_eoa, "pNonExchange": p_eoa,
        "topSourcesAll": top_all.iter().map(|(a, c)| json!({"a": a, "targets": c, "exchange": is_exch(a), "label": name(a)})).collect::<Vec<Value>>(),
        "topSourcesNonExchange": top_eoa.iter().map(|(a, c)| json!({"a": a, "targets": c, "label": name(a)})).collect::<Vec<Value>>(),
        "verdict": verdict})).unwrap()).ok();
}
