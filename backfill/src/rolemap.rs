//! rolemap — структура групи: функція КОЖНОГО гаманця і хто кого створив.
//!
//! Підхід протилежний до `regroup`: там membership звужувалось до доведеного ядра, і 75%
//! відсіювалось. Але якщо дрібні гаманці — це навмисне розмиття входів, відсів викидає саме
//! маскування, тобто частину структури. Тут НІКОГО не викидаємо: кожна адреса зі списку
//! отримує функцію, навіть якщо її зв'язки слабкі.
//!
//! ІЄРАРХІЯ будується за ФІНАНСУВАННЯМ, а не за обсягом переказів. Гаманець не може діяти
//! без газу, тож той, хто дав першим ETH, — його творець. Цей слід не розмивається дрібними
//! транзакціями: заправка потрібна рівно один раз і завжди від когось конкретного.
//!
//! ФУНКЦІЇ (за пріоритетом; перша, що підійшла):
//!   КОНТРАКТ    — код за адресою (не гаманець узагалі);
//!   КАЗНА       — великий залишок ETH, мало операцій;
//!   ФАНДЕР      — заправив ≥FUND_MIN адрес (роздавач газу);
//!   ВОРОНКА     — вхідних ≥3× вихідних від багатьох джерел (збирає);
//!   РОЗДАВАЧ    — вихідних ≥3× вхідних на багатьох (розсіює);
//!   ШЛЮЗ_CEX    — ≥50% обсягу йде на біржі/OTC (точка виходу);
//!   ВХІД_CEX    — ≥50% обсягу приходить із бірж (точка входу);
//!   ТРАНЗИТ     — вхід≈вихід, залишок ~0 (переливає далі);
//!   МЕЙКЕР      — багато токенних операцій в обидва боки;
//!   МАСКА       — дрібні суми, коротке життя, мало обсягу (розмиття слідів);
//!   ХОЛДЕР      — має залишок і майже не витрачає;
//!   СПЛЯЧИЙ     — не діяв понад SLEEP_D днів.
//!
//! Вихід: dataset/backfill/rolemap.json + rolemap.csv. Args: ROOT [WALLET_FILE].
//! Env: CHAIN(1) FUND_MIN(5) SLEEP_D(90) MASK_USD(5000)
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

#[derive(Default)]
struct W {
    n_in: u32, n_out: u32, n_tok: u32,
    eth_in: f64, eth_out: f64,
    usd_in: f64, usd_out: f64,
    to_cex: f64, from_cex: f64,
    senders: HashSet<u32>, recips: HashSet<u32>,
    first: u64, last: u64,
    funder: String, funder_ts: u64,     // хто дав перший ETH
    funded: u32,                        // скільки адрес заправив сам
}

