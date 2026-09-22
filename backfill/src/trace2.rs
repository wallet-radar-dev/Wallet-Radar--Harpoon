//! trace2 — другий хоп: куди йдуть гроші з тих EOA, на які група вивела 75.5% обсягу.
//!
//! Питання: перший рівень показав, що на біржі напряму йде 9.4%, а 75.5% — на звичайні
//! гаманці. Але серед перевірених EOA частина шле далі на біржі. Отже пряма частка бірж
//! занижена, і треба порахувати, скільки доходить туди через один додатковий переказ.
//!
//! Метод: для кожної EOA першого рівня беремо її ВИХОДИ і класифікуємо адресатів так само,
//! як у `outflow`. Сумуємо в доларах за поточними цінами (історичних тут не треба: питання
//! не «скільки заробили», а «куди пішло»).
//!
//! Обмеження, які треба знати: беремо останні OFFSET операцій кожної адреси, тож давня
//! історія може лишитись поза кадром; і це другий хоп, а не повний шлях до кінця.
//!
//! Вихід: dataset/backfill/trace2.json. Args: ROOT ADDR_FILE. Env: OFFSET(400)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn envv(root: &str, k: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

const CEX_WORDS: [&str; 18] = ["binance", "coinbase", "okx", "kraken", "bybit", "kucoin",
    "gate", "bitfinex", "huobi", "mexc", "crypto.com", "bitget", "upbit", "bithumb",
    "gemini", "bitstamp", "hot wallet", "deposit"];
const DEX_WORDS: [&str; 12] = ["uniswap", "sushi", "curve", "balancer", "1inch", "router",
    "pool", "pancake", "0x:", "paraswap", "cow protocol", "dodo"];
const OTC_WORDS: [&str; 10] = ["otc", "genesis trading", "wintermute", "cumberland", "jump trading",
    "alameda", "blockfi", "galaxy digital", "falconx", "amber group"];
const BRIDGE_WORDS: [&str; 8] = ["across", "hop protocol", "stargate", "celer", "relay",
    "wormhole", "synapse", "bridge"];

fn classify(a: &str, labels: &HashMap<String, String>) -> &'static str {
    if a.starts_with("0x000000000000000000000000000000000000000") { return "BURN" }
    if a == wr_store::addrs::weth() { return "WRAP" }
    if let Some(l) = labels.get(a) {
        if l.contains("null address") { return "BURN" }
        if l.contains("wrapped ether") || l.contains("wrapped-token") { return "WRAP" }
        if DEX_WORDS.iter().any(|w| l.contains(w)) { return "DEX" }
        if BRIDGE_WORDS.iter().any(|w| l.contains(w)) { return "BRIDGE" }
        if OTC_WORDS.iter().any(|w| l.contains(w)) { return "OTC" }
        if CEX_WORDS.iter().any(|w| l.contains(w)) { return "CEX" }
    }
    "EOA/інше"
}

