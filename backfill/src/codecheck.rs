//! codecheck — визначає, які адреси є КОНТРАКТАМИ, для довільного списку.
//!
//! Навіщо окремий бін: перевірка коду в `ethbal` робилась лише для адрес із балансом ≥5 ETH,
//! тож контракти з нульовим балансом лишались у даних як «звичайні гаманці». Через це детектор
//! OTC подав контракт `0xfd3dfb52…` (код 0x6080…, 0 ETH) як «OTC-службу з 60 контрагентами».
//! Мітки й wallet-kind таких адрес теж не знають — єдина надійна ознака це `eth_getCode`.
//!
//! Вихід: dataset/backfill/contract-flags.json {addr: true|false} — накопичувальний, не перезаписує
//! попередні перевірки. Args: ROOT WALLET_FILE. Env: CONC(10) CHAIN(1)
use backfill::patterns::{Finding, PatternDb};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wfile = std::env::args().nth(2).expect("потрібен файл зі списком адрес");
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());

    let path = format!("{bf}/contract-flags.json");
    let known: HashMap<String, bool> = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let want: Vec<String> = fs::read_to_string(&wfile).expect("wallet file").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && l.starts_with("0x"))
        .filter(|a| !known.contains_key(a)).collect();
    eprintln!("[codecheck] у списку нових {} · уже відомо {}", want.len(), known.len());

    let client = Arc::new(reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap());
    let keys = Arc::new(keys); let kc = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(conc));
    let res: Arc<Mutex<HashMap<String, bool>>> = Arc::new(Mutex::new(known));
    let prog = Arc::new(AtomicUsize::new(0));
    let hits = Arc::new(AtomicUsize::new(0));
    let total = want.len();

    let mut hs = Vec::new();
    for a in want {
        let (client, keys, kc, sem, res, prog, hits) = (client.clone(), keys.clone(), kc.clone(), sem.clone(), res.clone(), prog.clone(), hits.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            for _ in 0..4 {
                let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                let url = format!("https://api.etherscan.io/v2/api?chainid={chain}&module=proxy&action=eth_getCode&address={a}&tag=latest&apikey={key}");
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        if let Some(code) = v["result"].as_str() {
                            // при ліміті Etherscan кладе у result ТЕКСТ помилки — валідна відповідь
                            // завжди починається з 0x, інакше це не «не контракт», а невдача
                            if !code.starts_with("0x") { tokio::time::sleep(std::time::Duration::from_millis(500)).await; continue; }
                            let is_c = code.len() > 2;
                            if is_c { hits.fetch_add(1, Ordering::Relaxed); }
                            res.lock().unwrap().insert(a.clone(), is_c);
                            break;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
            let n = prog.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 500 == 0 || n == total { eprintln!("[codecheck] {n}/{total} · контрактів {}", hits.load(Ordering::Relaxed)); }
        }));
    }
    for h in hs { let _ = h.await; }

    let r = res.lock().unwrap().clone();
    let contracts = r.values().filter(|v| **v).count();

    // ── ЕТАП 2: чи контракт є мультипідписом, і хто ним керує ──────────────────
    // `getOwners()` (0xa0e67e2b) у не-Safe контракті просто не відповість списком, тож
    // окремої перевірки типу не потрібно. Підписанти — це справжні розпорядники: вони
    // тримають лише газ, капіталу не мають, і жоден відбір за розміром їх не бачить.
    if std::env::var("SKIP_SAFE").is_err() {
        let todo: Vec<String> = r.iter().filter(|(_, v)| **v).map(|(a, _)| a.clone()).collect();
        eprintln!("[codecheck] перевіряю на мультипідпис: {} контрактів", todo.len());
        let mut db = PatternDb::load(&bf);
        let found = Arc::new(AtomicUsize::new(0));
        let owners_out: Arc<Mutex<Vec<(String, Vec<String>, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let sem2 = Arc::new(Semaphore::new(conc));
        let mut hs2 = Vec::new();

        for a in todo {
            let (client, keys, kc, sem2, found, owners_out) =
                (client.clone(), keys.clone(), kc.clone(), sem2.clone(), found.clone(), owners_out.clone());
            hs2.push(tokio::spawn(async move {
                let _p = sem2.acquire().await.unwrap();
                let mut owners: Vec<String> = Vec::new();
                for _ in 0..3 {
                    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                    let url = format!("https://api.etherscan.io/v2/api?chainid={chain}&module=proxy&action=eth_call&to={a}&data=0xa0e67e2b&tag=latest&apikey={key}");
                    if let Ok(rr) = client.get(&url).send().await {
                        if let Ok(v) = rr.json::<Value>().await {
                            let hex = v["result"].as_str().unwrap_or("");
                            if !hex.starts_with("0x") {
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                continue;
                            }
                            let h = &hex[2..];
                            // ABI: зсув, довжина, далі адреси по 32 байти
                            if h.len() >= 128 {
                                if let Ok(n) = u64::from_str_radix(&h[64..128], 16) {
                                    if n > 0 && n <= 50 && h.len() >= 128 + (n as usize) * 64 {
                                        for i in 0..n as usize {
                                            let w = &h[128 + i * 64..128 + (i + 1) * 64];
                                            owners.push(format!("0x{}", &w[24..]));
                                        }
                                    }
                                }
                            }
                            break;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
                if owners.is_empty() {
                    return;
                }
                // поріг: скільки підписів треба, щоб рушити кошти
                let mut thr = 0u64;
                for _ in 0..2 {
                    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                    let url = format!("https://api.etherscan.io/v2/api?chainid={chain}&module=proxy&action=eth_call&to={a}&data=0xe75235b8&tag=latest&apikey={key}");
                    if let Ok(rr) = client.get(&url).send().await {
                        if let Ok(v) = rr.json::<Value>().await {
                            let hex = v["result"].as_str().unwrap_or("");
                            if hex.starts_with("0x") && hex.len() > 2 {
                                thr = u64::from_str_radix(&hex[2..], 16).unwrap_or(0);
                                break;
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
                found.fetch_add(1, Ordering::Relaxed);
                owners_out.lock().unwrap().push((a, owners, thr));
            }));
        }
        for h in hs2 { let _ = h.await; }

        let list = owners_out.lock().unwrap().clone();
        eprintln!("[codecheck] мультипідписів знайдено: {}", list.len());
        for (safe, owners, thr) in &list {
            db.add(safe, Finding::new("SAFE", "codecheck")
                .ev("owners", owners.len() as f64).ev("threshold", *thr as f64));
            for o in owners {
                db.add(o, Finding::new("SAFE-OWNER", "codecheck")
                    .evk("of", &safe[..10], *thr as f64));
            }
            println!("  {safe} · підписантів {} · поріг {thr}", owners.len());
            for o in owners {
                println!("      {o}");
            }
        }
        db.save(&bf);
    }

    fs::write(&path, serde_json::to_string(&json!(r)).unwrap()).ok();
    eprintln!("[codecheck] ГОТОВО: у базі {} адрес, з них контрактів {} → {path}", r.len(), contracts);
}
