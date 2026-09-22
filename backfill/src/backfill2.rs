//! Full-history ERC-20 backfill for Wallet Radar — Etherscan V2 (швидко) + Blockscout (фолбек).
//!
//! ETHERSCAN-режим (env ETHERSCAN_API_KEYS=key1,key2,...): для мереж із chainid тягне tokentx
//! через Etherscan V2 з block-range пагінацією (free-tier 1000 записів/запит) і РОТАЦІЄЮ ключів
//! (кожен запит бере наступний ключ круговою). ~×20 щільніше за Blockscout + N ключів паралельно.
//! Непідтримувані Etherscan-мережі падають на Blockscout (як раніше).
//!
//! БЕЗПЕКА: OUT_FILE/PROG_FILE перевизначають шляхи (за замовч. transfers.csv/_progress.csv).
//! Для Etherscan-прогону пишемо в transfers-es.csv, щоб не чіпати робочий датасет; звіряємо → підміна.
//!
//! CSV-схема ІДЕНТИЧНА: wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash
//! (ts — RFC3339 з .000000Z, як у Blockscout-рядках, щоб downstream-парсери не ламались).
//!
//! Env: APP_ROOT, CONCURRENCY(4), MAX_PAGES(100000), LIMIT_WALLETS(0), ETHERSCAN_API_KEYS(""),
//!      OUT_FILE, PROG_FILE.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use serde_json::Value;
use tokio::sync::Mutex;

type Wr = Arc<Mutex<std::io::BufWriter<std::fs::File>>>;

/// chain -> Blockscout base URL (фолбек). Копія bs.rs::host().
fn host(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" => "https://eth.blockscout.com",
        "base" => "https://base.blockscout.com",
        "optimism" => "https://optimism.blockscout.com",
        "gnosis" => "https://gnosis.blockscout.com",
        "polygon" => "https://polygon.blockscout.com",
        "arbitrum" => "https://arbitrum.blockscout.com",
        "zksync" => "https://zksync.blockscout.com",
        "scroll" => "https://scroll.blockscout.com",
        "mode" => "https://explorer.mode.network",
        "fraxtal" => "https://fraxscan.blockscout.com",
        "blast" => "https://blast.blockscout.com",
        "linea" => "https://explorer.linea.build",
        "zora" => "https://explorer.zora.energy",
        "celo" => "https://celo.blockscout.com",
        "mantle" => "https://mantle.blockscout.com",
        "sonic" => "https://sonic.blockscout.com",
        "rootstock" => "https://rootstock.blockscout.com",
        "fuse" => "https://explorer.fuse.io",
        "metis" => "https://andromeda-explorer.metis.io",
        "aurora" => "https://explorer.aurora.dev",
        "lisk" => "https://blockscout.lisk.com",
        "redstone" => "https://explorer.redstone.xyz",
        "immutable" => "https://explorer.immutable.com",
        "lukso" => "https://explorer.execution.mainnet.lukso.network",
        _ => return None,
    })
}

/// chain -> Etherscan V2 chainid (для підтримуваних). None -> фолбек на Blockscout.
fn etherscan_chainid(chain: &str) -> Option<u64> {
    Some(match chain {
        "eth" => 1,
        "base" => 8453,
        "optimism" => 10,
        "gnosis" => 100,
        "polygon" => 137,
        "arbitrum" => 42161,
        "zksync" => 324,
        "scroll" => 534352,
        "blast" => 81457,
        "linea" => 59144,
        "celo" => 42220,
        "mantle" => 5000,
        "sonic" => 146,
        "fraxtal" => 252,
        "mode" => 34443,
        "zora" => 7777777,
        "metis" => 1088,
        "lisk" => 1135,
        _ => return None, // rootstock/fuse/aurora/redstone/immutable/lukso -> Blockscout
    })
}

const CHAINS: &[&str] = &[
    "eth", "base", "optimism", "gnosis", "polygon", "arbitrum", "zksync", "scroll", "mode",
    "fraxtal", "blast", "linea", "zora", "celo", "mantle", "sonic", "rootstock", "fuse",
    "metis", "aurora", "lisk", "redstone", "immutable", "lukso",
];

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c == ',' || c == '"' || c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