async fn es(c: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Option<Vec<Value>> {
    for attempt in 0..6u64 {
        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
        let url = format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}");
        if let Ok(r) = c.get(&url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                let msg = v["message"].as_str().unwrap_or("");
                let s = v["result"].as_str().unwrap_or("");
                let limited = msg == "NOTOK" || s.contains("rate limit")
                    || s.contains("Max calls") || s.contains("Too many");
                if !limited {
                    if let Some(a) = v["result"].as_array() { return Some(a.clone()) }
                    if msg == "No transactions found" { return Some(Vec::new()) }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << attempt.min(5)))).await;
    }
    None
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let afile = std::env::args().nth(2).expect("вкажи файл з адресами");
    let offset: u32 = envn("OFFSET", 400);

    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());

    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).unwrap())).unwrap();
    let eth_usd = cur["eth"].as_f64().unwrap_or(0.0);
    let mut price: HashMap<String, f64> = HashMap::new();
    if let Some(o) = cur["tokens"].as_object() {
        for (c, v) in o { price.insert(c.to_lowercase(), v["usd"].as_f64().unwrap_or(0.0)); }
    }
    let mut labels: HashMap<String, String> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/labels-cache.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m {
                let mut s = v["name"].as_str().unwrap_or("").to_string();
                if let Some(arr) = v["labels"].as_array() {
                    for x in arr { if let Some(t) = x.as_str() { s.push(' '); s.push_str(t) } }
                }
                if !s.trim().is_empty() { labels.insert(a.to_lowercase(), s.to_lowercase()); }
            }
        }
    }

    let addrs: Vec<String> = BufReader::new(File::open(&afile).unwrap()).lines()
        .map_while(Result::ok).map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42).collect();
    eprintln!("[trace2] адрес першого рівня: {}", addrs.len());

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();
    let kc = AtomicUsize::new(0);
    let mut cat: HashMap<&'static str, (f64, u64)> = HashMap::new();
    let mut per_addr: Vec<Value> = Vec::new();
    let mut dest_top: HashMap<String, f64> = HashMap::new();

    for (i, a) in addrs.iter().enumerate() {
        let txs = es(&client, &keys, &kc,
            &format!("module=account&action=txlist&address={a}&page=1&offset={offset}&sort=desc")).await
            .unwrap_or_default();
        let toks = es(&client, &keys, &kc,
            &format!("module=account&action=tokentx&address={a}&page=1&offset={offset}&sort=desc")).await
            .unwrap_or_default();

        let mut mine: HashMap<&'static str, f64> = HashMap::new();
        for t in &txs {
            if t["from"].as_str().unwrap_or("").to_lowercase() != *a { continue }
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            if to.is_empty() { continue }
            let v: f64 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0) / 1e18;
            let usd = v * eth_usd;
            if usd <= 0.0 { continue }
            let c = classify(&to, &labels);
            *mine.entry(c).or_insert(0.0) += usd;
            *cat.entry(c).or_insert((0.0, 0)) = {
                let e = cat.get(c).copied().unwrap_or((0.0, 0));
                (e.0 + usd, e.1 + 1)
            };
            *dest_top.entry(to).or_insert(0.0) += usd;
        }
        for t in &toks {
            if t["from"].as_str().unwrap_or("").to_lowercase() != *a { continue }
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            if to.is_empty() { continue }
            let cc = t["contractAddress"].as_str().unwrap_or("").to_lowercase();
            let dec: i32 = t["tokenDecimal"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            let amt: f64 = t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
                / 10f64.powi(dec);
            let usd = amt * price.get(&cc).copied().unwrap_or(0.0);
            if usd <= 0.0 { continue }
            let c = classify(&to, &labels);
            *mine.entry(c).or_insert(0.0) += usd;
            let e = cat.get(c).copied().unwrap_or((0.0, 0));
            cat.insert(c, (e.0 + usd, e.1 + 1));
            *dest_top.entry(to).or_insert(0.0) += usd;
        }
        let tot: f64 = mine.values().sum();
        let to_cex = mine.get("CEX").copied().unwrap_or(0.0) + mine.get("OTC").copied().unwrap_or(0.0);
        per_addr.push(json!({"addr": a, "totalOutUsd": tot,
            "toCexOtcUsd": to_cex,
            "cexPct": if tot > 0.0 { 100.0 * to_cex / tot } else { 0.0 },
            "byCat": mine.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_,_>>()}));
        if (i + 1) % 10 == 0 { eprintln!("[trace2] {}/{}", i + 1, addrs.len()) }
    }

    let total: f64 = cat.values().map(|v| v.0).sum();
    let mut cs: Vec<(&&str, &(f64, u64))> = cat.iter().collect();
    cs.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap());

    eprintln!("\n=== ДРУГИЙ ХОП: куди шлють далі ці EOA ===");
    eprintln!("{:<10} {:>18} {:>8} {:>10}", "КУДИ", "СУМА $", "ЧАСТКА", "ОПЕРАЦІЙ");
    for (c, v) in &cs {
        eprintln!("{:<10} {:>18.0} {:>7.1}% {:>10}", c, v.0, 100.0 * v.0 / total.max(1e-9), v.1);
    }
    let cex = cat.get("CEX").map(|v| v.0).unwrap_or(0.0) + cat.get("OTC").map(|v| v.0).unwrap_or(0.0);
    eprintln!("\nНА БІРЖІ ТА OTC на другому хопі: ${:.0} = {:.1}% від виходів цих адрес",
        cex, 100.0 * cex / total.max(1e-9));

    per_addr.sort_by(|a, b| b["toCexOtcUsd"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["toCexOtcUsd"].as_f64().unwrap_or(0.0)).unwrap());
    eprintln!("\n=== ХТО З НИХ НАЙБІЛЬШЕ ШЛЕ НА БІРЖІ ===");
    for r in per_addr.iter().take(12) {
        eprintln!("  {} ${:>13.0} на біржі ({:.0}% його виходів)",
            r["addr"].as_str().unwrap_or(""), r["toCexOtcUsd"].as_f64().unwrap_or(0.0),
            r["cexPct"].as_f64().unwrap_or(0.0));
    }

    let mut ds: Vec<(&String, &f64)> = dest_top.iter().collect();
    ds.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    eprintln!("\n=== ТОП АДРЕС ДРУГОГО РІВНЯ ===");
    for (a, v) in ds.iter().take(12) {
        eprintln!("  ${:>13.0} · {} · {} {}", v, classify(a, &labels), &a[..12],
            labels.get(*a).map(|s| s.chars().take(30).collect::<String>()).unwrap_or_default());
    }

    let outp = format!("{bf}/trace2.json");
    let _ = File::create(&outp).unwrap().write_all(serde_json::to_string(&json!({
        "totalUsd": total, "cexOtcUsd": cex,
        "byCategory": cs.iter().map(|(c, v)| json!({"cat": c, "usd": v.0, "ops": v.1})).collect::<Vec<_>>(),
        "perAddr": per_addr})).unwrap().as_bytes());
    eprintln!("\n[trace2] збережено {outp}");
}
