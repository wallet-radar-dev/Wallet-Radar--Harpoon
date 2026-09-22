//! Per-wallet feature extraction for the Wallet Radar ML pilot.
//!
//! Streams the thin backfill CSV (`dataset/backfill/transfers.csv`) in a single
//! pass and emits one feature row per wallet to `dataset/backfill/features.csv`.
//! Pure std, low memory (aggregates keyed by wallet; ~hundreds of wallets).
//!
//! Input schema:  wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash
//! `dir`: i = incoming, o = outgoing, ? = third-party leg.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, BufWriter, Write};

/// Curated stablecoin symbols → treated as `stable` in the sector mix. Kept tiny
/// on purpose; the point is a behavioural signal, not exhaustive labelling.
fn is_stable(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.starts_with("USD")
        || s.ends_with("USD")
        || matches!(s.as_str(), "DAI" | "GHO" | "FRAX" | "TUSD" | "PYUSD" | "USDE" | "USDS" | "RLUSD" | "CRVUSD" | "EURC" | "EURS")
}

#[derive(Default)]
struct Acc {
    n: u64,
    n_in: u64,
    n_out: u64,
    n_stable: u64,
    chains: HashMap<String, u64>,
    tokens: HashMap<String, u64>,
    days: HashSet<u32>, // YYYYMMDD packed
    first: String,
    last: String,
}

fn day_key(ts: &str) -> Option<u32> {
    // ts like "2018-02-12T03:04:05.000000Z" → 20180212
    let b = ts.as_bytes();
    if b.len() < 10 {
        return None;
    }
    let d = |i: usize| (b[i] as char).to_digit(10);
    let y = d(0)? * 1000 + d(1)? * 100 + d(2)? * 10 + d(3)?;
    let m = d(5)? * 10 + d(6)?;
    let day = d(8)? * 10 + d(9)?;
    Some(y * 10000 + m * 100 + day)
}

fn entropy(counts: impl Iterator<Item = u64>, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let mut h = 0.0;
    for c in counts {
        if c == 0 {
            continue;
        }
        let p = c as f64 / total as f64;
        h -= p * p.log2();
    }
    h
}

fn days_between(a: u32, b: u32) -> f64 {
    // crude day-count from packed YYYYMMDD (good enough for a lifespan feature).
    let unpack = |x: u32| -> i64 {
        let y = (x / 10000) as i64;
        let m = ((x / 100) % 100) as i64;
        let d = (x % 100) as i64;
        (y * 365) + (m * 30) + d
    };
    (unpack(b) - unpack(a)).max(0) as f64
}

fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let inp = format!("{root}/dataset/backfill/transfers.csv");
    let outp = format!("{root}/dataset/backfill/features.csv");

    let f = std::fs::File::open(&inp).expect("open transfers.csv");
    let rdr = BufReader::with_capacity(1 << 20, f);
    let mut map: HashMap<String, Acc> = HashMap::new();
    let mut rows = 0u64;

    for line in rdr.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let p: Vec<&str> = line.splitn(9, ',').collect();
        if p.len() < 9 {
            continue;
        }
        let (wallet, chain, dir, ts, token, symbol) = (p[0], p[1], p[2], p[3], p[4], p[5]);
        if wallet == "wallet" {
            continue; // header
        }
        rows += 1;
        let a = map.entry(wallet.to_string()).or_default();
        a.n += 1;
        match dir {
            "i" => a.n_in += 1,
            "o" => a.n_out += 1,
            _ => {}
        }
        if is_stable(symbol) {
            a.n_stable += 1;
        }
        *a.chains.entry(chain.to_string()).or_insert(0) += 1;
        *a.tokens.entry(token.to_string()).or_insert(0) += 1;
        if let Some(dk) = day_key(ts) {
            a.days.insert(dk);
        }
        if a.first.is_empty() || ts < a.first.as_str() {
            a.first = ts.to_string();
        }
        if ts > a.last.as_str() {
            a.last = ts.to_string();
        }
    }

    let of = std::fs::File::create(&outp).expect("create features.csv");
    let mut w = BufWriter::new(of);
    writeln!(
        w,
        "wallet,n,n_in,n_out,in_ratio,stable_frac,n_chains,primary_chain,primary_chain_share,chain_entropy,n_tokens,top_token_share,token_entropy,active_days,lifespan_days,tx_per_active_day,first_ts,last_ts"
    )
    .unwrap();

    let mut wallets: Vec<(&String, &Acc)> = map.iter().collect();
    wallets.sort_by(|a, b| b.1.n.cmp(&a.1.n));

    for (wallet, a) in wallets {
        let in_ratio = if a.n > 0 { a.n_in as f64 / a.n as f64 } else { 0.0 };
        let stable_frac = if a.n > 0 { a.n_stable as f64 / a.n as f64 } else { 0.0 };
        let n_chains = a.chains.len();
        let (primary_chain, pc_count) = a
            .chains
            .iter()
            .max_by_key(|(_, c)| **c)
            .map(|(k, v)| (k.clone(), *v))
            .unwrap_or_default();
        let pc_share = if a.n > 0 { pc_count as f64 / a.n as f64 } else { 0.0 };
        let chain_h = entropy(a.chains.values().copied(), a.n);
        let n_tokens = a.tokens.len();
        let top_tok = a.tokens.values().copied().max().unwrap_or(0);
        let top_tok_share = if a.n > 0 { top_tok as f64 / a.n as f64 } else { 0.0 };
        let tok_h = entropy(a.tokens.values().copied(), a.n);
        let active_days = a.days.len();
        let lifespan = match (day_key(&a.first), day_key(&a.last)) {
            (Some(x), Some(y)) => days_between(x, y),
            _ => 0.0,
        };
        let tx_per_day = if active_days > 0 { a.n as f64 / active_days as f64 } else { 0.0 };
        writeln!(
            w,
            "{wallet},{},{},{},{:.4},{:.4},{},{},{:.4},{:.4},{},{:.4},{:.4},{},{:.0},{:.3},{},{}",
            a.n, a.n_in, a.n_out, in_ratio, stable_frac, n_chains, primary_chain, pc_share,
            chain_h, n_tokens, top_tok_share, tok_h, active_days, lifespan, tx_per_day,
            &a.first, &a.last
        )
        .unwrap();
    }
    w.flush().unwrap();
    eprintln!("[features] {} rows -> {} wallets -> {}", rows, map.len(), outp);
}