// ---------- Blockscout шлях (без змін) ----------

fn decimals_of(it: &Value) -> f64 {
    let pick = |v: &Value| -> Option<f64> {
        v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64())
    };
    pick(&it["total"]["decimals"])
        .or_else(|| pick(&it["token"]["decimals"]))
        .unwrap_or(0.0)
}

fn row(wallet: &str, chain: &str, it: &Value) -> Option<String> {
    let from = it["from"]["hash"].as_str().unwrap_or("").to_lowercase();
    let to = it["to"]["hash"].as_str().unwrap_or("").to_lowercase();
    if from.is_empty() && to.is_empty() {
        return None;
    }
    let dir = if from == wallet { "o" } else if to == wallet { "i" } else { "?" };
    let counterparty = if dir == "o" { &to } else { &from };
    let token = {
        let a = it["token"]["address"].as_str().unwrap_or("");
        if a.is_empty() { it["token"]["address_hash"].as_str().unwrap_or("") } else { a }
    }
    .to_lowercase();
    let symbol = sanitize(it["token"]["symbol"].as_str().unwrap_or(""));
    let dec = decimals_of(it);
    let raw = it["total"]["value"].as_str().unwrap_or("0");
    let amount = raw.parse::<f64>().unwrap_or(0.0) / 10f64.powf(dec);
    let ts = it["timestamp"].as_str().unwrap_or("");
    let hash = it["transaction_hash"].as_str().or_else(|| it["tx_hash"].as_str()).unwrap_or("");
    Some(format!("{wallet},{chain},{dir},{ts},{token},{symbol},{counterparty},{amount},{hash}"))
}

