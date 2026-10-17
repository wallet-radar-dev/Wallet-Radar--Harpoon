//! paribu_deanon (Rust) — деанонімізація в межах он-чейну: чи «різні» ядрові гаманці шлють на ОДНУ
//! депозит-адресу Paribu (=1 рахунок=1 сутність). Фаза1: краул вихідних ядра → dest->{ядрові}.
//! Фаза2: для dest з ≥2 ядрових перевіряє форвардинг у Paribu (hot 3/4/5). Reqwest+tokio, таймаути
//! (без зависань як у Python). Ключі BACKFILL_ETHERSCAN_KEYS. Вихід: dataset/backfill/paribu-deposits.json.
//! Env: COV_K(5) MAXCORE(200) CONC(8) OFFSET(2000).
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;



/// Адреси Paribu — тег `paribu_deanon:paribu` у базі.
fn paribu() -> Vec<String> {
    let c = wr_store::open().expect("paribu_deanon: без сховища немає переліку адрес");
    let mut q = c
        .prepare("SELECT address FROM wallet_tags WHERE tag='paribu_deanon:paribu'")
        .expect("запит тегу");
    let v: Vec<String> =
        q.query_map([], |r| r.get(0)).expect("читання").filter_map(|r| r.ok()).collect();
    assert!(!v.is_empty(), "paribu_deanon: тег paribu_deanon:paribu порожній");
    v
}

fn es_url(keys: &[String], kc: &AtomicUsize, params: &str) -> String {
    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
    format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")
}
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Vec<Value> {
    for _ in 0..5 {
        if let Ok(r) = client.get(es_url(keys, kc, params)).send().await {
            if let Ok(v) = r.json::<Value>().await { if let Some(a) = v["result"].as_array() { return a.clone(); } }
        }
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }
    Vec::new()
}

