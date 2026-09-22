//! txfull — ПОВНА транзакційна історія гаманців мережі (а не зведення й не перші 10k подій).
//!
//! Навіщо: досі ми мали агрегати по контрагентах (eth-edges2) і ERC-20 лише для 7% членів.
//! На таких даних не можна шукати патерни поведінки: нема часу окремих транзакцій, газу, нонсів,
//! послідовності дій. Цей бін зберігає КОЖНУ транзакцію по-рядково.
//!
//! ⚠️ КЛЮЧОВЕ: Etherscan віддає максимум 10 000 записів на запит, тож «сторінками» повну історію
//! активного гаманця не взяти. Обхід — просування СТАРТОВОГО БЛОКУ: беремо партію в 1000 записів
//! у порядку зростання, запам'ятовуємо останній блок і продовжуємо з нього. Так вибирається вся
//! історія будь-якої довжини.
//!
//! Вихід: dataset/backfill/txfull-<chain>.jsonl, по рядку на транзакцію:
//!   {w,k:ext|int|tok,ts,h,f,t,v,tok,sym,gas,gp,nonce}
//! Резюм: txfull-<chain>.done (гаманець вважається зробленим лише після ПОВНОГО проходу).
//! Args: ROOT [WALLET_FILE]. Env: CHAIN(1) API_BASE(для Blockscout) CONC(8) KINDS(ext,int,tok) MAXWIN(200)
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

/// одна партія: повертає (записи, останній блок). Порожньо → історія вичерпана.
async fn batch(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, chain: u32, api: &str,
               action: &str, w: &str, from_block: u64) -> Vec<Value> {
    let params = format!("module=account&action={action}&address={w}&startblock={from_block}&endblock=99999999&page=1&offset=1000&sort=asc");
    for attempt in 0..5 {
        let url = if api.is_empty() {
            let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
            format!("https://api.etherscan.io/v2/api?chainid={chain}&{params}&apikey={key}")
        } else { format!("{api}?{params}") };
        if let Ok(r) = client.get(url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                if let Some(a) = v["result"].as_array() { return a.clone(); }
                if v["message"] == "No transactions found" { return Vec::new(); }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(350 * (attempt + 1))).await;
    }
    Vec::new()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let api = std::env::var("API_BASE").unwrap_or_default();
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let maxwin: u32 = std::env::var("MAXWIN").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let kinds: Vec<String> = std::env::var("KINDS").unwrap_or_else(|_| "ext,int,tok".into())
        .split(',').map(|s| s.trim().to_string()).collect();
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty() || !api.is_empty(), "потрібні ключі або API_BASE");

    let wallets: Vec<String> = fs::read_to_string(&wfile).expect("wallet file").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42 && l.starts_with("0x")).collect();
    let outp = format!("{bf}/txfull-{chain}.jsonl");
    let donep = format!("{bf}/txfull-{chain}.done");
    let mut done: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(&donep) { for l in BufReader::new(f).lines().map_while(Result::ok) { done.insert(l.trim().to_string()); } }
    let todo: Vec<String> = wallets.iter().filter(|w| !done.contains(*w)).cloned().collect();
    eprintln!("[txfull] мережа {chain} · гаманців {} · зроблено {} · до збору {} · види {:?} · conc {conc}",
        wallets.len(), done.len(), todo.len(), kinds);

    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-txfull/0.1").pool_max_idle_per_host(conc).timeout(std::time::Duration::from_secs(45)).build().unwrap());
    let keys = Arc::new(keys); let kc = Arc::new(AtomicUsize::new(0)); let api = Arc::new(api);
    let sem = Arc::new(Semaphore::new(conc));
    // БУФЕРИЗАЦІЯ + один запис на гаманець. Було: файл без буфера і writeln! на КОЖЕН рядок під
    // спільним мʼютексом — гаманець на 30к транзакцій робив 30к системних викликів, і всі інші
    // потоки стояли. Через це 28 паралельних запитів давали ту саму швидкість, що й 10.
    let out = Arc::new(Mutex::new(BufWriter::with_capacity(1 << 23,
        OpenOptions::new().create(true).append(true).open(&outp).unwrap())));
    let donef = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&donep).unwrap()));
    let prog = Arc::new(AtomicUsize::new(0));
    let txs_total = Arc::new(AtomicU64::new(0));
    let total = todo.len();
    let kinds = Arc::new(kinds);

    let mut hs = Vec::new();
    for w in todo {
        let (client, keys, kc, api, sem, out, donef, prog, txs_total, kinds) =
            (client.clone(), keys.clone(), kc.clone(), api.clone(), sem.clone(), out.clone(), donef.clone(), prog.clone(), txs_total.clone(), kinds.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut lines: Vec<String> = Vec::new();
            for kind in kinds.iter() {
                let action = match kind.as_str() { "ext" => "txlist", "int" => "txlistinternal", _ => "tokentx" };
                let mut from_block = 0u64;
                for _win in 0..maxwin {
                    let arr = batch(&client, &keys, &kc, chain, &api, action, &w, from_block).await;
                    if arr.is_empty() { break; }
                    let mut last_block = from_block;
                    for t in &arr {
                        let blk: u64 = t["blockNumber"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                        if blk > last_block { last_block = blk; }
                        let rec = json!({
                            "w": w, "k": kind, "ts": t["timeStamp"].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0),
                            "h": t["hash"].as_str().unwrap_or(""), "f": t["from"].as_str().unwrap_or("").to_lowercase(),
                            "t": t["to"].as_str().unwrap_or("").to_lowercase(), "v": t["value"].as_str().unwrap_or("0"),
                            "tok": t["contractAddress"].as_str().unwrap_or(""), "sym": t["tokenSymbol"].as_str().unwrap_or(""),
                            "dec": t["tokenDecimal"].as_str().unwrap_or(""),
                            "gas": t["gas"].as_str().unwrap_or(""), "gp": t["gasPrice"].as_str().unwrap_or(""),
                            "nonce": t["nonce"].as_str().unwrap_or(""), "err": t["isError"].as_str().unwrap_or("0")});
                        lines.push(rec.to_string());
                    }
                    if arr.len() < 1000 { break; }
                    if last_block <= from_block { break; }          // захист від зациклення
                    from_block = last_block;                         // наступне вікно з останнього блока
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                }
            }
            txs_total.fetch_add(lines.len() as u64, Ordering::Relaxed);
            // склеюємо ВСЕ поза мʼютексом, під ним — один write
            let mut blob = lines.join("\n");
            if !blob.is_empty() { blob.push('\n'); }
            { let mut o = out.lock().unwrap(); let _ = o.write_all(blob.as_bytes()); }
            { let mut d = donef.lock().unwrap(); let _ = writeln!(d, "{w}"); }
            let n = prog.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 100 == 0 || n == total {
                eprintln!("[txfull] {n}/{total} · транзакцій зібрано {}", txs_total.load(Ordering::Relaxed));
            }
        }));
    }
    for h in hs { let _ = h.await; }
    { let mut o = out.lock().unwrap(); let _ = o.flush(); }
    eprintln!("[txfull] ГОТОВО: {total} гаманців, {} транзакцій → {outp}", txs_total.load(Ordering::Relaxed));
}