fn next_query(next: &Value) -> String {
    let mut q = String::from("type=ERC-20");
    if let Some(obj) = next.as_object() {
        for (k, v) in obj {
            let vs = match v {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            q.push('&');
            q.push_str(k);
            q.push('=');
            q.push_str(&vs);
        }
    }
    q
}

async fn write_ckpt(pgw: &Wr, wallet: &str, chain: &str, rows: u64, status: &str, page: u32, query: &str) {
    let status = status.replace(',', ";");
    let mut g = pgw.lock().await;
    let _ = writeln!(*g, "{wallet},{chain},{rows},{status},{page},{query}");
    let _ = g.flush();
}

async fn fetch_pair(
    client: &reqwest::Client, chain: &str, wallet: &str, max_pages: u32,
    start_page: u32, start_query: &str, start_rows: u64, txw: &Wr, pgw: &Wr,
) -> (u64, String) {
    let short = &wallet[..8.min(wallet.len())];
    let Some(h) = host(chain) else {
        write_ckpt(pgw, wallet, chain, start_rows, "done", start_page, start_query).await;
        return (start_rows, "done".to_string());
    };
    let mut rows_total = start_rows;
    let mut query = start_query.to_string();
    let mut page = start_page;
    let mut prev_q = String::new();
    loop {
        let url = format!("{h}/api/v2/addresses/{wallet}/token-transfers?{query}");
        if page % 25 == 0 { eprintln!("  [bs] {short} {chain} p{page} rows={rows_total}"); }
        let mut attempt = 0u32;
        let d: Value = loop {
            attempt += 1;
            let sent = tokio::time::timeout(Duration::from_secs(20),
                client.get(&url).header("accept", "application/json").send()).await;
            let backoff_ms: u64 = match sent {
                Err(_) => { if attempt >= 4 { write_ckpt(pgw, wallet, chain, rows_total, "err:req:timeout", page, &query).await; return (rows_total, "err:req:timeout".to_string()); } 800 * attempt as u64 }
                Ok(Ok(r)) if r.status().is_success() => match tokio::time::timeout(Duration::from_secs(20), r.json::<Value>()).await {
                    Ok(Ok(v)) => break v,
                    _ => { if attempt >= 6 { write_ckpt(pgw, wallet, chain, rows_total, "err:json:read", page, &query).await; return (rows_total, "err:json:read".to_string()); } 800 }
                },
                Ok(Ok(r)) => {
                    let code = r.status().as_u16();
                    if code == 404 { write_ckpt(pgw, wallet, chain, rows_total, "done", page, &query).await; return (rows_total, "done".to_string()); }
                    if code == 403 { write_ckpt(pgw, wallet, chain, rows_total, "err:http:403", page, &query).await; return (rows_total, "err:http:403".to_string()); }
                    if attempt >= 8 { let s = format!("err:http:{code}"); write_ckpt(pgw, wallet, chain, rows_total, &s, page, &query).await; return (rows_total, s); }
                    if code == 429 { 2500 * attempt as u64 } else { 800 * attempt as u64 }
                }
                Ok(Err(e)) => { if attempt >= 4 { let s = format!("err:req:{e}"); write_ckpt(pgw, wallet, chain, rows_total, &s, page, &query).await; return (rows_total, s); } 800 * attempt as u64 }
            };
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        };
        let items = d["items"].as_array().cloned().unwrap_or_default();
        { let mut g = txw.lock().await; for it in &items { if let Some(r) = row(wallet, chain, it) { let _ = writeln!(*g, "{r}"); rows_total += 1; } } let _ = g.flush(); }
        let next = &d["next_page_params"];
        page += 1;
        let q = next_query(next);
        if items.is_empty() || next.is_null() || page >= max_pages { write_ckpt(pgw, wallet, chain, rows_total, "done", page, &q).await; return (rows_total, "done".to_string()); }
        if q == query || q == prev_q { write_ckpt(pgw, wallet, chain, rows_total, "done", page, &q).await; return (rows_total, "done".to_string()); }
        prev_q = std::mem::replace(&mut query, q);
        if page % 20 == 0 { write_ckpt(pgw, wallet, chain, rows_total, "part", page, &query).await; }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

// ---------- Etherscan V2 шлях (нове) ----------

/// unix seconds -> "YYYY-MM-DDTHH:MM:SS.000000Z" (як Blockscout-рядки).
fn ts_rfc3339(unix: i64) -> String {
    match chrono::DateTime::from_timestamp(unix, 0) {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string(),
        None => String::new(),
    }
}

/// Один Etherscan tokentx запис -> CSV-рядок (ідентична схема).
fn row_es(wallet: &str, chain: &str, it: &Value) -> Option<String> {
    let from = it["from"].as_str().unwrap_or("").to_lowercase();
    let to = it["to"].as_str().unwrap_or("").to_lowercase();
    if from.is_empty() && to.is_empty() { return None; }
    let dir = if from == wallet { "o" } else if to == wallet { "i" } else { "?" };
    let counterparty = if dir == "o" { &to } else { &from };
    let token = it["contractAddress"].as_str().unwrap_or("").to_lowercase();
    let symbol = sanitize(it["tokenSymbol"].as_str().unwrap_or(""));
    let dec: f64 = it["tokenDecimal"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let raw = it["value"].as_str().unwrap_or("0");
    let amount = raw.parse::<f64>().unwrap_or(0.0) / 10f64.powf(dec);
    let unix: i64 = it["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
    let ts = ts_rfc3339(unix);
    let hash = it["hash"].as_str().unwrap_or("");
    Some(format!("{wallet},{chain},{dir},{ts},{token},{symbol},{counterparty},{amount},{hash}"))
}

/// Ключ для boundary-dedup у межах одного блоку.
fn es_key(it: &Value) -> String {
    format!("{}|{}|{}|{}|{}",
        it["hash"].as_str().unwrap_or(""), it["from"].as_str().unwrap_or(""),
        it["to"].as_str().unwrap_or(""), it["contractAddress"].as_str().unwrap_or(""),
        it["value"].as_str().unwrap_or(""))
}

/// Повна ERC-20 історія (wallet, chain) через Etherscan V2 block-range пагінацію.
/// keys — усі ключі; key_ctr — глобальний лічильник ротації. Резюме: "es:sb=<block>".
#[allow(clippy::too_many_arguments)]
async fn fetch_pair_es(
    client: &reqwest::Client, chainid: u64, chain: &str, wallet: &str,
    keys: &[String], key_ctr: &AtomicUsize, start_block: u64, start_rows: u64, txw: &Wr, pgw: &Wr,
) -> (u64, String) {
    let short = &wallet[..8.min(wallet.len())];
    let mut rows_total = start_rows;
    let mut sb = start_block;
    let mut boundary_block: u64 = u64::MAX; // блок, чиї записи вже записані (dedup)
    let mut boundary_seen: HashSet<String> = HashSet::new();
    let mut iters = 0u32;
    let mut notok = 0u32; // скільки разів поспіль API відповів «0» без «немає транзакцій»
    // Сторінка В МЕЖАХ вікна блоків. Без неї гаманець, що має понад 1000 переказів у
    // ОДНОМУ блоці, обривався назавжди: наступний запит повертав ті самі записи, всі
    // вони відсіювались як дублі boundary-блоку, written_this ставав нулем — і краул
    // позначав гаманець `done`, мовчки викидаючи решту історії.
    let mut page = 1u32;
    loop {
        iters += 1;
        if iters % 25 == 0 { eprintln!("  [es] {short} {chain} sb={sb} rows={rows_total}"); }
        let key = &keys[key_ctr.fetch_add(1, Ordering::Relaxed) % keys.len()];
        let url = format!("https://api.etherscan.io/v2/api?chainid={chainid}&module=account&action=tokentx&address={wallet}&startblock={sb}&endblock=99999999&page={page}&offset=1000&sort=asc&apikey={key}");
        let mut attempt = 0u32;
        let d: Value = loop {
            attempt += 1;
            let sent = tokio::time::timeout(Duration::from_secs(25),
                client.get(&url).header("accept", "application/json").send()).await;
            let backoff: u64 = match sent {
                Err(_) => { if attempt >= 5 { write_ckpt(pgw, wallet, chain, rows_total, "err:req:timeout", 0, &format!("es:sb={sb}")).await; return (rows_total, "err:req:timeout".to_string()); } 700 * attempt as u64 }
                Ok(Ok(r)) if r.status().is_success() => match tokio::time::timeout(Duration::from_secs(20), r.json::<Value>()).await {
                    Ok(Ok(v)) => break v,
                    _ => { if attempt >= 6 { write_ckpt(pgw, wallet, chain, rows_total, "err:json:read", 0, &format!("es:sb={sb}")).await; return (rows_total, "err:json:read".to_string()); } 700 }
                },
                Ok(Ok(r)) => { let code = r.status().as_u16(); if attempt >= 8 { let s = format!("err:http:{code}"); write_ckpt(pgw, wallet, chain, rows_total, &s, 0, &format!("es:sb={sb}")).await; return (rows_total, s); } if code == 429 { 1200 * attempt as u64 } else { 700 * attempt as u64 } }
                Ok(Err(e)) => { if attempt >= 4 { let s = format!("err:req:{e}"); write_ckpt(pgw, wallet, chain, rows_total, &s, 0, &format!("es:sb={sb}")).await; return (rows_total, s); } 700 * attempt as u64 }
            };
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        };
        // Etherscan: status "1" з масивом, або "0" з message (порожньо / no transactions).
        let status = d["status"].as_str().unwrap_or("0");
        let results = d["result"].as_array().cloned().unwrap_or_default();
        if status != "1" || results.is_empty() {
            // Etherscan віддає status:"0" З HTTP 200 і на «немає транзакцій», і на
            // «Max calls per sec rate limit reached». Позначати обидва як done — це
            // тиха втрата гаманця: саме так 21664 адреси групи отримали «виконано»
            // з нулем рядків, хоча перекази в них є.
            let msg = d["message"].as_str().unwrap_or("");
            let res_txt = d["result"].as_str().unwrap_or("");
            let empty_ok = msg.contains("No transactions") || msg.contains("No records")
                || res_txt.contains("No transactions") || res_txt.contains("No records");
            if !empty_ok {
                if notok < 6 {
                    notok += 1;
                    tokio::time::sleep(Duration::from_millis(1500 * notok as u64)).await;
                    continue; // той самий sb, ще раз — квота відновиться
                }
                let s = format!("err:notok:{}", if res_txt.is_empty() { msg } else { res_txt });
                write_ckpt(pgw, wallet, chain, rows_total, &s, 0, &format!("es:sb={sb}")).await;
                return (rows_total, s);
            }
            write_ckpt(pgw, wallet, chain, rows_total, "done", 0, &format!("es:sb={sb}")).await;
            return (rows_total, "done".to_string());
        }
        notok = 0;
        let n = results.len();
        // записуємо, пропускаючи вже записані записи boundary-блоку
        let mut written_this = 0u64;
        let mut last_block: u64 = sb;
        {
            let mut g = txw.lock().await;
            for it in &results {
                let bn: u64 = it["blockNumber"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                if bn == boundary_block && boundary_seen.contains(&es_key(it)) { continue; }
                if let Some(r) = row_es(wallet, chain, it) { let _ = writeln!(*g, "{r}"); rows_total += 1; written_this += 1; }
                last_block = bn;
            }
            let _ = g.flush();
        }
        // кінець історії: сторінка неповна
        if n < 1000 { write_ckpt(pgw, wallet, chain, rows_total, "done", 0, &format!("es:sb={last_block}")).await; return (rows_total, "done".to_string()); }
        // Нічого нового не записали — але це НЕ обов'язково кінець історії. Якщо сторінка
        // повна, значить у boundary-блоці більше 1000 переказів: беремо наступну сторінку
        // того ж вікна (стеля Etherscan — page*offset <= 10000). Позначати тут `done`, як
        // було раніше, означало тихо втратити все, що йде після такого блока.
        if written_this == 0 && n == 1000 && page < 10 { page += 1; continue; }
        if written_this == 0 && n == 1000 {
            // понад 10000 переказів в одному блоці — далі Etherscan не віддає; рухаємось
            // за межу блока й чесно кажемо про це в лог, а не мовчимо
            eprintln!("  [es] {short} {chain} блок {last_block}: >10000 переказів, частину не забрати");
            sb = last_block + 1; page = 1; boundary_seen.clear(); boundary_block = u64::MAX;
            continue;
        }
        if written_this == 0 { write_ckpt(pgw, wallet, chain, rows_total, "done", 0, &format!("es:sb={last_block}")).await; return (rows_total, "done".to_string()); }
        // готуємо dedup для наступної сторінки: усі записи last_block
        boundary_seen = results.iter().filter(|it| it["blockNumber"].as_str().and_then(|s| s.parse::<u64>().ok()) == Some(last_block)).map(es_key).collect();
        boundary_block = last_block;
        sb = last_block; // перекриваємось на boundary-блоці, дублі відсіє dedup
        page = 1;
        if iters % 20 == 0 { write_ckpt(pgw, wallet, chain, rows_total, "part", 0, &format!("es:sb={sb}")).await; }
    }
}

const DEAD: i32 = 5;

/// Обробити одну (wallet, chain) пару. Винесено з main, щоб гнати ES- і BS-пари у ДВА
/// НЕЗАЛЕЖНІ пули конкурентності (повільний Blockscout не краде слоти у швидкого Etherscan).
#[allow(clippy::too_many_arguments)]
async fn run_one(
    w: String, c: String, page0: u32, query0: String, rows0: u64,
    client: Arc<reqwest::Client>, txw: Wr, pgw: Wr,
    counter: Arc<AtomicUsize>, health: Arc<Mutex<HashMap<String, i32>>>,
    key_ctr: Arc<AtomicUsize>, es_keys: Arc<Vec<String>>,
    es_mode: bool, max_pages: u32, total: usize,
) {
    if health.lock().await.get(&c).copied().unwrap_or(0) >= DEAD {
        write_ckpt(&pgw, &w, &c, 0, "skip:dead", 0, "type=ERC-20").await;
        let i = counter.fetch_add(1, Ordering::Relaxed) + 1;
        eprintln!("[{i}/{total}] {} {:9} -> skip:dead", &w[..10.min(w.len())], c);
        return;
    }
    let (n, status) = match (es_mode, etherscan_chainid(&c)) {
        (true, Some(cid)) => {
            let sb0 = query0.strip_prefix("es:sb=").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            fetch_pair_es(&client, cid, &c, &w, &es_keys, &key_ctr, sb0, rows0, &txw, &pgw).await
        }
        _ => fetch_pair(&client, &c, &w, max_pages, page0, &query0, rows0, &txw, &pgw).await,
    };
    if status == "done" { *health.lock().await.entry(c.clone()).or_insert(0) = 0; }
    else if status.starts_with("err:req:") || status == "err:http:403" { *health.lock().await.entry(c.clone()).or_insert(0) += 1; }
    let i = counter.fetch_add(1, Ordering::Relaxed) + 1;
    let tag = if status == "done" { "" } else { status.as_str() };
    eprintln!("[{i}/{total}] {} {:9} -> {:8} {}", &w[..10.min(w.len())], c, n, tag);
}

#[tokio::main(flavor = "multi_thread", worker_threads = 6)]
async fn main() {
    let root = env_or("APP_ROOT", "/opt/wallet-tracker");
    let ds = format!("{root}/dataset/backfill");
    let wl_path = env_or("WALLETS_FILE", &format!("{ds}/backfill-wallets.txt"));
    let out_path = env_or("OUT_FILE", &format!("{ds}/transfers.csv"));
    let prog_path = env_or("PROG_FILE", &format!("{ds}/_progress.csv"));
    let concurrency: usize = env_or("CONCURRENCY", "4").parse().unwrap_or(4);
    let max_pages: u32 = env_or("MAX_PAGES", "100000").parse().unwrap_or(100_000);
    let limit: usize = env_or("LIMIT_WALLETS", "0").parse().unwrap_or(0);
    let es_keys: Vec<String> = env_or("ETHERSCAN_API_KEYS", "")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let es_mode = !es_keys.is_empty();

    let mut wallets: Vec<String> = std::fs::read_to_string(&wl_path).unwrap_or_default()
        .lines().map(|l| l.trim().to_lowercase()).filter(|l| l.starts_with("0x") && l.len() == 42).collect();
    if limit > 0 && wallets.len() > limit { wallets.truncate(limit); }
    // CHAINS=eth,arbitrum — звузити прогін до потрібних мереж; порожнє = усі 24
    let only: HashSet<String> = env_or("CHAINS", "").split(',')
        .map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let chains: Vec<&str> = CHAINS.iter().copied()
        .filter(|c| only.is_empty() || only.contains(*c)).collect();

    // replay progress: key -> (status, page, query, rows)
    let mut state: HashMap<String, (String, u32, String, u64)> = HashMap::new();
    if let Ok(p) = std::fs::read_to_string(&prog_path) {
        for line in p.lines() {
            let f: Vec<&str> = line.splitn(6, ',').collect();
            if f.len() < 4 { continue; }
            let key = format!("{}:{}", f[0], f[1]);
            let rows: u64 = f[2].parse().unwrap_or(0);
            let status = f[3].to_string();
            let page: u32 = f.get(4).and_then(|x| x.parse().ok()).unwrap_or(0);
            let query = f.get(5).map(|s| s.to_string()).unwrap_or_else(|| "type=ERC-20".to_string());
            state.insert(key, (status, page, query, rows));
        }
    }

    let new_file = std::fs::metadata(&out_path).map(|m| m.len() == 0).unwrap_or(true);
    let txf = std::fs::OpenOptions::new().create(true).append(true).open(&out_path).expect("open out");
    let mut txw = std::io::BufWriter::new(txf);
    if new_file { let _ = writeln!(txw, "wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash"); let _ = txw.flush(); }
    let txw: Wr = Arc::new(Mutex::new(txw));
    let pgf = std::fs::OpenOptions::new().create(true).append(true).open(&prog_path).expect("open prog");
    let pgw: Wr = Arc::new(Mutex::new(std::io::BufWriter::new(pgf)));

    let mut tasks: Vec<(String, String, u32, String, u64)> = Vec::new();
    let (mut resumed, mut done_ct) = (0usize, 0usize);
    for w in &wallets {
        for c in &chains {
            let key = format!("{w}:{c}");
            match state.get(&key) {
                Some((st, _, _, _)) if st == "done" => done_ct += 1,
                Some((_, page, query, rows)) => { if *page > 0 || query.starts_with("es:") { resumed += 1; } tasks.push((w.clone(), c.to_string(), *page, query.clone(), *rows)); }
                None => tasks.push((w.clone(), c.to_string(), 0, "type=ERC-20".to_string(), 0)),
            }
        }
    }
    let total = tasks.len();
    eprintln!("[backfill] режим={} ключів={}; {} wallets x {} chains; {} pending ({} done, {} resume); concurrency={}",
        if es_mode { "ETHERSCAN+bs-фолбек" } else { "Blockscout" }, es_keys.len(), wallets.len(), chains.len(), total, done_ct, resumed, concurrency);

    // Keep-alive УВІМКНЕНО (раніше pool_max_idle_per_host(0) вбивав reuse — кожен запит новий
    // TLS-хендшейк ~100-300мс; при десятках паралельних запитів на ОДИН хост api.etherscan.io це
    // й було стелею. Тепер з'єднання переюзуються per-host.
    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-backfill/0.2")
        .connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(32).pool_idle_timeout(Duration::from_secs(45))
        .build().expect("http client"));
    let counter = Arc::new(AtomicUsize::new(0));
    let key_ctr = Arc::new(AtomicUsize::new(0));
    let health = Arc::new(Mutex::new(HashMap::<String, i32>::new()));
    let es_keys = Arc::new(es_keys);

    // Два НЕЗАЛЕЖНІ пули: швидкі Etherscan-пари й повільні Blockscout-пари не конкурують за
    // слоти. Раніше один пул — щільна BS-пагінація (120мс/стор + таймаути) блокувала слоти й
    // тримала загальну пропускну на плато; тепер BS має власний бюджет, ES жене на повну.
    let (es_tasks, bs_tasks): (Vec<_>, Vec<_>) = tasks.into_iter()
        .partition(|(_, c, _, _, _)| es_mode && etherscan_chainid(c).is_some());
    let bs_conc: usize = env_or("BS_CONCURRENCY", "32").parse().unwrap_or(32);
    eprintln!("[backfill] пули: ES={} пар @ conc={concurrency}, BS={} пар @ conc={bs_conc}", es_tasks.len(), bs_tasks.len());

    let es_fut = {
        let (client, txw, pgw, counter, health, key_ctr, es_keys) =
            (client.clone(), txw.clone(), pgw.clone(), counter.clone(), health.clone(), key_ctr.clone(), es_keys.clone());
        stream::iter(es_tasks).for_each_concurrent(concurrency, move |(w, c, page0, query0, rows0)| {
            run_one(w, c, page0, query0, rows0, client.clone(), txw.clone(), pgw.clone(),
                counter.clone(), health.clone(), key_ctr.clone(), es_keys.clone(), es_mode, max_pages, total)
        })
    };
    let bs_fut = {
        let (client, txw, pgw, counter, health, key_ctr, es_keys) =
            (client.clone(), txw.clone(), pgw.clone(), counter.clone(), health.clone(), key_ctr.clone(), es_keys.clone());
        stream::iter(bs_tasks).for_each_concurrent(bs_conc, move |(w, c, page0, query0, rows0)| {
            run_one(w, c, page0, query0, rows0, client.clone(), txw.clone(), pgw.clone(),
                counter.clone(), health.clone(), key_ctr.clone(), es_keys.clone(), es_mode, max_pages, total)
        })
    };
    tokio::join!(es_fut, bs_fut);

    eprintln!("[backfill] complete");
}
