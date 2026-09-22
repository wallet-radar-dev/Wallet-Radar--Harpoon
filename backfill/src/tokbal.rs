//! tokbal — РЕАЛЬНІ баланси токенів (Blockscout `/api/v2/addresses/{a}/token-balances`).
//! Причина: досі токенні позиції були flow-net реконструкцією з бекфілу (скільки ПРОЙШЛО), а не
//! залишком — та сама помилка, що з нативним ETH, де оцінка $1.16млрд обернулась балансом $4.
//! Blockscout віддає ВСІ токени гаманця одним запитом і без ключа, тож 6k гаманців = 6k запитів.
//! Вихід: dataset/backfill/token-balances.jsonl {"w":addr,"toks":[{"c":контракт,"sym":..,"amt":f64}]}
//! Резюмований (`token-balances.done`). Args: ROOT WALLET_FILE. Env: CONC(6) API(https://eth.blockscout.com) MIN_USD(50)
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{root}/dataset/backfill/net-members.txt"));
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
    let api = std::env::var("API").unwrap_or_else(|_| "https://eth.blockscout.com".into());
    let bf = format!("{root}/dataset/backfill");

    let wallets: Vec<String> = fs::read_to_string(&wfile).expect("wallet file").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && l.starts_with("0x")).collect();
    let outp = format!("{bf}/token-balances.jsonl");
    let donep = format!("{bf}/token-balances.done");
    let mut done: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(&donep) { for l in BufReader::new(f).lines().map_while(Result::ok) { done.insert(l.trim().to_string()); } }
    let todo: Vec<String> = wallets.iter().filter(|w| !done.contains(*w)).cloned().collect();
    eprintln!("[tokbal] гаманців {} · зроблено {} · до краулу {} (conc {conc})", wallets.len(), done.len(), todo.len());

    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-tokbal/0.1").timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let sem = Arc::new(Semaphore::new(conc));
    let out = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&outp).unwrap()));
    let donef = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&donep).unwrap()));
    let prog = Arc::new(AtomicUsize::new(0));
    let hits = Arc::new(AtomicUsize::new(0));
    let total = todo.len();
    let api = Arc::new(api);

    let mut hs = Vec::new();
    for w in todo {
        let (client, sem, out, donef, prog, hits, api) = (client.clone(), sem.clone(), out.clone(), donef.clone(), prog.clone(), hits.clone(), api.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut toks: Vec<Value> = Vec::new();
            for _ in 0..3 {
                let url = format!("{api}/api/v2/addresses/{w}/token-balances");
                match client.get(&url).send().await {
                    Ok(r) => {
                        let ok = r.status().is_success();
                        if let Ok(v) = r.json::<Value>().await {
                            if let Some(arr) = v.as_array() {
                                for t in arr {
                                    let dec: i32 = t["token"]["decimals"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
                                    let raw: f64 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                                    let amt = raw / 10f64.powi(dec);
                                    if amt <= 0.0 { continue; }
                                    toks.push(json!({"c": t["token"]["address"].as_str().or(t["token"]["address_hash"].as_str()).unwrap_or("").to_lowercase(),
                                                     "sym": t["token"]["symbol"].as_str().unwrap_or(""), "amt": amt}));
                                }
                                break;
                            }
                            if ok { break; }        // порожня відповідь = гаманець без токенів
                        }
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            }
            if !toks.is_empty() {
                hits.fetch_add(1, Ordering::Relaxed);
                let mut o = out.lock().unwrap();
                let _ = writeln!(o, "{}", json!({"w": w, "toks": toks}));
            }
            { let mut d = donef.lock().unwrap(); let _ = writeln!(d, "{w}"); }
            let n = prog.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 250 == 0 || n == total { eprintln!("[tokbal] {n}/{total} · з токенами {}", hits.load(Ordering::Relaxed)); }
        }));
    }
    for h in hs { let _ = h.await; }
    eprintln!("[tokbal] ГОТОВО: {total}, з токенами {}", hits.load(Ordering::Relaxed));
}
