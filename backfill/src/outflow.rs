//! outflow — КУДИ група виводить кошти: на біржі, в DEX, чи на звичайні гаманці.
//!
//! Питання, на яке відповідаємо: обіг у мільярди дає реалізований прибуток лише $1.4 млн.
//! Якщо основний вихід іде на ЦЕНТРАЛІЗОВАНІ біржі — результат угоди відбувається поза
//! ланцюгом, і ончейн-P&L його просто не бачить. Якщо ж вихід іде в DEX — угода видима,
//! і тоді малий прибуток означає, що його справді нема.
//!
//! Класифікація адресата (у порядку пріоритету):
//!   CEX      — мітка біржі в labels-cache або wallet-kind=exchange|otc;
//!   DEX      — мітка роутера/пулу (uniswap, curve, balancer, 1inch, sushi, router, pool);
//!   BRIDGE   — мости й крос-чейн (across, hop, stargate, celer, relay, wormhole);
//!   CONTRACT — просто контракт без відомої ролі (contract-flags);
//!   EOA      — звичайний гаманець.
//!
//! Внутрішньогрупові перекази виключено — це не вихід.
//! Вихід: dataset/backfill/outflow.json. Args: ROOT [WALLET_FILE]. Env: CHAIN(1) TOP(25)
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

const CEX_WORDS: [&str; 20] = ["binance", "coinbase", "okx", "kraken", "bybit", "kucoin",
    "gate.io", "gate ", "bitfinex", "huobi", "mexc", "crypto.com", "bitget", "upbit",
    "bithumb", "gemini", "bitstamp", "exchange", "hot wallet", "deposit"];
const DEX_WORDS: [&str; 12] = ["uniswap", "sushi", "curve", "balancer", "1inch", "router",
    "pool", "pancake", "0x:", "paraswap", "cow protocol", "dodo"];
const BRIDGE_WORDS: [&str; 8] = ["across", "hop protocol", "stargate", "celer", "relay",
    "wormhole", "synapse", "bridge"];

/// OTC-десеки й маркет-мейкери — не біржа й не звичайний гаманець. Перший прогін кинув
/// «Genesis Trading: OTC Desk» ($7.3 млрд) і «BlockFi» в EOA, бо їхні мітки не містять
/// жодного з біржових слів; через це «на звичайні гаманці» вийшло 86.6% — цифра-фікція.
const OTC_WORDS: [&str; 10] = ["otc", "genesis trading", "wintermute", "cumberland", "jump trading",
    "alameda", "blockfi", "galaxy digital", "falconx", "amber group"];