/// вихідні призначення ядрового (from==w, value>0) + чи прямо в Paribu
async fn outbound(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, w: &str, offset: u32) -> (HashSet<String>, bool) {
    let plist = paribu();
    let paribu: HashSet<&str> = plist.iter().map(String::as_str).collect();
    let txs = es(client, keys, kc, &format!("module=account&action=txlist&address={w}&startblock=0&endblock=99999999&page=1&offset={offset}&sort=asc")).await;
    let mut dests = HashSet::new(); let mut direct = false;
    for t in &txs {
        if t["from"].as_str().unwrap_or("").to_lowercase() != w { continue; }
        let val: u128 = t["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0); if val == 0 { continue; }
        let to = t["to"].as_str().unwrap_or("").to_lowercase();
        if to.is_empty() || to == w { continue; }
        if paribu.contains(to.as_str()) { direct = true; } else { dests.insert(to); }
    }
    (dests, direct)
}
async fn forwards_to_paribu(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, addr: &str) -> bool {
    let plist = paribu();
    let paribu: HashSet<&str> = plist.iter().map(String::as_str).collect();
    let txs = es(client, keys, kc, &format!("module=account&action=txlist&address={addr}&startblock=0&endblock=99999999&page=1&offset=500&sort=asc")).await;
    txs.iter().any(|t| t["from"].as_str().unwrap_or("").to_lowercase() == addr && paribu.contains(t["to"].as_str().unwrap_or("").to_lowercase().as_str()))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let cov_k: u64 = std::env::var("COV_K").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let maxcore: usize = std::env::var("MAXCORE").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let offset: u32 = std::env::var("OFFSET").ok().and_then(|s| s.parse().ok()).unwrap_or(2000);
    let bf = format!("{root}/dataset/backfill");

    let keys: Vec<String> = fs::read_to_string(format!("{root}/.env")).unwrap_or_default()
        .lines().find(|l| l.starts_with("BACKFILL_ETHERSCAN_KEYS="))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect()).unwrap_or_default();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");
    // ядро з rglead
    let mut cores: Vec<String> = Vec::new();
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/rglead.json")).unwrap())) {
        if let Some(ws) = v["wallets"].as_array() { for w in ws { if w["coverage"].as_u64().unwrap_or(0) >= cov_k { if let Some(a) = w["w"].as_str() { cores.push(a.to_lowercase()); } } } }
    }
    cores.truncate(maxcore);
    let core_set: HashSet<String> = cores.iter().cloned().collect();
    eprintln!("[paribu] ядро (охопл≥{}): {} гам, conc {}, offset {}", cov_k, cores.len(), conc, offset);

    let keys = Arc::new(keys); let kc = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-paribu/0.1").pool_max_idle_per_host(conc).timeout(std::time::Duration::from_secs(30)).build().unwrap());
    let sem = Arc::new(Semaphore::new(conc));

    // ФАЗА 1: краул вихідних ядра
    let dest_cores: Arc<Mutex<HashMap<String, HashSet<String>>>> = Arc::new(Mutex::new(HashMap::new()));
    let direct: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicUsize::new(0)); let total = cores.len();
    let mut h1 = Vec::new();
    for c in cores.clone() {
        let (keys, kc, client, sem, dc, dir, done) = (keys.clone(), kc.clone(), client.clone(), sem.clone(), dest_cores.clone(), direct.clone(), done.clone());
        h1.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let (dests, dp) = outbound(&client, &keys, &kc, &c, offset).await;
            if dp { dir.lock().unwrap().push(c.clone()); }
            { let mut m = dc.lock().unwrap(); for d in dests { m.entry(d).or_default().insert(c.clone()); } }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1; if n % 25 == 0 { eprintln!("[paribu] фаза1 {n}/{total}"); }
        }));
    }
    for h in h1 { let _ = h.await; }
    let dc = dest_cores.lock().unwrap().clone();
    let shared: Vec<(String, HashSet<String>)> = dc.into_iter().filter(|(d, cs)| cs.len() >= 2 && !core_set.contains(d)).collect();
    eprintln!("[paribu] прямо в Paribu: {} ядрових | спільних призначень (≥2 ядрових): {}", direct.lock().unwrap().len(), shared.len());

    // ФАЗА 2: перевірка форвардингу у Paribu
    let hits: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let mut h2 = Vec::new();
    let mut sv: Vec<(String, HashSet<String>)> = shared; sv.sort_by(|a, b| b.1.len().cmp(&a.1.len())); sv.truncate(200);
    for (d, cs) in sv {
        let (keys, kc, client, sem, hits) = (keys.clone(), kc.clone(), client.clone(), sem.clone(), hits.clone());
        h2.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            if forwards_to_paribu(&client, &keys, &kc, &d).await {
                let cs: Vec<String> = cs.into_iter().collect();
                eprintln!("   ✅ СПІЛЬНА ДЕПОЗИТ-АДРЕСА Paribu {}  ←  {} ядрових = ОДНА сутність", d, cs.len());
                hits.lock().unwrap().push(json!({"dep": d, "cores": cs}));
            }
        }));
    }
    for h in h2 { let _ = h.await; }

    // КЛАСИФІКАЦІЯ всіх спільних призначень (не лише Paribu-форвардних): біржа/контракт/приватний EOA
    // приватний EOA, куди шлють ≥2 ядрових = та сама рука (деанон, якого не дав Paribu)
    let shared2: Vec<(String, HashSet<String>)> = dest_cores.lock().unwrap().iter().filter(|(d, cs)| cs.len() >= 2 && !core_set.contains(*d)).map(|(d, cs)| (d.clone(), cs.clone())).collect();
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) { for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); } }
    let mut classified: Vec<Value> = Vec::new();
    for (d, cs) in &shared2 {
        let label = lbl.get(d).and_then(|x| x["name"].as_str());
        let is_contract = kind.get(d).map(|k| k == "contract").unwrap_or(false);
        let is_exch = label.map(|n| { let l = n.to_lowercase(); ["paribu","binance","coinbase","okx","gate","kraken","bybit","kucoin","bitget","huobi","mexc","bitfinex","upbit","bithumb","uphold","btcturk","bingx","exchange","deposit"].iter().any(|c| l.contains(c)) }).unwrap_or(false) || matches!(kind.get(d).map(|s| s.as_str()), Some("exchange") | Some("otc"));
        let class = if is_exch { "біржа" } else if is_contract { "контракт" } else { "приватний-EOA" };
        classified.push(json!({"dest": d, "cores": cs.len(), "label": label, "class": class}));
    }
    classified.sort_by(|a, b| b["cores"].as_u64().unwrap().cmp(&a["cores"].as_u64().unwrap()));
    let priv_eoa: Vec<&Value> = classified.iter().filter(|c| c["class"] == "приватний-EOA").collect();
    eprintln!("[paribu] --- 43 СПІЛЬНІ ПРИЗНАЧЕННЯ класифіковано ---");
    for c in classified.iter().take(20) { eprintln!("   {} ← {} ядрових · {} {}", c["dest"].as_str().unwrap(), c["cores"].as_u64().unwrap(), c["class"].as_str().unwrap(), c["label"].as_str().unwrap_or("")); }
    eprintln!("[paribu] ПРИВАТНИХ-EOA спільних призначень (=та сама рука): {}", priv_eoa.len());
    fs::write(format!("{bf}/shared-dests.json"), serde_json::to_string_pretty(&json!({"shared": classified})).unwrap()).ok();

    let hits = hits.lock().unwrap();
    let direct = direct.lock().unwrap();
    let linked: HashSet<&str> = hits.iter().flat_map(|h| h["cores"].as_array().unwrap().iter().map(|c| c.as_str().unwrap())).collect();
    eprintln!("[paribu] ===== РЕЗУЛЬТАТ =====");
    eprintln!("[paribu] спільних депозит-адрес Paribu (≥2 ядрових): {}", hits.len());
    eprintln!("[paribu] ядрових деанонімізовано як спільні рахунки: {}", linked.len());
    eprintln!("[paribu] шлють ПРЯМО в Paribu: {}", direct.len());
    fs::write(format!("{bf}/paribu-deposits.json"), serde_json::to_string_pretty(&json!({"shared_deposits": &*hits, "direct_to_paribu": &*direct, "linked_wallets": linked.len()})).unwrap()).ok();
    eprintln!("[paribu] збережено paribu-deposits.json");
}
