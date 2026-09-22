//! hubtrace — розкручує ETH-виходи хаба вглиб (BFS кілька хопів), щоб знайти КУДИ зрештою йдуть гроші:
//! мічені кінцівки (біржі / L2-мости) = точки виходу/кешауту; неміченні EOA — розгортаємо далі.
//! Reqwest+tokio, таймаути. Env: SEED(0x6fc48de8...) MAX_HOPS(3) MAX_NODES(200) MIN_EDGE(0.2) CONC(8).
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::BufReader;
use std::sync::atomic::{AtomicUsize, Ordering};


/// Перша адреса з тегу — замість зашитого значення за замовчуванням.
/// Порожній результат краще, ніж мовчки взята стара адреса: видно одразу.
fn default_tagged(tag: &str) -> String {
    let Ok(c) = wr_store::open() else { return String::new() };
    let Ok(mut q) =
        c.prepare("SELECT address FROM wallet_tags WHERE tag=?1 ORDER BY address LIMIT 1")
    else {
        return String::new();
    };
    q.query_row([tag], |r| r.get(0)).unwrap_or_default()
}

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Vec<Value> {
    for _ in 0..4 {
        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
        if let Ok(r) = client.get(format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")).send().await {
            if let Ok(v) = r.json::<Value>().await { if let Some(a) = v["result"].as_array() { return a.clone(); } }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    Vec::new()
}
/// ETH-виходи addr -> (to -> wei), лише value>0
async fn outflows(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str) -> HashMap<String, f64> {
    let txs = es(client, keys, kc, &format!("module=account&action=txlist&address={addr}&startblock=0&endblock=99999999&page=1&offset=1000&sort=asc")).await;
    let mut o: HashMap<String, f64> = HashMap::new();
    for t in &txs {
        if t["from"].as_str().unwrap_or("").to_lowercase() != addr { continue; }
        let v: u128 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0); if v == 0 { continue; }
        let to = t["to"].as_str().unwrap_or("").to_lowercase();
        if !to.is_empty() { *o.entry(to).or_insert(0.0) += v as f64 / 1e18; }
    }
    o
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let seed = std::env::var("SEED").unwrap_or_else(|_| default_tagged("cexsignal:infra")).to_lowercase();
    let max_hops: u32 = std::env::var("MAX_HOPS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let max_nodes: usize = std::env::var("MAX_NODES").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let min_edge: f64 = std::env::var("MIN_EDGE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.2);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap();
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(fs::File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let ex_terms = ["binance", "coinbase", "okx", "gate", "kraken", "bybit", "kucoin", "bitget", "huobi", "mexc", "bitfinex", "upbit", "bithumb", "uphold", "paribu", "btcturk", "bingx", "exchange", "hot wallet", "deposit"];
    let br_terms = ["bridge", "arbitrum", "optimism", "base:", "zksync", "polygon", "hop", "across", "orbiter", "stargate", "synapse", "celer", "wormhole", "l2", "gateway"];
    let classify = |a: &str| -> &'static str {
        if let Some(n) = lbl.get(a).and_then(|x| x["name"].as_str()) { let l = n.to_lowercase();
            if br_terms.iter().any(|t| l.contains(t)) { return "bridge"; }
            if ex_terms.iter().any(|t| l.contains(t)) { return "exchange"; }
        }
        "eoa"
    };
    let lname = |a: &str| lbl.get(a).and_then(|x| x["name"].as_str()).unwrap_or("").to_string();

    let mut q: VecDeque<(String, u32)> = VecDeque::new();
    q.push_back((seed.clone(), 0));
    let mut visited: HashSet<String> = HashSet::new(); visited.insert(seed.clone());
    let mut exch: HashMap<String, f64> = HashMap::new();   // біржа-мітка -> ETH
    let mut bridge: HashMap<String, f64> = HashMap::new();
    let mut fetched = 0; let mut internal = 0usize;
    while let Some((a, hop)) = q.pop_front() {
        if hop >= max_hops || internal >= max_nodes { continue; }
        let o = outflows(&client, &keys, &kc, &a).await; fetched += 1; internal += 1;
        for (to, eth) in &o {
            if *eth < min_edge || to == &a { continue; }
            match classify(to) {
                "exchange" => { *exch.entry(lname(to)).or_insert(0.0) += eth; }
                "bridge" => { *bridge.entry(lname(to)).or_insert(0.0) += eth; }
                _ => { if !visited.contains(to) && visited.len() < max_nodes * 3 { visited.insert(to.clone()); q.push_back((to.clone(), hop + 1)); } }
            }
        }
        if fetched % 20 == 0 { eprintln!("[hubtrace] fetched={fetched} internal={internal} q={} біржі={} мости={}", q.len(), exch.len(), bridge.len()); }
    }
    let total_ex: f64 = exch.values().sum(); let total_br: f64 = bridge.values().sum();
    eprintln!("[hubtrace] ===== РОЗКРУТ ХАБА {} =====", seed);
    eprintln!("[hubtrace] вузлів обійдено: {}, EOA-мережа: {}", fetched, internal);
    eprintln!("[hubtrace] --- ВИХІД НА БІРЖІ (кешаут): {:.1} ETH ---", total_ex);
    let mut ev: Vec<(&String, &f64)> = exch.iter().collect(); ev.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    for (n, e) in ev.iter().take(12) { eprintln!("     {:>10.1} ETH → {}", e, n); }
    eprintln!("[hubtrace] --- ВИХІД НА L2-МОСТИ: {:.1} ETH ---", total_br);
    let mut bv: Vec<(&String, &f64)> = bridge.iter().collect(); bv.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    for (n, e) in bv.iter().take(8) { eprintln!("     {:>10.1} ETH → {}", e, n); }
    fs::write(format!("{bf}/hubtrace.json"), serde_json::to_string_pretty(&json!({"seed": seed, "hops": max_hops, "nodes": internal, "exchanges": exch, "bridges": bridge})).unwrap()).ok();
    eprintln!("[hubtrace] збережено hubtrace.json");
}