fn classify(addr: &str, labels: &HashMap<String, String>, kind: &HashMap<String, String>,
            contracts: &HashSet<String>) -> &'static str {
    // Спалення й обгортка — окремі категорії: це не «вивід комусь», а технічні операції.
    // Null Address забирав $1.89 млрд, WETH-контракт $22.4 млрд — обидва як «EOA».
    if addr.starts_with("0x000000000000000000000000000000000000000") { return "BURN" }
    if addr == wr_store::addrs::weth() { return "WRAP" }
    if let Some(l) = labels.get(addr) {
        let l = l.to_lowercase();
        if l.contains("null address") { return "BURN" }
        if l.contains("wrapped ether") || l.contains("wrapped-token") { return "WRAP" }
        // порядок важливий: «uniswap router» не має потрапити в CEX через слово «exchange»
        if DEX_WORDS.iter().any(|w| l.contains(w)) { return "DEX" }
        if BRIDGE_WORDS.iter().any(|w| l.contains(w)) { return "BRIDGE" }
        if OTC_WORDS.iter().any(|w| l.contains(w)) { return "OTC" }
        if CEX_WORDS.iter().any(|w| l.contains(w)) { return "CEX" }
    }
    if let Some(k) = kind.get(addr) {
        if k == "otc" { return "OTC" }
        if k == "exchange" { return "CEX" }
    }
    if contracts.contains(addr) { return "CONTRACT" }
    "EOA"
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

    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).unwrap())).unwrap();
    let eth_usd = cur["eth"].as_f64().unwrap_or(0.0);
    let mut price: HashMap<String, f64> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Some(o) = cur["tokens"].as_object() {
        for (c, v) in o {
            price.insert(c.to_lowercase(), v["usd"].as_f64().unwrap_or(0.0));
            symbol.insert(c.to_lowercase(), v["symbol"].as_str().unwrap_or("").to_string());
        }
    }

    let mut labels: HashMap<String, String> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr { if let Some(t) = x.as_str() { s.push(' '); s.push_str(t); } }
                }
                if !s.trim().is_empty() { labels.insert(a.to_lowercase(), s); }
            }
        }
    }
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/wallet-kind.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m { if let Some(s) = v.as_str() { kind.insert(a.to_lowercase(), s.to_string()); } }
        }
    }
    let mut contracts: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/contract-flags.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m { if v.as_bool() == Some(true) { contracts.insert(a.to_lowercase()); } }
        }
    }
    eprintln!("[outflow] членів {} · міток {} · kind {} · контрактів {}",
        members.len(), labels.len(), kind.len(), contracts.len());

    // категорія -> (usd_токенів, usd_eth, операцій)
    let mut cat: HashMap<&'static str, (f64, f64, u64)> = HashMap::new();
    let mut dest: HashMap<String, (f64, u64, &'static str)> = HashMap::new();

    let rdr = BufReader::with_capacity(1 << 22,
        File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let (mut n, mut outs) = (0u64, 0u64);

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків, виходів {outs}"); }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear(); }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };

        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), t["k"].as_str().unwrap_or(""),
                  t["f"].as_str().unwrap_or(""), t["t"].as_str().unwrap_or(""),
                  t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211); }
        }
        if !seen.insert(h) { continue }

        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        if frm != cur_w { continue }                       // цікавлять лише ВИХОДИ
        if to.is_empty() || members.contains(to.as_str()) { continue }   // всередині групи — не вихід

        let kindk = t["k"].as_str().unwrap_or("");
        let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if raw <= 0.0 { continue }

        let usd = if kindk == "tok" {
            let c = t["tok"].as_str().unwrap_or("").to_lowercase();
            let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            let amt = raw / 10f64.powi(dec);
            amt * price.get(&c).copied().unwrap_or(0.0)
        } else if kindk == "ext" || kindk == "int" {
            (raw / 1e18) * eth_usd
        } else { 0.0 };
        if usd <= 0.0 { continue }
        outs += 1;

        let c = classify(&to, &labels, &kind, &contracts);
        let e = cat.entry(c).or_insert((0.0, 0.0, 0));
        if kindk == "tok" { e.0 += usd } else { e.1 += usd }
        e.2 += 1;
        let d = dest.entry(to).or_insert((0.0, 0, c));
        d.0 += usd; d.1 += 1;
    }
    eprintln!("[outflow] рядків {n} · виходів назовні {outs}");

    let total: f64 = cat.values().map(|v| v.0 + v.1).sum();
    let mut cats: Vec<(&&str, &(f64, f64, u64))> = cat.iter().collect();
    cats.sort_by(|a, b| (b.1.0 + b.1.1).partial_cmp(&(a.1.0 + a.1.1)).unwrap());

    eprintln!("\n=== КУДИ ЙДУТЬ ВИХОДИ ГРУПИ ===");
    eprintln!("{:<10} {:>16} {:>8} {:>16} {:>16} {:>10}", "КУДИ", "УСЬОГО $", "ЧАСТКА", "у токенах $", "в ETH $", "ОПЕРАЦІЙ");
    for (c, v) in &cats {
        eprintln!("{:<10} {:>16.0} {:>7.1}% {:>16.0} {:>16.0} {:>10}",
            c, v.0 + v.1, 100.0 * (v.0 + v.1) / total.max(1e-9), v.0, v.1, v.2);
    }

    let mut ds: Vec<(&String, &(f64, u64, &str))> = dest.iter().collect();
    ds.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap());
    eprintln!("\n=== ТОП-{top} АДРЕС-ОТРИМУВАЧІВ ===");
    for (a, v) in ds.iter().take(top) {
        let nm = labels.get(*a).map(|s| s.chars().take(34).collect::<String>()).unwrap_or_default();
        eprintln!("  {:<8} ${:>14.0} · {:>7} оп · {} {}", v.2, v.0, v.1, &a[..12], nm);
    }

    let outp = format!("{bf}/outflow.json");
    let _ = File::create(&outp).unwrap().write_all(serde_json::to_string(&json!({
        "totalUsd": total,
        "byCategory": cats.iter().map(|(c, v)| json!({"cat": c, "usd": v.0 + v.1,
            "tokenUsd": v.0, "ethUsd": v.1, "ops": v.2,
            "pct": 100.0 * (v.0 + v.1) / total.max(1e-9)})).collect::<Vec<_>>(),
        "topDest": ds.iter().take(200).map(|(a, v)| json!({"addr": a, "usd": v.0,
            "ops": v.1, "cat": v.2, "label": labels.get(*a)})).collect::<Vec<_>>(),
    })).unwrap().as_bytes());
    eprintln!("\n[outflow] збережено {outp}");
}
