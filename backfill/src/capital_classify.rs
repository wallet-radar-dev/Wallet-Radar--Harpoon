//! capital_classify: класифікує гаманці з wallet-capital.jsonl на kind ∈
//!   otc / exchange / contract / whale — щоб почистити верхівку від хабів
//!   (біржі/мости/контракти, де flow-net роздуває фантомний капітал).
//! Метод: наявні мітки (network-labels.json) -> eth_getCode (контракт?) ->
//!   nonce>=NONCE_CEX (біржовий EOA, як у desk_flow) -> інакше whale.
//! Класифікує лише гаманці з usd>=MIN_USD (дефолт 10000 -> тіри mega+large+mid).
//! Вихід: wallet-capital-classified.jsonl (класифікований підмнож., +kind/+label)
//!        + real-whales.jsonl (kind==whale, переранжовано). Etherscan V2 chainid=1.
//! Env: MIN_USD(10000) NONCE_CEX(200000) CONCURRENCY(16).

use futures::stream::{self, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

fn es_url(keys: &[String], kc: &AtomicUsize, params: &str) -> String {
    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
    format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")
}
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Value {
    for _ in 0..5 {
        if let Ok(r) = client.get(es_url(keys, kc, params)).send().await {
            if let Ok(v) = r.json::<Value>().await {
                return v;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    json!({ "result": "" })
}
async fn nonce(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str) -> u64 {
    let v = es(client, keys, kc, &format!("module=proxy&action=eth_getTransactionCount&address={addr}&tag=latest")).await;
    v["result"].as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(0)
}
async fn is_contract(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str) -> bool {
    let v = es(client, keys, kc, &format!("module=proxy&action=eth_getCode&address={addr}&tag=latest")).await;
    v["result"].as_str().map(|s| s.len() > 2 && s != "0x").unwrap_or(false)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(10_000.0);
    let nonce_cex: u64 = std::env::var("NONCE_CEX").ok().and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let conc: usize = std::env::var("CONCURRENCY").ok().and_then(|s| s.parse().ok()).unwrap_or(16);
    // custody-евристика (підозріло-кастодні кити: чистий ETH + низька активність/диверсифікація)
    let eth_frac: f64 = std::env::var("CUSTODY_ETH_FRAC").ok().and_then(|s| s.parse().ok()).unwrap_or(0.95);
    let max_tok: u64 = std::env::var("CUSTODY_MAXTOK").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let max_ch: u64 = std::env::var("CUSTODY_MAXCHAINS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let max_tx: u64 = std::env::var("CUSTODY_MAXTX").ok().and_then(|s| s.parse().ok()).unwrap_or(30);

    // 1) відомі мітки -> addr -> (kind, label)
    let mut known: HashMap<String, (&'static str, String)> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(
        File::open(format!("{root}/dataset/backfill/network-labels.json")).expect("network-labels.json"),
    )) {
        for (sect, kind) in [("hubs_otc", "otc"), ("exchange_sinks", "exchange")] {
            if let Some(m) = v.get(sect).and_then(|x| x.as_object()) {
                for (a, lab) in m {
                    known.insert(a.to_lowercase(), (kind, lab.as_str().unwrap_or("").to_string()));
                }
            }
        }
    }
    eprintln!("[classify] відомих міток: {}", known.len());

    // 2) прочитати wallet-capital.jsonl (усі рядки збережемо; класифікуємо usd>=min_usd)
    let jin = format!("{root}/dataset/backfill/wallet-capital.jsonl");
    let mut all: Vec<Value> = Vec::new();
    for l in BufReader::new(File::open(&jin).expect("wallet-capital.jsonl")).lines().map_while(Result::ok) {
        if let Ok(v) = serde_json::from_str::<Value>(&l) {
            all.push(v);
        }
    }
    // targets: ADDR_FILE (довільний список) АБО usd>=MIN_USD із capital
    let targets: Vec<String> = if let Ok(af) = std::env::var("ADDR_FILE") {
        let mut v: Vec<String> = Vec::new();
        if let Ok(f) = File::open(&af) {
            for l in BufReader::new(f).lines().map_while(Result::ok) {
                let a = l.trim().to_lowercase();
                if a.starts_with("0x") && a.len() == 42 { v.push(a); }
            }
        }
        eprintln!("[classify] ADDR_FILE={af}: {} адрес", v.len());
        v
    } else {
        all.iter().filter(|r| r["usd"].as_f64().unwrap_or(0.0) >= min_usd)
            .filter_map(|r| r["w"].as_str().map(|s| s.to_string())).collect()
    };
    eprintln!("[classify] усього рядків: {}; до класифікації: {}", all.len(), targets.len());

    // 2b) enrichment для custody-евристики: addr -> (n_tx, n_tokens, n_chains)
    let mut enr: HashMap<String, (u64, u64, u64)> = HashMap::new();
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/wallet-enrichment.jsonl")) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                if let Some(w) = v["w"].as_str() {
                    enr.insert(
                        w.to_lowercase(),
                        (v["n_tx"].as_u64().unwrap_or(0), v["n_tokens"].as_u64().unwrap_or(0), v["n_chains"].as_u64().unwrap_or(0)),
                    );
                }
            }
        }
    }
    eprintln!("[classify] enrichment-записів: {}", enr.len());

    // 3) ключі + клієнт
    let keys: Vec<String> = fs::read_to_string(format!("{root}/.env")).unwrap_or_default()
        .lines().find(|l| l.starts_with("BACKFILL_ETHERSCAN_KEYS="))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect())
        .unwrap_or_default();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS у .env");
    eprintln!("[classify] ключів: {}; concurrency: {}", keys.len(), conc);
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-classify/0.1")
        .pool_max_idle_per_host(conc).pool_idle_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(20))
        .build().unwrap();

    // 4) класифікувати конкурентно
    let done = AtomicUsize::new(0);
    let total = targets.len();
    let results: HashMap<String, (String, String)> = stream::iter(targets.into_iter())
        .map(|addr| {
            let client = &client; let keys = &keys; let kc = &kc; let known = &known; let done = &done;
            async move {
                let lw = addr.to_lowercase();
                let (kind, label) = if let Some((k, lab)) = known.get(&lw) {
                    (k.to_string(), lab.clone())
                } else if is_contract(client, keys, kc, &lw).await {
                    ("contract".to_string(), String::new())
                } else if nonce(client, keys, kc, &lw).await >= nonce_cex {
                    ("exchange".to_string(), String::new())
                } else {
                    ("whale".to_string(), String::new())
                };
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 200 == 0 { eprintln!("[classify] {n}/{total}"); }
                (lw, (kind, label))
            }
        })
        .buffer_unordered(conc)
        .collect()
        .await;
    eprintln!("[classify] класифіковано: {}", results.len());

    // кумулятивний кеш wallet-kind.json (addr -> kind), мерджимо з наявним
    let kind_path = format!("{root}/dataset/backfill/wallet-kind.json");
    let mut kind_cache: serde_json::Map<String, Value> = File::open(&kind_path).ok()
        .and_then(|f| serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)).ok())
        .unwrap_or_default();
    for (a, (k, _)) in &results {
        kind_cache.insert(a.clone(), json!(k));
    }
    fs::write(&kind_path, serde_json::to_string(&Value::Object(kind_cache.clone())).unwrap()).ok();
    eprintln!("[classify] wallet-kind.json: {} записів", kind_cache.len());

    // 5) записати класифікований підмножинний файл + real-whales
    // (ADDR_FILE-режим лише оновлює kind-кеш, аби не затерти capital-виходи)
    let sfx = if std::env::var("ADDR_FILE").is_ok() { ".holders" } else { "" };
    let mut kinds: HashMap<String, u64> = HashMap::new();
    let cls_path = format!("{root}/dataset/backfill/wallet-capital-classified{sfx}.jsonl");
    let rw_path = format!("{root}/dataset/backfill/real-whales{sfx}.jsonl");
    let mut cw = BufWriter::new(File::create(&cls_path).unwrap());
    let mut whales: Vec<Value> = Vec::new();

    for r in &all {
        let w = r["w"].as_str().unwrap_or("").to_lowercase();
        if let Some((kind, label)) = results.get(&w) {
            *kinds.entry(kind.clone()).or_insert(0) += 1;
            let mut o = r.clone();
            o["kind"] = json!(kind);
            if !label.is_empty() { o["label"] = json!(label); }
            if kind == "whale" {
                let usd = o["usd"].as_f64().unwrap_or(0.0);
                let eth_usd = o["eth_usd"].as_f64().unwrap_or(0.0);
                let pure_eth = usd > 0.0 && eth_usd / usd >= eth_frac;
                let e = enr.get(&w);
                let no_erc20 = e.is_none();
                let (ntx, ntok, nch) = e.copied().unwrap_or((0, 0, 0));
                let low_div = ntok <= max_tok && nch <= max_ch;
                let low_act = ntx <= max_tx;
                let mut reasons: Vec<&str> = Vec::new();
                if pure_eth { reasons.push("pure_eth"); }
                if no_erc20 { reasons.push("no_erc20"); }
                if low_div { reasons.push("low_diversity"); }
                if low_act { reasons.push("low_activity"); }
                let custody = pure_eth && (no_erc20 || low_div || low_act);
                o["custody_suspect"] = json!(custody);
                o["flags"] = json!(reasons);
                o["e_ntx"] = json!(ntx);
                o["e_ntok"] = json!(ntok);
                o["e_nchains"] = json!(nch);
                if custody {
                    *kinds.entry("whale_custody?".to_string()).or_insert(0) += 1;
                }
                whales.push(o.clone());
            }
            writeln!(cw, "{o}").ok();
        }
    }
    whales.sort_by(|a, b| b["usd"].as_f64().unwrap_or(0.0).partial_cmp(&a["usd"].as_f64().unwrap_or(0.0)).unwrap());
    let pw_path = format!("{root}/dataset/backfill/private-whales{sfx}.jsonl");
    let mut rw = BufWriter::new(File::create(&rw_path).unwrap());
    let mut pw = BufWriter::new(File::create(&pw_path).unwrap());
    for (i, w) in whales.iter_mut().enumerate() {
        w["whale_rank"] = json!(i + 1);
        writeln!(rw, "{w}").ok();
        if !w["custody_suspect"].as_bool().unwrap_or(false) {
            writeln!(pw, "{w}").ok();
        }
    }

    // 6) звіт
    let g = |k: &str| kinds.get(k).copied().unwrap_or(0);
    let custody = g("whale_custody?");
    let private = g("whale").saturating_sub(custody);
    eprintln!("=== KIND (серед класифікованих) ===");
    eprintln!("  whale={} (private={} custody?={}) contract={} exchange={} otc={}",
        g("whale"), private, custody, g("contract"), g("exchange"), g("otc"));
    let removed = g("contract") + g("exchange") + g("otc");
    eprintln!("прибрано хабів (не-whale): {removed}");
    eprintln!("=== ТОП-25 РЕАЛЬНИХ КИТІВ (kind=whale; [C?]=підозра кастоді) ===");
    for w in whales.iter().take(25) {
        let c = if w["custody_suspect"].as_bool().unwrap_or(false) { "[C?]" } else { "    " };
        eprintln!("  #{:<4}{} ${:>14}  ethNet={:>10.0}  n={:<3} {}",
            w["whale_rank"].as_u64().unwrap_or(0), c,
            fmt(w["usd"].as_f64().unwrap_or(0.0)),
            w["eth_net"].as_f64().unwrap_or(0.0),
            w["n_tok"].as_u64().unwrap_or(0),
            w["w"].as_str().unwrap_or(""));
    }
    eprintln!("вихід: {cls_path} | {rw_path} | {pw_path}");
    // summary json
    let sp = format!("{root}/dataset/backfill/capital-classify-summary.json");
    fs::write(&sp, serde_json::to_string_pretty(&json!({
        "classified": results.len(), "min_usd": min_usd, "nonce_cex": nonce_cex,
        "kinds": {"whale": g("whale"), "whale_private": private, "whale_custody_suspect": custody,
                  "contract": g("contract"), "exchange": g("exchange"), "otc": g("otc")},
        "hubs_removed": removed, "real_whales": whales.len(),
        "custody_heuristic": {"eth_frac": eth_frac, "max_tok": max_tok, "max_chains": max_ch, "max_tx": max_tx},
        "note": "kind на ethereum mainnet (getCode/nonce); хаб на L2 але EOA на mainnet міг лишитись whale; custody_suspect=евристика (pure_eth + low activity/diversity), не підтверджена мітка"
    })).unwrap()).ok();
}

fn fmt(x: f64) -> String {
    let s = format!("{:.0}", x);
    let b = s.as_bytes();
    let mut out = String::new();
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 { out.push(','); }
        out.push(*c as char);
    }
    out
}
