//! Standalone full-history ERC-20 backfill worker for Wallet Radar.
//!
//! Reuses the Blockscout v2 pagination that `rust-server/src/bs.rs::transfers_rich`
//! uses (host table + `next_page_params` cursor), but WITHOUT the 500 / 3-page caps
//! the live server applies — so it pulls a wallet's *entire* ERC-20 transfer history
//! per chain. Isolated from the prod crate: its own tiny crate, own process, so a
//! weeks-long run never touches the live server's RAM or binary.
//!
//! Output: dataset/backfill/transfers.csv  (thin schema, loaded into DuckDB later)
//! State:  dataset/backfill/_progress.csv  (append-only checkpoint log)
//!
//! Streaming + resumable: rows are flushed to disk PER PAGE (never buffered per
//! wallet — a whale with 500k transfers must not sit in RAM), and a cursor
//! checkpoint is written every few pages, so a restart continues a wallet from
//! the middle instead of re-crawling it from page 0.
//!
//! Progress line format (append-only, last line per pair wins):
//!   wallet,chain,rows_cum,status,page,query
//!   status: part | done | err:... | skip:dead    (commas in status -> ';')
//!   query : the cursor querystring to fetch page `page` (resume point)
//!
//! Env: APP_ROOT (default /opt/wallet-tracker), CONCURRENCY (4), MAX_PAGES (100000),
//!      LIMIT_WALLETS (0 = all).

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use serde_json::Value;
use tokio::sync::Mutex;

type Wr = Arc<Mutex<std::io::BufWriter<std::fs::File>>>;

/// chain id -> Blockscout base URL. Copied verbatim from bs.rs::host() (source of truth).
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

const CHAINS: &[&str] = &[
    "eth", "base", "optimism", "gnosis", "polygon", "arbitrum", "zksync", "scroll", "mode",
    "fraxtal", "blast", "linea", "zora", "celo", "mantle", "sonic", "rootstock", "fuse",
    "metis", "aurora", "lisk", "redstone", "immutable", "lukso",
];

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

/// Strip characters that would break a bare CSV field (only `symbol` is free text).
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c == ',' || c == '"' || c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

fn decimals_of(it: &Value) -> f64 {
    let pick = |v: &Value| -> Option<f64> {
        v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64())
    };
    pick(&it["total"]["decimals"])
        .or_else(|| pick(&it["token"]["decimals"]))
        .unwrap_or(0.0)
}

/// One Blockscout v2 token-transfer item -> one thin CSV line, from `wallet`'s POV.
fn row(wallet: &str, chain: &str, it: &Value) -> Option<String> {
    let from = it["from"]["hash"].as_str().unwrap_or("").to_lowercase();
    let to = it["to"]["hash"].as_str().unwrap_or("").to_lowercase();
    if from.is_empty() && to.is_empty() {
        return None;
    }
    let dir = if from == wallet {
        "o"
    } else if to == wallet {
        "i"
    } else {
        "?"
    };
    let counterparty = if dir == "o" { &to } else { &from };

    let token = {
        let a = it["token"]["address"].as_str().unwrap_or("");
        if a.is_empty() {
            it["token"]["address_hash"].as_str().unwrap_or("")
        } else {
            a
        }
    }
    .to_lowercase();
    let symbol = sanitize(it["token"]["symbol"].as_str().unwrap_or(""));

    let dec = decimals_of(it);
    let raw = it["total"]["value"].as_str().unwrap_or("0");
    let amount = raw.parse::<f64>().unwrap_or(0.0) / 10f64.powf(dec);

    let ts = it["timestamp"].as_str().unwrap_or("");
    let hash = it["transaction_hash"]
        .as_str()
        .or_else(|| it["tx_hash"].as_str())
        .unwrap_or("");

    Some(format!(
        "{wallet},{chain},{dir},{ts},{token},{symbol},{counterparty},{amount},{hash}"
    ))
}

/// Append one checkpoint line to _progress.csv. `status` commas are neutralised so the
/// field count stays fixed; `query` is the last field and may hold `&`/`=` freely.
async fn write_ckpt(
    pgw: &Wr,
    wallet: &str,
    chain: &str,
    rows: u64,
    status: &str,
    page: u32,
    query: &str,
) {
    let status = status.replace(',', ";");
    let mut g = pgw.lock().await;
    let _ = writeln!(*g, "{wallet},{chain},{rows},{status},{page},{query}");
    let _ = g.flush();
}

/// Build the next-page querystring from a Blockscout `next_page_params` object.
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

