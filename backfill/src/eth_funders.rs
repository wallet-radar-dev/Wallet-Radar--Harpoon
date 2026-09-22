//! eth_funders — для кожного гаманця зі списку тягне НАЙРАНІШИЙ вхідний нативний ETH (і txlist, і
//! txlistinternal → вибирає ранішій за timeStamp) = перший фандер (100% покриття, не 2% з бекфілу).
//! Etherscan V2 chainid=1, ротація BACKFILL_ETHERSCAN_KEYS. Резюмований (пропускає вже зроблені).
//! Вихід: dataset/backfill/eth-funders.jsonl {"w","funder","ts","via"}. Args: ROOT WALLET_FILE.
//! Env: CONC(10).

use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn es_url(keys: &[String], kc: &AtomicUsize, params: &str) -> String {
    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
    format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")
}
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Value {
    for _ in 0..5 {
        if let Ok(r) = client.get(es_url(keys, kc, params)).send().await {
            if let Ok(v) = r.json::<Value>().await {
                // rate-limit ("Max calls per sec") -> ретрай
                if v["result"].is_array() || v["status"] == "1" || v["message"] == "No transactions found" { return v; }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    json!({"result": []})
}

/// Найраніший вхідний (to==wallet, value>0, !error) для дії action -> (funder, ts).
async fn earliest_inbound(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, w: &str, action: &str) -> Option<(String, u64)> {
    let v = es(client, keys, kc, &format!("module=account&action={action}&address={w}&startblock=0&endblock=99999999&page=1&offset=100&sort=asc")).await;
    let res = v["result"].as_array()?;
    for t in res {
        if t["isError"].as_str() == Some("1") { continue; }
        let value: u128 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        if value == 0 { continue; }
        let to = t["to"].as_str().unwrap_or("").to_lowercase();
        if to != w { continue; }
        let from = t["from"].as_str().unwrap_or("").to_lowercase();
        if from.is_empty() || from == wr_store::addrs::zero() { continue; }
        let ts: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        return Some((from, ts));
    }
    None
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{root}/dataset/backfill/cap-members.txt"));
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(10);

    let keys: Vec<String> = fs::read_to_string(format!("{root}/.env")).unwrap_or_default()
        .lines().find(|l| l.starts_with("BACKFILL_ETHERSCAN_KEYS="))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect())
        .unwrap_or_default();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS у .env");

    let wallets: Vec<String> = fs::read_to_string(&wfile).expect("wallet file")
        .lines().map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && l.starts_with("0x")).collect();
    // резюм: вже зроблені
    let outp = format!("{root}/dataset/backfill/eth-funders.jsonl");
    let mut done: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(&outp) { for line in BufReader::new(f).lines().map_while(Result::ok) { if let Ok(v) = serde_json::from_str::<Value>(&line) { if let Some(w) = v["w"].as_str() { done.insert(w.to_string()); } } } }
    let todo: Vec<String> = wallets.iter().filter(|w| !done.contains(*w)).cloned().collect();
    eprintln!("[eth_funders] гаманців: {}, вже зроблено: {}, до краулу: {} (ключів {}, conc {})", wallets.len(), done.len(), todo.len(), keys.len(), conc);

    let keys = Arc::new(keys);
    let kc = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-funders/0.1").pool_max_idle_per_host(conc).timeout(std::time::Duration::from_secs(30)).build().unwrap());
    let sem = Arc::new(Semaphore::new(conc));
    let out = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&outp).unwrap()));
    let progress = Arc::new(AtomicUsize::new(0));
    let total = todo.len();

    let mut handles = Vec::new();
    for w in todo {
        let (keys, kc, client, sem, out, progress) = (keys.clone(), kc.clone(), client.clone(), sem.clone(), out.clone(), progress.clone());
        handles.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let ext = earliest_inbound(&client, &keys, &kc, &w, "txlist").await;
            let int = earliest_inbound(&client, &keys, &kc, &w, "txlistinternal").await;
            let pick = match (ext, int) {
                (Some(e), Some(i)) => if i.1 < e.1 { Some((i.0, i.1, "internal")) } else { Some((e.0, e.1, "txlist")) },
                (Some(e), None) => Some((e.0, e.1, "txlist")),
                (None, Some(i)) => Some((i.0, i.1, "internal")),
                (None, None) => None,
            };
            let rec = match pick { Some((f, ts, via)) => json!({"w": w, "funder": f, "ts": ts, "via": via}), None => json!({"w": w, "funder": Value::Null, "ts": 0, "via": "none"}) };
            { let mut g = out.lock().unwrap(); writeln!(g, "{}", rec).ok(); }
            let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 100 == 0 { eprintln!("[eth_funders] {n}/{total}"); }
        }));
    }
    for h in handles { let _ = h.await; }
    eprintln!("[eth_funders] ГОТОВО. Вихід: eth-funders.jsonl");
}