const CEX_WORDS: [&str; 24] = ["binance", "coinbase", "okx", "kraken", "bybit", "kucoin",
    "gate", "bitfinex", "huobi", "mexc", "crypto.com", "bitget", "upbit", "bithumb",
    "gemini", "bitstamp", "hot wallet", "deposit", "otc", "genesis trading", "wintermute",
    "cumberland", "jump trading", "alameda"];

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let fund_min: u32 = envn("FUND_MIN", 5);
    let sleep_d: u64 = envn("SLEEP_D", 90);
    let mask_usd: f64 = envn("MASK_USD", 5000.0);
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();

    let mut is_contract: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/contract-flags.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m { if v.as_bool() == Some(true) { is_contract.insert(a.to_lowercase()); } }
        }
    }
    let mut cex: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_lowercase();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr { if let Some(t) = x.as_str() { s.push(' '); s.push_str(&t.to_lowercase()) } }
                }
                if CEX_WORDS.iter().any(|w| s.contains(w)) { cex.insert(a.to_lowercase()); }
            }
        }
    }
    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).unwrap())).unwrap();
    let eth_usd = cur["eth"].as_f64().unwrap_or(0.0);
    let mut price: HashMap<String, f64> = HashMap::new();
    if let Some(o) = cur["tokens"].as_object() {
        for (c, v) in o { price.insert(c.to_lowercase(), v["usd"].as_f64().unwrap_or(0.0)); }
    }
    eprintln!("[rolemap] адрес {} · контрактів {} · біржових міток {}",
        members.len(), is_contract.len(), cex.len());

    let mut w: HashMap<String, W> = HashMap::new();
    let mut idx: HashMap<String, u32> = HashMap::new();
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

        let kind = t["k"].as_str().unwrap_or("");
        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        let ts = t["ts"].as_u64().unwrap_or(0);
        let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if raw <= 0.0 || ts == 0 { continue }

        let usd = if kind == "tok" {
            let c = t["tok"].as_str().unwrap_or("").to_lowercase();
            let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            (raw / 10f64.powi(dec)) * price.get(&c).copied().unwrap_or(0.0)
        } else { (raw / 1e18) * eth_usd };

        let nxt = idx.len() as u32;
        let oid = *idx.entry(if to == cur_w { frm.clone() } else { to.clone() }).or_insert(nxt);
        let e = w.entry(cur_w.clone()).or_default();
        if e.first == 0 || ts < e.first { e.first = ts }
        if ts > e.last { e.last = ts }
        if kind == "tok" { e.n_tok += 1 }

        if to == cur_w {
            e.n_in += 1; e.usd_in += usd; e.senders.insert(oid);
            if kind != "tok" {
                e.eth_in += raw / 1e18;
                // перший вхідний ETH = заправка; хто дав, той і створив цей гаманець
                if e.funder.is_empty() || ts < e.funder_ts { e.funder = frm.clone(); e.funder_ts = ts }
            }
            if cex.contains(&frm) { e.from_cex += usd }
        } else if frm == cur_w {
            e.n_out += 1; e.usd_out += usd; e.recips.insert(oid);
            if kind != "tok" { e.eth_out += raw / 1e18 }
            if cex.contains(&to) { e.to_cex += usd }
        }
    }
    eprintln!("[rolemap] рядків {n} · гаманців із даними {}", w.len());

    // скільки адрес заправив кожен (усередині списку)
    let mut funded_by: HashMap<String, u32> = HashMap::new();
    for (_, e) in &w {
        if !e.funder.is_empty() { *funded_by.entry(e.funder.clone()).or_insert(0) += 1 }
    }
    for (a, e) in w.iter_mut() { e.funded = funded_by.get(a).copied().unwrap_or(0) }

    let now = w.values().map(|x| x.last).max().unwrap_or(0);
    let mut roles: HashMap<&'static str, u32> = HashMap::new();
    let mut rows: Vec<Value> = Vec::new();

    for m in &members {
        let e = match w.get(m) { Some(x) => x, None => {
            *roles.entry("НЕМАЄ ДАНИХ").or_insert(0) += 1;
            rows.push(json!({"addr": m, "role": "НЕМАЄ ДАНИХ"}));
            continue } };
        let tot = e.usd_in + e.usd_out;
        let bal_eth = e.eth_in - e.eth_out;
        let idle_d = if e.last > 0 { (now.saturating_sub(e.last)) / 86400 } else { 9999 };

        let role: &'static str = if is_contract.contains(m) { "КОНТРАКТ" }
            else if e.funded >= fund_min { "ФАНДЕР" }
            else if bal_eth > 1000.0 && e.n_in + e.n_out < 500 { "КАЗНА" }
            else if e.to_cex > 0.5 * e.usd_out && e.usd_out > 0.0 { "ШЛЮЗ_CEX" }
            else if e.from_cex > 0.5 * e.usd_in && e.usd_in > 0.0 { "ВХІД_CEX" }
            else if e.n_in >= 3 * e.n_out.max(1) && e.senders.len() >= 10 { "ВОРОНКА" }
            else if e.n_out >= 3 * e.n_in.max(1) && e.recips.len() >= 10 { "РОЗДАВАЧ" }
            else if idle_d > sleep_d { "СПЛЯЧИЙ" }
            else if e.n_tok >= 50 && e.n_in > 5 && e.n_out > 5 { "МЕЙКЕР" }
            else if e.usd_out > 0.0 && (e.usd_in - e.usd_out).abs() < 0.1 * tot.max(1.0) { "ТРАНЗИТ" }
            else if tot < mask_usd && e.n_in + e.n_out < 200 { "МАСКА" }
            else { "ХОЛДЕР" };
        *roles.entry(role).or_insert(0) += 1;
        rows.push(json!({"addr": m, "role": role, "funder": e.funder, "funded": e.funded,
            "nIn": e.n_in, "nOut": e.n_out, "nTok": e.n_tok,
            "usdIn": e.usd_in, "usdOut": e.usd_out, "toCex": e.to_cex, "fromCex": e.from_cex,
            "ethBal": bal_eth, "senders": e.senders.len(), "recips": e.recips.len(),
            "idleDays": idle_d, "first": e.first, "last": e.last}));
    }

    // --- ІЄРАРХІЯ: рівень = глибина ланцюга фінансування ---
    let mut level: HashMap<String, u32> = HashMap::new();
    for m in &members {
        let mut cur = m.clone();
        let mut d = 0u32;
        let mut guard = 0;
        while let Some(e) = w.get(&cur) {
            if e.funder.is_empty() || !members.contains(&e.funder) { break }
            cur = e.funder.clone();
            d += 1; guard += 1;
            if guard > 20 { break }                 // захист від циклів у графі фінансування
        }
        level.insert(m.clone(), d);
    }
    let mut by_level: HashMap<u32, u32> = HashMap::new();
    for (_, d) in &level { *by_level.entry(*d).or_insert(0) += 1 }

    eprintln!("\n=== ФУНКЦІЇ ГАМАНЦІВ ({} адрес) ===", members.len());
    let mut rs: Vec<(&&str, &u32)> = roles.iter().collect();
    rs.sort_by(|a, b| b.1.cmp(a.1));
    for (r, c) in &rs {
        eprintln!("  {:<12} {:>5} ({:.1}%)", r, c, 100.0 * **c as f64 / members.len() as f64);
    }
    eprintln!("\n=== ІЄРАРХІЯ (глибина ланцюга фінансування) ===");
    let mut ls: Vec<(&u32, &u32)> = by_level.iter().collect();
    ls.sort();
    for (d, c) in ls {
        eprintln!("  рівень {:<2} {:>5} адрес {}", d, c, "#".repeat((*c as usize / 60).min(50)));
    }
    // найбільші фандери — вершина структури
    let mut top: Vec<(&String, u32)> = funded_by.iter().map(|(a, c)| (a, *c)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("\n=== ХТО СТВОРИВ НАЙБІЛЬШЕ ГАМАНЦІВ ===");
    for (a, c) in top.iter().take(12) {
        let inside = if members.contains(*a) { "[у списку]" } else { "[ЗЗОВНІ]" };
        eprintln!("  {} заправив {:>4} адрес {}", &a[..20.min(a.len())], c, inside);
    }

    let outp = format!("{bf}/rolemap.json");
    let _ = File::create(&outp).unwrap().write_all(serde_json::to_string(&json!({
        "roles": roles.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_,_>>(),
        "byLevel": by_level.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_,_>>(),
        "topFunders": top.iter().take(50).map(|(a, c)| json!({"addr": a, "funded": c,
            "inList": members.contains(**&a)})).collect::<Vec<_>>(),
        "rows": rows})).unwrap().as_bytes());
    let mut csv = File::create(format!("{bf}/rolemap.csv")).unwrap();
    let _ = writeln!(csv, "addr,role,level,funder,funded,nIn,nOut,usdIn,usdOut,toCex,idleDays");
    for r in &rows {
        let a = r["addr"].as_str().unwrap_or("");
        let _ = writeln!(csv, "{},{},{},{},{},{},{},{:.0},{:.0},{:.0},{}",
            a, r["role"].as_str().unwrap_or(""), level.get(a).copied().unwrap_or(0),
            r["funder"].as_str().unwrap_or(""), r["funded"].as_u64().unwrap_or(0),
            r["nIn"].as_u64().unwrap_or(0), r["nOut"].as_u64().unwrap_or(0),
            r["usdIn"].as_f64().unwrap_or(0.0), r["usdOut"].as_f64().unwrap_or(0.0),
            r["toCex"].as_f64().unwrap_or(0.0), r["idleDays"].as_u64().unwrap_or(0));
    }
    eprintln!("\n[rolemap] збережено rolemap.json і rolemap.csv");
}
