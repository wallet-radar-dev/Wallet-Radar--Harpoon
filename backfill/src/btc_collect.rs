//! Rust-порт `deploy/btc-collect.py`. Нативний BTC-збір: Esplora (mempool/blockstream, без ключа)
//! per-адреса tx-історія -> UTXO у сумісну EVM-схему: wallet,btc,dir,ts,BTC,BTC,counterparty,amount,txid.
//! Резюмиться (пропускає адреси, вже в OUT). Env: MAX_TXS(1000) SLEEP_MS(250) OUT SEED.

use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

async fn get(client: &reqwest::Client, path: &str) -> Option<Value> {
    for base in ["https://mempool.space/api", "https://blockstream.info/api"] {
        for _ in 0..2 {
            if let Ok(r) = client.get(format!("{base}{path}")).send().await {
                if let Ok(v) = r.json::<Value>().await {
                    return Some(v);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }
    None
}

fn iso(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S.000000Z").to_string())
        .unwrap_or_default()
}

/// (ts, dir, amount_btc, counterparty, txid)
async fn addr_events(client: &reqwest::Client, addr: &str, max_txs: usize, sleep_ms: u64) -> Vec<(i64, char, f64, String, String)> {
    let mut out = Vec::new();
    let mut last: Option<String> = None;
    let mut total = 0usize;
    while total < max_txs {
        let path = match &last {
            Some(l) => format!("/address/{addr}/txs/chain/{l}"),
            None => format!("/address/{addr}/txs"),
        };
        let Some(txs) = get(client, &path).await else { break };
        let Some(arr) = txs.as_array() else { break };
        if arr.is_empty() {
            break;
        }
        for tx in arr {
            let Some(txid) = tx["txid"].as_str() else { continue };
            let Some(ts) = tx["status"]["block_time"].as_i64() else { continue };
            let empty = Vec::new();
            let vin = tx["vin"].as_array().unwrap_or(&empty);
            let vout = tx["vout"].as_array().unwrap_or(&empty);
            let mut vin_sum = 0i64;
            for v in vin {
                if v["prevout"]["scriptpubkey_address"].as_str() == Some(addr) {
                    vin_sum += v["prevout"]["value"].as_i64().unwrap_or(0);
                }
            }
            let mut vout_sum = 0i64;
            for v in vout {
                if v["scriptpubkey_address"].as_str() == Some(addr) {
                    vout_sum += v["value"].as_i64().unwrap_or(0);
                }
            }
            let net = vout_sum - vin_sum;
            if net == 0 {
                continue;
            }
            // counterparty: найбільша ІНША адреса з протилежного боку
            let cp = if net > 0 {
                vin.iter()
                    .filter_map(|v| {
                        let a = v["prevout"]["scriptpubkey_address"].as_str()?;
                        if a == addr { return None; }
                        Some((v["prevout"]["value"].as_i64().unwrap_or(0), a.to_string()))
                    })
                    .max_by_key(|(val, _)| *val)
                    .map(|(_, a)| a)
                    .unwrap_or_default()
            } else {
                vout.iter()
                    .filter_map(|v| {
                        let a = v["scriptpubkey_address"].as_str()?;
                        if a == addr { return None; }
                        Some((v["value"].as_i64().unwrap_or(0), a.to_string()))
                    })
                    .max_by_key(|(val, _)| *val)
                    .map(|(_, a)| a)
                    .unwrap_or_default()
            };
            let dir = if net > 0 { 'i' } else { 'o' };
            let amount = (net.abs() as f64) / 1e8;
            out.push((ts, dir, amount, cp, txid.to_string()));
            total += 1;
        }
        last = arr.last().and_then(|t| t["txid"].as_str()).map(String::from);
        if arr.len() < 25 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
    }
    out
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let seed_path = std::env::var("SEED").unwrap_or_else(|_| format!("{root}/dataset/backfill/btc-wallets.txt"));
    let out = std::env::var("OUT").unwrap_or_else(|_| format!("{root}/dataset/backfill/transfers-btc.csv"));
    let max_txs: usize = envn("MAX_TXS", 1000);
    let sleep_ms: u64 = envn("SLEEP_MS", 250);

    let addrs: Vec<String> = fs::read_to_string(&seed_path)
        .expect("seed")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let mut done: HashSet<String> = HashSet::new();
    let new_file = fs::metadata(&out).map(|m| m.len() == 0).unwrap_or(true);
    if !new_file {
        if let Ok(f) = File::open(&out) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if let Some(a) = line.split(',').next() {
                    done.insert(a.to_string());
                }
            }
        }
    }
    let mut w = OpenOptions::new().create(true).append(true).open(&out).expect("open out");
    if new_file {
        let _ = writeln!(w, "wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash");
    }
    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-btc/0.1")
        .pool_max_idle_per_host(4)
        .build()
        .unwrap();
    let mut ok = 0;
    for (i, a) in addrs.iter().enumerate() {
        if done.contains(a) {
            eprintln!("[{}/{}] {} skip", i + 1, addrs.len(), &a[..12.min(a.len())]);
            continue;
        }
        let evs = addr_events(&client, a, max_txs, sleep_ms).await;
        for (ts, dir, amount, cp, txid) in &evs {
            let _ = writeln!(w, "{a},btc,{dir},{},BTC,BTC,{cp},{amount:.8},{txid}", iso(*ts));
        }
        let _ = w.flush();
        ok += 1;
        eprintln!("[{}/{}] {} -> {} подій", i + 1, addrs.len(), &a[..14.min(a.len())], evs.len());
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
    }
    eprintln!("готово: {ok} нових адрес, файл {out}");
}
