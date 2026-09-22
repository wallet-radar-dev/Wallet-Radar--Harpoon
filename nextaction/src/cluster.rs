//! Крок 2 (ML): кластеризація КИТІВ за поведінкою з backfill/transfers.csv.
//!
//! Нульова адреса 0x0 виключена (mint/burn). Фічі price-free й робастні:
//!   ln(txns), n_tokens, n_chains, in_ratio (накопичувач↔роздавач), span_days,
//!   txns_per_token, eth_share. Z-стандартизація + k-means (детермінований seed).
//! Вихід: dataset/nextaction/whale-clusters.json — центроїди, мітки, приклади.

use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

const DAY: i64 = 86_400;
const K: usize = 6;
const ITERS: usize = 50;

struct W {
    txns: u64,
    ins: u64,
    outs: u64,
    tokens: HashSet<String>,
    chains: HashSet<String>,
    eth_txns: u64,
    first_day: i64,
    last_day: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let f = BufReader::new(
        File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"),
    );
    let mut ws: HashMap<String, W> = HashMap::new();
    for line in f.lines() {
        let Ok(line) = line else { continue };
        let mut left = line.splitn(6, ',');
        let wallet = left.next().unwrap_or("");
        if wallet == wr_store::addrs::zero() || wallet == "wallet" {
            continue;
        }
        let chain = left.next().unwrap_or("");
        let dir = left.next().unwrap_or("?");
        let ts = left.next().unwrap_or("");
        let token = left.next().unwrap_or("");
        if dir != "i" && dir != "o" {
            continue;
        }
        let day = match chrono::DateTime::parse_from_rfc3339(ts) {
            Ok(dt) => dt.timestamp() / DAY,
            Err(_) => continue,
        };
        let w = ws.entry(wallet.to_string()).or_insert_with(|| W {
            txns: 0,
            ins: 0,
            outs: 0,
            tokens: HashSet::new(),
            chains: HashSet::new(),
            eth_txns: 0,
            first_day: day,
            last_day: day,
        });
        w.txns += 1;
        if dir == "i" {
            w.ins += 1;
        } else {
            w.outs += 1;
        }
        w.tokens.insert(format!("{chain}\t{token}"));
        w.chains.insert(chain.to_string());
        if chain == "eth" {
            w.eth_txns += 1;
        }
        w.first_day = w.first_day.min(day);
        w.last_day = w.last_day.max(day);
    }

    // фічі (тільки кити з мінімальною активністю)
    let mut ids: Vec<String> = Vec::new();
    let mut feats: Vec<[f64; 7]> = Vec::new();
    for (addr, w) in &ws {
        if w.txns < 5 {
            continue;
        }
        let ntok = w.tokens.len().max(1) as f64;
        feats.push([
            (w.txns as f64).ln(),
            w.tokens.len() as f64,
            w.chains.len() as f64,
            w.ins as f64 / (w.ins + w.outs).max(1) as f64,
            (w.last_day - w.first_day) as f64,
            w.txns as f64 / ntok,
            w.eth_txns as f64 / w.txns.max(1) as f64,
        ]);
        ids.push(addr.clone());
    }
    let n = feats.len();
    eprintln!("[cluster] китів з txns>=5: {n} (усього адрес: {})", ws.len());
    if n < K {
        eprintln!("[cluster] замало точок");
        return;
    }

    // z-стандартизація
    let mut mean = [0.0f64; 7];
    let mut sd = [0.0f64; 7];
    for f in &feats {
        for j in 0..7 {
            mean[j] += f[j];
        }
    }
    for m in &mut mean {
        *m /= n as f64;
    }
    for f in &feats {
        for j in 0..7 {
            sd[j] += (f[j] - mean[j]).powi(2);
        }
    }
    for s in &mut sd {
        *s = (*s / n as f64).sqrt().max(1e-9);
    }
    let z: Vec<[f64; 7]> = feats
        .iter()
        .map(|f| {
            let mut o = [0.0; 7];
            for j in 0..7 {
                o[j] = (f[j] - mean[j]) / sd[j];
            }
            o
        })
        .collect();

