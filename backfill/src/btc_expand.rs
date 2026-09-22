//! Rust-порт `deploy/btc-expand.py`. BTC whale-універсум graph-expansion: топ-counterparty за
//! обсягом → баланс-фільтр (Esplora) → додати whale (>=MIN_BTC, не біржа) у btc-wallets.txt.
//! Env: MIN_BTC(50) TOPN(120) HUB(200) MAX_TX(50000).

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

async fn balance(client: &reqwest::Client, addr: &str) -> Option<(f64, u64)> {
    for base in ["https://mempool.space/api", "https://blockstream.info/api"] {
        if let Ok(r) = client.get(format!("{base}/address/{addr}")).send().await {
            if let Ok(v) = r.json::<Value>().await {
                let cs = &v["chain_stats"];
                let f = cs["funded_txo_sum"].as_f64()?;
                let s = cs["spent_txo_sum"].as_f64()?;
                return Some(((f - s) / 1e8, cs["tx_count"].as_u64().unwrap_or(0)));
            }
        }
    }
    None
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_btc: f64 = envn("MIN_BTC", 50.0);
    let topn: usize = envn("TOPN", 120);
    let hub: usize = envn("HUB", 200);
    let max_tx: u64 = envn("MAX_TX", 50000);

    let seed_path = format!("{root}/dataset/backfill/btc-wallets.txt");
    let tx = format!("{root}/dataset/backfill/transfers-btc.csv");
    let seed: HashSet<String> = fs::read_to_string(&seed_path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let mut vol: HashMap<String, f64> = HashMap::new();
    let mut seen_from: HashMap<String, HashSet<String>> = HashMap::new();
    let f = BufReader::new(File::open(&tx).expect("transfers-btc.csv"));
    for (i, line) in f.lines().enumerate() {
        let Ok(line) = line else { continue };
        if i == 0 {
            continue;
        }
        let p: Vec<&str> = line.split(',').collect();
        if p.len() < 9 {
            continue;
        }
        let (w, cp, amt) = (p[0], p[6], p[7]);
        if cp.is_empty() || seed.contains(cp) {
            continue;
        }
        if let Ok(a) = amt.parse::<f64>() {
            *vol.entry(cp.to_string()).or_insert(0.0) += a;
            seen_from.entry(cp.to_string()).or_default().insert(w.to_string());
        }
    }
    let mut cands: Vec<(String, f64)> = vol.into_iter().collect();
    cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    cands.truncate(topn);
    eprintln!("кандидатів (топ-{topn} counterparty за обсягом): {}; перевіряю баланси...", cands.len());
    if std::env::var("DRY").is_ok() {
        for (c, v) in cands.iter().take(10) {
            println!("  cand {:.2}\t{}", v, c);
        }
        return;
    }

    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-btc/0.1")
        .pool_max_idle_per_host(4)
        .build()
        .unwrap();
    let mut added = Vec::new();
    for (c, _) in &cands {
        if seen_from.get(c).map(|s| s.len()).unwrap_or(0) > hub {
            continue;
        }
        if let Some((b, txc)) = balance(&client, c).await {
            if b >= min_btc && txc <= max_tx {
                added.push(c.clone());
                eprintln!("  + {} balance={:.1} BTC tx={}", &c[..16.min(c.len())], b, txc);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    if !added.is_empty() {
        let mut f = OpenOptions::new().append(true).open(&seed_path).unwrap();
        for c in &added {
            writeln!(f, "{c}").ok();
        }
    }
    eprintln!(
        "додано WHALE: {} (>= {} BTC); seed тепер {} адрес",
        added.len(),
        min_btc,
        seed.len() + added.len()
    );
}