/// Full ERC-20 history for one (wallet, chain). Streams each page's rows straight to
/// `txw` (never buffers the whole wallet), checkpoints the cursor every 20 pages, and
/// resumes from (`start_page`, `start_query`). Returns (cumulative_rows, terminal_status).
/// Writes its own terminal checkpoint (done / err:...) before returning.
async fn fetch_pair(
    client: &reqwest::Client,
    chain: &str,
    wallet: &str,
    max_pages: u32,
    start_page: u32,
    start_query: &str,
    start_rows: u64,
    txw: &Wr,
    pgw: &Wr,
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
        if page % 25 == 0 {
            eprintln!("  fetch {short} {chain} p{page} rows={rows_total}");
        }
        let mut attempt = 0u32;
        let d: Value = loop {
            attempt += 1;
            // Hard per-request wall-clock cap via tokio::time::timeout, backed by the
            // reqwest client's own connect/read timeouts. A hung obscure explorer must
            // never freeze a worker forever.
            let sent = tokio::time::timeout(
                Duration::from_secs(20),
                client.get(&url).header("accept", "application/json").send(),
            )
            .await;
            let backoff_ms: u64 = match sent {
                Err(_elapsed) => {
                    if attempt >= 4 {
                        write_ckpt(pgw, wallet, chain, rows_total, "err:req:timeout", page, &query).await;
                        return (rows_total, "err:req:timeout".to_string());
                    }
                    800 * attempt as u64
                }
                Ok(Ok(r)) if r.status().is_success() => {
                    match tokio::time::timeout(Duration::from_secs(20), r.json::<Value>()).await {
                        Ok(Ok(v)) => break v,
                        _ => {
                            if attempt >= 6 {
                                write_ckpt(pgw, wallet, chain, rows_total, "err:json:read", page, &query).await;
                                return (rows_total, "err:json:read".to_string());
                            }
                            800
                        }
                    }
                }
                Ok(Ok(r)) => {
                    let code = r.status().as_u16();
                    // 404 = this explorer has no v2 for the chain: treat as "no data".
                    if code == 404 {
                        write_ckpt(pgw, wallet, chain, rows_total, "done", page, &query).await;
                        return (rows_total, "done".to_string());
                    }
                    // 403 = explorer blocks the API for this host: stable, give up fast.
                    if code == 403 {
                        write_ckpt(pgw, wallet, chain, rows_total, "err:http:403", page, &query).await;
                        return (rows_total, "err:http:403".to_string());
                    }
                    if attempt >= 8 {
                        let s = format!("err:http:{code}");
                        write_ckpt(pgw, wallet, chain, rows_total, &s, page, &query).await;
                        return (rows_total, s);
                    }
                    // 429 rate limit: back off hard (2.5s,5s,7.5s…). 5xx: shorter.
                    if code == 429 {
                        2500 * attempt as u64
                    } else {
                        800 * attempt as u64
                    }
                }
                Ok(Err(e)) => {
                    if attempt >= 4 {
                        let s = format!("err:req:{e}");
                        write_ckpt(pgw, wallet, chain, rows_total, &s, page, &query).await;
                        return (rows_total, s);
                    }
                    800 * attempt as u64
                }
            };
            eprintln!("  retry {short} {chain} p{page} a{attempt} bo={backoff_ms}ms");
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        };

        // Stream this page's rows straight to disk — never buffer the whole wallet.
        let items = d["items"].as_array().cloned().unwrap_or_default();
        {
            let mut g = txw.lock().await;
            for it in &items {
                if let Some(r) = row(wallet, chain, it) {
                    let _ = writeln!(*g, "{r}");
                    rows_total += 1;
                }
            }
            let _ = g.flush();
        }

        let next = &d["next_page_params"];
        page += 1;
        let q = next_query(next);

        // Natural end of history, or configured page ceiling.
        if items.is_empty() || next.is_null() || page >= max_pages {
            write_ckpt(pgw, wallet, chain, rows_total, "done", page, &q).await;
            return (rows_total, "done".to_string());
        }
        // Cursor-loop guard: a buggy explorer that keeps handing back the same cursor
        // would otherwise spin to max_pages writing duplicate rows. Stop if it stalls.
        if q == query || q == prev_q {
            write_ckpt(pgw, wallet, chain, rows_total, "done", page, &q).await;
            return (rows_total, "done".to_string());
        }
        prev_q = std::mem::replace(&mut query, q);

        // Resume checkpoint every 20 pages, so a restart continues mid-wallet.
        if page % 20 == 0 {
            write_ckpt(pgw, wallet, chain, rows_total, "part", page, &query).await;
        }

        // Politeness: keyless public Blockscout — small gap between pages.
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 6)]
async fn main() {
    let root = env_or("APP_ROOT", "/opt/wallet-tracker");
    let ds = format!("{root}/dataset/backfill");
    let wl_path = format!("{ds}/backfill-wallets.txt");
    let out_path = format!("{ds}/transfers.csv");
    let prog_path = format!("{ds}/_progress.csv");
    let concurrency: usize = env_or("CONCURRENCY", "4").parse().unwrap_or(4);
    let max_pages: u32 = env_or("MAX_PAGES", "100000").parse().unwrap_or(100_000);
    let limit: usize = env_or("LIMIT_WALLETS", "0").parse().unwrap_or(0);

    let mut wallets: Vec<String> = std::fs::read_to_string(&wl_path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| l.starts_with("0x") && l.len() == 42)
        .collect();
    if limit > 0 && wallets.len() > limit {
        wallets.truncate(limit);
    }

    // Replay the append-only checkpoint log: last line per (wallet,chain) wins.
    // state: key -> (status, page, query, rows_cum)
    let mut state: HashMap<String, (String, u32, String, u64)> = HashMap::new();
    if let Ok(p) = std::fs::read_to_string(&prog_path) {
        for line in p.lines() {
            let f: Vec<&str> = line.splitn(6, ',').collect();
            if f.len() < 4 {
                continue;
            }
            let key = format!("{}:{}", f[0], f[1]);
            let rows: u64 = f[2].parse().unwrap_or(0);
            let status = f[3].to_string();
            let page: u32 = f.get(4).and_then(|x| x.parse().ok()).unwrap_or(0);
            let query = f
                .get(5)
                .map(|s| s.to_string())
                .unwrap_or_else(|| "type=ERC-20".to_string());
            state.insert(key, (status, page, query, rows));
        }
    }

    let new_file = std::fs::metadata(&out_path).map(|m| m.len() == 0).unwrap_or(true);
    let txf = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out_path)
        .expect("open transfers.csv");
    let mut txw = std::io::BufWriter::new(txf);
    if new_file {
        let _ = writeln!(txw, "wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash");
        let _ = txw.flush();
    }
    let txw: Wr = Arc::new(Mutex::new(txw));

    let pgf = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&prog_path)
        .expect("open _progress.csv");
    let pgw: Wr = Arc::new(Mutex::new(std::io::BufWriter::new(pgf)));

    // Build the work list. `done` pairs are skipped; `part`/`err`/interrupted pairs
    // resume from their last checkpoint (page + cursor); unseen pairs start fresh.
    let mut tasks: Vec<(String, String, u32, String, u64)> = Vec::new();
    let mut resumed = 0usize;
    let mut done_ct = 0usize;
    for w in &wallets {
        for c in CHAINS {
            let key = format!("{w}:{c}");
            match state.get(&key) {
                Some((st, _, _, _)) if st == "done" => {
                    done_ct += 1;
                }
                Some((_, page, query, rows)) => {
                    if *page > 0 {
                        resumed += 1;
                    }
                    tasks.push((w.clone(), c.to_string(), *page, query.clone(), *rows));
                }
                None => {
                    tasks.push((w.clone(), c.to_string(), 0, "type=ERC-20".to_string(), 0));
                }
            }
        }
    }
    let total = tasks.len();
    eprintln!(
        "[backfill] {} wallets x {} chains; {} pairs pending ({} done, {} resuming mid-wallet); concurrency={} max_pages={}",
        wallets.len(),
        CHAINS.len(),
        total,
        done_ct,
        resumed,
        concurrency,
        max_pages
    );

    let client = Arc::new(
        reqwest::Client::builder()
            .user_agent("wallet-radar-backfill/0.1")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(0)
            .build()
            .expect("http client"),
    );
    let counter = Arc::new(AtomicUsize::new(0));
    // Per-chain structural-failure count. A chain whose explorer is gone (won't connect,
    // or hard-403) gets probed a few times, then skipped for every remaining wallet — so a
    // weeks-long run doesn't pay timeouts on dead chains 500x over. A clean run resets it.
    const DEAD: i32 = 5;
    let health = Arc::new(Mutex::new(HashMap::<String, i32>::new()));

    stream::iter(tasks)
        .for_each_concurrent(concurrency, |(w, c, page0, query0, rows0)| {
            let client = client.clone();
            let txw = txw.clone();
            let pgw = pgw.clone();
            let counter = counter.clone();
            let health = health.clone();
            async move {
                // Skip chains already deemed dead this run.
                if health.lock().await.get(&c).copied().unwrap_or(0) >= DEAD {
                    write_ckpt(&pgw, &w, &c, 0, "skip:dead", 0, "type=ERC-20").await;
                    let i = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!("[{i}/{total}] {} {:9} -> skip:dead", &w[..10.min(w.len())], c);
                    return;
                }
                let (n, status) =
                    fetch_pair(&client, &c, &w, max_pages, page0, &query0, rows0, &txw, &pgw).await;
                // Only a chain that won't CONNECT (req:) or hard-blocks (403) counts toward
                // "dead". 429/5xx/json prove the explorer is alive, just throttling us.
                if status == "done" {
                    *health.lock().await.entry(c.clone()).or_insert(0) = 0;
                } else if status.starts_with("err:req:") || status == "err:http:403" {
                    *health.lock().await.entry(c.clone()).or_insert(0) += 1;
                }
                // fetch_pair already wrote the terminal checkpoint — don't double-write.
                let i = counter.fetch_add(1, Ordering::Relaxed) + 1;
                let tag = if status == "done" { "" } else { status.as_str() };
                eprintln!("[{i}/{total}] {} {:9} -> {:8} {}", &w[..10.min(w.len())], c, n, tag);
            }
        })
        .await;

    eprintln!("[backfill] complete");
}
