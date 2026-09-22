//! Rust-порт `deploy/eth-native.py`. Нативний ETH-збір: Etherscan V2 txlist+txlistinternal (value
//! в wei) для EVM-китів з STARTDATE. 8-ключова ротація, keep-alive. Рахує net = отримано−віддано
//! ETH з STARTDATE. Вихід dataset/backfill/eth-native-net.json. Env: STARTDATE(2025-09-01) LIMIT OUT.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};

fn es_url(keys: &[String], kc: &AtomicUsize, params: &str) -> String {
    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
    format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")
}

async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Value {
    for _ in 0..4 {
        if let Ok(r) = client.get(es_url(keys, kc, params)).send().await {
            if let Ok(v) = r.json::<Value>().await {
                return v;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    json!({"result": []})
}

async fn startblock(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, start: &str) -> u64 {
    let ts = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0);
    let v = es(client, keys, kc, &format!("module=block&action=getblocknobytime&timestamp={ts}&closest=before")).await;
    v["result"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0)
}

async fn flow(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str, action: &str, sb: u64) -> i128 {
    let w = addr.to_lowercase();
    let mut net: i128 = 0;
    let mut page_sb = sb;
    for _ in 0..200 {
        let v = es(client, keys, kc, &format!(
            "module=account&action={action}&address={addr}&startblock={page_sb}&endblock=99999999&page=1&offset=1000&sort=asc"
        )).await;
        let Some(res) = v["result"].as_array() else { break };
        if res.is_empty() {
            break;
        }
        for t in res {
            if t["isError"].as_str() == Some("1") {
                continue;
            }
            let value: u128 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
            if value == 0 {
                continue;
            }
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            let from = t["from"].as_str().unwrap_or("").to_lowercase();
            if to == w {
                net += value as i128;
            } else if from == w {
                net -= value as i128;
            }
        }
        if res.len() < 1000 {
            break;
        }
        let nb: u64 = res.last().and_then(|t| t["blockNumber"].as_str()).and_then(|s| s.parse().ok()).unwrap_or(0);
        if nb <= page_sb {
            break;
        }
        page_sb = nb;
    }
    net
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let start = std::env::var("STARTDATE").unwrap_or_else(|_| "2025-09-01".into());
    let limit: usize = std::env::var("LIMIT").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let out = std::env::var("OUT").unwrap_or_else(|_| format!("{root}/dataset/backfill/eth-native-net.json"));

    let keys: Vec<String> = fs::read_to_string(format!("{root}/.env"))
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("BACKFILL_ETHERSCAN_KEYS="))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect())
        .unwrap_or_default();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-ethnative/0.1")
        .pool_max_idle_per_host(8)
        .build()
        .unwrap();

    let sb = startblock(&client, &keys, &kc, &start).await;
    eprintln!("startblock ({start}): {sb}");
    if sb == 0 {
        eprintln!("не отримав startblock");
        std::process::exit(1);
    }
    let mut whales: Vec<String> = BufReader::new(File::open(format!("{root}/dataset/backfill/backfill-wallets.txt")).expect("wallets"))
        .lines()
        .map_while(Result::ok)
        .map(|l| l.trim().to_string())
        .filter(|l| l.to_lowercase().starts_with("0x") && l.len() == 42)
        .collect();
    if limit > 0 && whales.len() > limit {
        whales.truncate(limit);
    }
    eprintln!("китів: {}", whales.len());
    let mut net: HashMap<String, f64> = HashMap::new();
    for (i, w) in whales.iter().enumerate() {
        let n = flow(&client, &keys, &kc, w, "txlist", sb).await + flow(&client, &keys, &kc, w, "txlistinternal", sb).await;
        let eth = n as f64 / 1e18;
        if eth.abs() > 0.01 {
            net.insert(w.clone(), (eth * 10000.0).round() / 10000.0);
        }
        if i % 200 == 0 {
            fs::write(&out, serde_json::to_string(&net).unwrap()).ok();
            eprintln!("[{i}/{}] зібрано ненульових: {}", whales.len(), net.len());
        }
    }
    fs::write(&out, serde_json::to_string(&net).unwrap()).ok();
    let mut top: Vec<(&String, &f64)> = net.iter().collect();
    top.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    eprintln!("=== ТОП native-ETH накопичувачі з {start} ===");
    for (w, n) in top.iter().take(20) {
        println!("  +{:>14.1} ETH  {}", n, w);
    }
    eprintln!("готово: {} китів з ненульовим native-ETH -> {out}", net.len());
}