    // k-means, детермінований seed (рівномірно рознесені точки)
    let mut cent: Vec<[f64; 7]> = (0..K).map(|i| z[i * n / K]).collect();
    let mut assign = vec![0usize; n];
    for _ in 0..ITERS {
        for i in 0..n {
            let (mut best, mut bd) = (0usize, f64::MAX);
            for (c, ct) in cent.iter().enumerate() {
                let mut d = 0.0;
                for j in 0..7 {
                    d += (z[i][j] - ct[j]).powi(2);
                }
                if d < bd {
                    bd = d;
                    best = c;
                }
            }
            assign[i] = best;
        }
        let mut sum = vec![[0.0f64; 7]; K];
        let mut cnt = vec![0u64; K];
        for i in 0..n {
            cnt[assign[i]] += 1;
            for j in 0..7 {
                sum[assign[i]][j] += z[i][j];
            }
        }
        for c in 0..K {
            if cnt[c] > 0 {
                for j in 0..7 {
                    cent[c][j] = sum[c][j] / cnt[c] as f64;
                }
            }
        }
    }

    // інтерпретація (в оригінальних одиницях) + приклади
    let mut clusters = Vec::new();
    for c in 0..K {
        let members: Vec<usize> = (0..n).filter(|&i| assign[i] == c).collect();
        if members.is_empty() {
            continue;
        }
        let mut avg = [0.0f64; 7];
        for &i in &members {
            for j in 0..7 {
                avg[j] += feats[i][j];
            }
        }
        for a in &mut avg {
            *a /= members.len() as f64;
        }
        let mut ex: Vec<(&String, f64)> =
            members.iter().map(|&i| (&ids[i], feats[i][0])).collect();
        ex.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let examples: Vec<String> = ex.iter().take(4).map(|(a, _)| (*a).clone()).collect();
        let side = if avg[3] > 0.58 {
            "накопичувачі"
        } else if avg[3] < 0.42 {
            "роздавачі"
        } else {
            "збалансовані"
        };
        let breadth = if avg[1] > 40.0 {
            "широкий портфель"
        } else if avg[1] < 8.0 {
            "вузький портфель"
        } else {
            "середній портфель"
        };
        clusters.push(json!({
            "cluster": c,
            "size": members.len(),
            "label": format!("{side}, {breadth}"),
            "avg_txns": avg[0].exp().round(),
            "avg_tokens": (avg[1] * 10.0).round() / 10.0,
            "avg_chains": (avg[2] * 10.0).round() / 10.0,
            "in_ratio": (avg[3] * 100.0).round() / 100.0,
            "avg_span_days": avg[4].round(),
            "txns_per_token": (avg[5] * 10.0).round() / 10.0,
            "eth_share": (avg[6] * 100.0).round() / 100.0,
            "examples": examples,
        }));
    }
    clusters.sort_by_key(|c| std::cmp::Reverse(c["size"].as_u64().unwrap_or(0)));

    let out = json!({
        "generated": now(),
        "k": K,
        "whales": n,
        "features": ["ln_txns","n_tokens","n_chains","in_ratio","span_days","txns_per_token","eth_share"],
        "clusters": clusters,
    });
    let path = format!("{root}/dataset/nextaction/whale-clusters.json");
    fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).expect("write");
    eprintln!("[cluster] -> {path}");
    println!("=== КЛАСТЕРИ КИТІВ (k={K}, {n} китів) ===");
    for c in out["clusters"].as_array().unwrap() {
        println!(
            "#{} n={:<5} {:<34} | txns~{:<6} tokens~{:<6} chains~{} in_ratio={} span={}d eth={}%",
            c["cluster"].as_u64().unwrap(),
            c["size"].as_u64().unwrap(),
            c["label"].as_str().unwrap(),
            c["avg_txns"],
            c["avg_tokens"],
            c["avg_chains"],
            c["in_ratio"],
            c["avg_span_days"],
            c["eth_share"],
        );
    }
}
