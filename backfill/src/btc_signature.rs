//! Rust-порт `deploy/btc-signature.py`. BTC підпис-детектор (накоп 2022 / розпод з 2024-11) +
//! signature-based expansion. I/O ІДЕНТИЧНА python: читає dataset/backfill/transfers-btc.csv,
//! пише dataset/nextaction/btc-group.json, дописує whale-counterparty у btc-wallets.txt
//! (баланс через Esplora, keep-alive). Env: ACC_FROM ACC_TO DIST_FROM MIN_ACC MIN_BTC EXPAND.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

async fn balance(client: &reqwest::Client, addr: &str) -> Option<(f64, u64)> {
    for base in ["https://mempool.space/api", "https://blockstream.info/api"] {
        let url = format!("{base}/address/{addr}");
        if let Ok(r) = client.get(&url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                let cs = &v["chain_stats"];
                let funded = cs["funded_txo_sum"].as_f64()?;
                let spent = cs["spent_txo_sum"].as_f64()?;
                let txc = cs["tx_count"].as_u64().unwrap_or(0);
                return Some(((funded - spent) / 1e8, txc));
            }
        }
    }
    None
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let acc_from = env("ACC_FROM", "2022-05");
    let acc_to = env("ACC_TO", "2023-03");
    let dist_from = env("DIST_FROM", "2024-11");
    let min_acc: f64 = env("MIN_ACC", "10").parse().unwrap_or(10.0);
    let min_btc: f64 = env("MIN_BTC", "50").parse().unwrap_or(50.0);
    let expand = env("EXPAND", "1") == "1";

    let tx = format!("{root}/dataset/backfill/transfers-btc.csv");
    let seed_path = format!("{root}/dataset/backfill/btc-wallets.txt");
    let out = format!("{root}/dataset/nextaction/btc-group.json");

    // per-address: (acc, dist, counterparty->amt)
    let f = BufReader::new(File::open(&tx).expect("transfers-btc.csv"));
    let mut per: HashMap<String, (f64, f64, HashMap<String, f64>)> = HashMap::new();
    for (i, line) in f.lines().enumerate() {
        let Ok(line) = line else { continue };
        if i == 0 {
            continue;
        }
        let p: Vec<&str> = line.split(',').collect();
        if p.len() < 9 {
            continue;
        }
        let (w, d, ts, cp, amt) = (p[0], p[2], p[3], p[6], p[7]);
        if ts.len() < 7 {
            continue;
        }
        let ym = &ts[..7];
        let a: f64 = amt.parse().unwrap_or(0.0);
        let e = per.entry(w.to_string()).or_insert((0.0, 0.0, HashMap::new()));
        if d == "i" && ym >= acc_from.as_str() && ym <= acc_to.as_str() {
            e.0 += a;
            if !cp.is_empty() {
                *e.2.entry(cp.to_string()).or_insert(0.0) += a;
            }
        } else if d == "o" && ym >= dist_from.as_str() {
            e.1 += a;
            if !cp.is_empty() {
                *e.2.entry(cp.to_string()).or_insert(0.0) += a;
            }
        }
    }

    // група: acc>=MIN_ACC
    let mut group: Vec<(String, f64, f64)> = per
        .iter()
        .filter(|(_, v)| v.0 >= min_acc)
        .map(|(w, v)| (w.clone(), v.0, v.1))
        .collect();
    group.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let mut cp_group: HashMap<String, f64> = HashMap::new();
    for (w, _, _) in &group {
        for (c, amt) in &per[w].2 {
            *cp_group.entry(c.clone()).or_insert(0.0) += amt;
        }
    }

    let res = json!({
        "pattern": format!("накопичення BTC {}..{} ($16-22k) -> розподіл з {} (~$90k)", acc_from, acc_to, dist_from),
        "members": group.len(),
        "totalAcc2022": group.iter().map(|g| g.1).sum::<f64>().round(),
        "totalDist": group.iter().map(|g| g.2).sum::<f64>().round(),
        "wallets": group.iter().take(200).map(|(w, a, dd)| json!({
            "addr": w, "acc2022": (a * 10.0).round() / 10.0, "dist9xk": (dd * 10.0).round() / 10.0
        })).collect::<Vec<_>>(),
    });
    fs::create_dir_all(format!("{root}/dataset/nextaction")).ok();
    fs::write(&out, serde_json::to_string_pretty(&res).unwrap()).expect("write");
    eprintln!(
        "=== ПІДПИС-ГРУПА: {} членів | acc-2022 {} BTC | dist {} BTC ===",
        group.len(),
        res["totalAcc2022"],
        res["totalDist"]
    );
    for (w, a, dd) in group.iter().take(15) {
        println!("  acc22={:>12.0}  dist={:>12.0}  {}", a, dd, w);
    }

    // signature-expansion
    if expand && !group.is_empty() {
        let seed: HashSet<String> = fs::read_to_string(&seed_path)
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        let mut cands: Vec<(String, f64)> = cp_group
            .into_iter()
            .filter(|(c, _)| !c.is_empty() && !seed.contains(c))
            .collect();
        cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        cands.truncate(100);
        let client = reqwest::Client::builder()
            .user_agent("wallet-radar-btc/0.1")
            .pool_max_idle_per_host(4)
            .build()
            .unwrap();
        let mut added = Vec::new();
        for (c, _) in &cands {
            if let Some((b, txc)) = balance(&client, c).await {
                if b >= min_btc && txc <= 50000 {
                    added.push(c.clone());
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
        eprintln!("додано whale-кандидатів у seed: {}", added.len());
    }
}
