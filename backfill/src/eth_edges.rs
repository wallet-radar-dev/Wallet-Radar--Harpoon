//! eth_edges — докраулює нативні ETH-РЕБРА для гаманців мережі: для кожного тягне txlist+txlistinternal
//! і збирає ВСІХ контрагентів (from/to, value>0) = ребра EOA-графа (доповнює розріджений бекфіл, щоб
//! хоп-звʼязність була повна). Etherscan V2 chainid=1, ротація BACKFILL_ETHERSCAN_KEYS. Резюмований.
//! Вихід: dataset/backfill/eth-edges.jsonl {"w":addr,"cps":[контрагенти]}. Args: ROOT WALLET_FILE. Env: CONC(10) OFFSET(3000).
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
            if let Ok(v) = r.json::<Value>().await { if v["result"].is_array() || v["message"] == "No transactions found" { return v; } }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    json!({"result": []})
}
async fn cps(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, w: &str, action: &str, offset: u32, acc: &mut HashSet<String>) {
    let v = es(client, keys, kc, &format!("module=account&action={action}&address={w}&startblock=0&endblock=99999999&page=1&offset={offset}&sort=asc")).await;
    if let Some(res) = v["result"].as_array() {
        for t in res {
            if t["isError"].as_str() == Some("1") { continue; }
            let val: u128 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0); if val == 0 { continue; }
            let from = t["from"].as_str().unwrap_or("").to_lowercase();
            let to = t["to"].as_str().unwrap_or("").to_lowercase();
            let cp = if from == w { to } else if to == w { from } else { continue };
            if cp.len() == 42 && cp != *w && cp != wr_store::addrs::zero() { acc.insert(cp); }
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{root}/dataset/backfill/cap-members.txt"));
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let offset: u32 = std::env::var("OFFSET").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);

    let keys: Vec<String> = fs::read_to_string(format!("{root}/.env")).unwrap_or_default()
        .lines().find(|l| l.starts_with("BACKFILL_ETHERSCAN_KEYS="))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect()).unwrap_or_default();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");
    let wallets: Vec<String> = fs::read_to_string(&wfile).expect("wallet file").lines().map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && l.starts_with("0x")).collect();
    let outp = format!("{root}/dataset/backfill/eth-edges.jsonl");
    let mut done: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(&outp) { for line in BufReader::new(f).lines().map_while(Result::ok) { if let Ok(v) = serde_json::from_str::<Value>(&line) { if let Some(w) = v["w"].as_str() { done.insert(w.to_string()); } } } }
    let todo: Vec<String> = wallets.iter().filter(|w| !done.contains(*w)).cloned().collect();
    eprintln!("[eth_edges] гаманців: {}, зроблено: {}, до краулу: {} (ключів {}, conc {}, offset {})", wallets.len(), done.len(), todo.len(), keys.len(), conc, offset);

    let keys = Arc::new(keys); let kc = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-edges/0.1").pool_max_idle_per_host(conc).timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let sem = Arc::new(Semaphore::new(conc));
    let out = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&outp).unwrap()));
    let progress = Arc::new(AtomicUsize::new(0)); let total = todo.len();
    let mut handles = Vec::new();
    for w in todo {
        let (keys, kc, client, sem, out, progress) = (keys.clone(), kc.clone(), client.clone(), sem.clone(), out.clone(), progress.clone());
        handles.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut acc: HashSet<String> = HashSet::new();
            cps(&client, &keys, &kc, &w, "txlist", offset, &mut acc).await;
            cps(&client, &keys, &kc, &w, "txlistinternal", offset, &mut acc).await;
            let rec = json!({"w": w, "cps": acc.into_iter().collect::<Vec<_>>()});
            { let mut g = out.lock().unwrap(); writeln!(g, "{}", rec).ok(); }
            let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 100 == 0 { eprintln!("[eth_edges] {n}/{total}"); }
        }));
    }
    for h in handles { let _ = h.await; }
    eprintln!("[eth_edges] ГОТОВО → eth-edges.jsonl");
}
