//! ethbal — РЕАЛЬНІ баланси ETH членів груп (Etherscan balancemulti, 20 адрес за виклик).
//! Причина: `wallet-capital.jsonl.eth_usd` — це flow-net оцінка (скільки ETH ПРОЙШЛО через гаманець
//! із 09.2025, clamp ≥0), а не залишок. Перевірка показала розрив у десятки разів:
//!   0xeae7380d — оцінка $1.16 млрд, фактичний баланс $4; 0xf8191d98 — $1.33 млрд проти $34.6М;
//!   0xad6eaa73 — $804М проти $554. Транзитні гаманці роздувають «капітал» на порядки.
//!   (Є й точні: 0x1d48963d — оцінка $685М, факт $704М.)
//! Вихід: dataset/backfill/eth-balances.json {asOf, ethPrice, wallets:{addr:{eth,usd}}}
//! Args: ROOT [WALLET_FILE]. Env: BATCH(20) CONC(4)
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let batch: usize = std::env::var("BATCH").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");

    // адреси: або з файлу, або всі члени груп у застосунку
    let mut addrs: HashSet<String> = HashSet::new();
    if let Some(wf) = std::env::args().nth(2) {
        for l in BufReader::new(File::open(&wf).expect("wallet file")).lines().map_while(Result::ok) {
            let a = l.trim().to_lowercase();
            if a.len() == 42 && a.starts_with("0x") { addrs.insert(a); }
        }
    } else if let Ok(v) = File::open(format!("{bf}/group-membership.json")).map_err(|e| e.to_string())
        .and_then(|f| serde_json::from_reader::<_, Value>(BufReader::new(f)).map_err(|e| e.to_string())) {
        // повне членство (grouppart); INFRA/NODATA пропускаємо — це не капітал груп
        for (w, x) in v["wallets"].as_object().cloned().unwrap_or_default() {
            let g = x["group"].as_str().unwrap_or("");
            if g == "INFRA" || g == "NODATA" { continue; }
            if w.len() == 42 { addrs.insert(w.to_lowercase()); }
        }
    } else {
        let cg: Value = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/capital-groups.json")).expect("capital-groups"))).unwrap();
        for g in cg["groups"].as_array().cloned().unwrap_or_default() {
            for m in g["members"].as_array().cloned().unwrap_or_default() {
                let a = m["address"].as_str().or_else(|| m.as_str()).unwrap_or("").to_lowercase();
                if a.len() == 42 { addrs.insert(a); }
            }
        }
    }
    let list: Vec<String> = addrs.into_iter().collect();
    let eth_price: f64 = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/token-prices.json")).unwrap())).ok()
        .and_then(|v| v["eth"].as_f64()).unwrap_or(0.0);
    eprintln!("[ethbal] адрес: {} · пачка {batch} · ETH ${eth_price:.2}", list.len());

    let client = Arc::new(reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let keys = Arc::new(keys);
    let kc = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(conc));
    let out: Arc<Mutex<serde_json::Map<String, Value>>> = Arc::new(Mutex::new(serde_json::Map::new()));
    let progress = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));   // пачки, які не вдалось отримати після всіх спроб
    let total = list.len().div_ceil(batch);

    let mut hs = Vec::new();
    for chunk in list.chunks(batch) {
        let chunk: Vec<String> = chunk.to_vec();
        let (client, keys, kc, sem, out, progress, failed) = (client.clone(), keys.clone(), kc.clone(), sem.clone(), out.clone(), progress.clone(), failed.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut got = false;
            // 7 спроб з експоненційним відкатом замість 4 з фіксованою паузою: під лімітом
            // Etherscan віддає HTTP 200 з `result` у вигляді РЯДКА, тож `as_array()` мовчки
            // не спрацьовував і пачка з `batch` адрес просто зникала з результату. Далі
            // `capreal` рахував їм нуль капіталу, і ніде не було видно, скільки саме втрачено.
            for attempt in 0..7u64 {
                let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=balancemulti&address={}&tag=latest&apikey={key}", chunk.join(","));
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        if let Some(arr) = v["result"].as_array() {
                            let mut o = out.lock().unwrap();
                            for e in arr {
                                let a = e["account"].as_str().unwrap_or("").to_lowercase();
                                let wei: f64 = e["balance"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                                let eth = wei / 1e18;
                                o.insert(a, json!({"eth": eth, "usd": eth * eth_price}));
                            }
                            got = true;
                            break;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << attempt.min(6)))).await;
            }
            if !got { failed.fetch_add(1, Ordering::Relaxed); }
            let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 25 == 0 || n == total { eprintln!("[ethbal] {n}/{total} пачок"); }
        }));
    }
    for h in hs { let _ = h.await; }

    // ФАЗА 2: для великих балансів перевіряємо eth_getCode — контракт чи справжній гаманець.
    // Без цього у «капітал групи» потрапляє сам контракт WETH (2.3М ETH) та немічені мости/пули.
    let code_min: f64 = std::env::var("CODE_MIN").ok().and_then(|s| s.parse().ok()).unwrap_or(50.0);
    let big: Vec<String> = out.lock().unwrap().iter()
        .filter(|(_, v)| v["eth"].as_f64().unwrap_or(0.0) >= code_min).map(|(k, _)| k.clone()).collect();
    eprintln!("[ethbal] перевірка коду для {} адрес із балансом ≥{code_min} ETH", big.len());
    let checked = Arc::new(AtomicUsize::new(0));
    let mut h2 = Vec::new();
    for a in big {
        let (client, keys, kc, sem, out, checked) = (client.clone(), keys.clone(), kc.clone(), sem.clone(), out.clone(), checked.clone());
        h2.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            for _ in 0..3 {
                let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=proxy&action=eth_getCode&address={a}&tag=latest&apikey={key}");
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        if let Some(code) = v["result"].as_str() {
                            // ВАЖЛИВО: при ліміті Etherscan повертає у result ТЕКСТ помилки
                            // ("Max rate limit reached"), і перевірка «непорожній рядок = контракт»
                            // хибно мітила EOA контрактами (653 адреси, $2млрд викинуто з капіталу).
                            // Валідна відповідь — лише "0x" або "0x<байткод>"; решта = ретрай.
                            if !code.starts_with("0x") { tokio::time::sleep(std::time::Duration::from_millis(600)).await; continue; }
                            let is_c = code.len() > 2;
                            out.lock().unwrap().get_mut(&a).map(|x| x["code"] = json!(is_c));
                            if is_c { checked.fetch_add(1, Ordering::Relaxed); }
                            break;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
        }));
    }
    for h in h2 { let _ = h.await; }
    eprintln!("[ethbal] з них контрактів: {}", checked.load(Ordering::Relaxed));

    let mut o = out.lock().unwrap().clone();
    let nfail = failed.load(Ordering::Relaxed);

    // ЗЛИТТЯ, а не перезапис: файл писався через fs::write цілком, тож пачка, яку цього разу
    // з'їв ліміт, зникала з нього назовсім — капітал груп мовчки просідав, і за самим файлом
    // це було не відрізнити від «у цих адрес справді нуль». Свіже значення завжди перемагає
    // старе; старе лишається тільки там, де свіжого немає.
    let outp = format!("{bf}/eth-balances.json");
    if nfail > 0 {
        if let Some(prev) = File::open(&outp).ok()
            .and_then(|f| serde_json::from_reader::<_, Value>(BufReader::new(f)).ok())
        {
            let mut kept = 0usize;
            for (a, v) in prev["wallets"].as_object().cloned().unwrap_or_default() {
                if !o.contains_key(&a) { o.insert(a, v); kept += 1; }
            }
            eprintln!("[ethbal] ⚠ {nfail} пачок не отримано (ліміт) — узято {kept} балансів із попереднього прогону (можуть бути застарілі)");
        } else {
            eprintln!("[ethbal] ⚠ {nfail} пачок не отримано (ліміт) і попереднього файлу нема — ці адреси відсутні у виході, capreal порахує їм нуль");
        }
    }

    let total_eth: f64 = o.values().filter_map(|v| v["eth"].as_f64()).sum();
    let eoa_eth: f64 = o.values().filter(|v| v["code"] != json!(true)).filter_map(|v| v["eth"].as_f64()).sum();
    eprintln!("[ethbal] з них на НЕ-контрактах: {:.1} ETH (${:.0})", eoa_eth, eoa_eth * eth_price);
    fs::write(&outp, serde_json::to_string_pretty(&json!({
        "asOf": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "ethPrice": eth_price, "failedBatches": nfail, "wallets": o })).unwrap()).ok();
    eprintln!("[ethbal] отримано балансів: {} · сумарно {:.1} ETH (${:.0})", o.len(), total_eth, total_eth * eth_price);
    eprintln!("[ethbal] збережено eth-balances.json");
}
