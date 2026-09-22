//! Token total-supply collector (Phase B) — unblocks the "share of emission"
//! metric: a wallet/family's holding as a fraction of a token's total supply.
//!
//! Reads the whale-asset contracts, fetches each token's total_supply + decimals
//! from Blockscout, and writes dataset/token-supply.json  {contract: {supply,
//! decimals, symbol}}. Resumable (skips contracts already collected), gentle on
//! the shared Blockscout quota so it does not starve the running backfill.
//!
//! Env: APP_ROOT (/opt/wallet-tracker), CONCURRENCY (2), LIMIT (0=all).

use std::collections::HashMap;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use serde_json::{json, Value};

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .user_agent("wallet-radar-supply/0.1")
        .build()
        .unwrap()
}

/// Blockscout token info by contract (eth). `total_supply` is the raw integer
/// string; `decimals` scales it. None on 429 / miss (never cached as an answer).
async fn fetch_supply(c: &reqwest::Client, contract: &str) -> Option<(String, i64, String)> {
    let url = format!("https://eth.blockscout.com/api/v2/tokens/{contract}");
    for attempt in 1..=4u32 {
        match tokio::time::timeout(Duration::from_secs(20), c.get(&url).header("accept", "application/json").send()).await {
            Ok(Ok(r)) if r.status().is_success() => {
                let v: Value = r.json().await.ok()?;
                let supply = v["total_supply"].as_str().unwrap_or("").to_string();
                if supply.is_empty() {
                    return None;
                }
                let dec = v["decimals"].as_str().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                let sym = v["symbol"].as_str().unwrap_or("").to_string();
                return Some((supply, dec, sym));
            }
            Ok(Ok(r)) if r.status().as_u16() == 404 => return None, // no such token on eth
            _ => {
                tokio::time::sleep(Duration::from_millis(600 * attempt as u64)).await;
            }
        }
    }
    None
}

#[tokio::main]
async fn main() {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    let conc: usize = std::env::var("CONCURRENCY").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
    let limit: usize = std::env::var("LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let outp = format!("{root}/dataset/token-supply.json");

    // resume: load whatever is already collected
    let mut have: HashMap<String, Value> = std::fs::read_to_string(&outp)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|o| o.into_iter().collect())
        .unwrap_or_default();

    // targets: whale-asset contracts not yet collected
    let wa: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{root}/dataset/cmc-whale-assets.json")).unwrap_or_else(|_| "{}".into()),
    )
    .unwrap_or(Value::Null);
    let mut todo: Vec<(String, String)> = Vec::new(); // (contract, symbol)
    for a in wa["assets"].as_array().cloned().unwrap_or_default() {
        let contract = a["contract"].as_str().unwrap_or("").to_lowercase();
        if contract.len() != 42 || have.contains_key(&contract) {
            continue;
        }
        todo.push((contract, a["symbol"].as_str().unwrap_or("").to_string()));
    }
    if limit > 0 && todo.len() > limit {
        todo.truncate(limit);
    }
    eprintln!("[supply] {} already have; fetching {} contracts (conc={conc})", have.len(), todo.len());

    let client = http();
    let results: Vec<(String, Option<(String, i64, String)>)> = stream::iter(todo)
        .map(|(contract, _sym)| {
            let client = client.clone();
            async move {
                let r = fetch_supply(&client, &contract).await;
                tokio::time::sleep(Duration::from_millis(300)).await; // politeness
                (contract, r)
            }
        })
        .buffer_unordered(conc)
        .collect()
        .await;

    let mut added = 0;
    for (contract, r) in results {
        if let Some((supply, dec, sym)) = r {
            have.insert(contract, json!({"supply": supply, "decimals": dec, "symbol": sym}));
            added += 1;
        }
    }

    let obj: serde_json::Map<String, Value> = have.into_iter().collect();
    let tmp = format!("{outp}.tmp");
    std::fs::write(&tmp, serde_json::to_string(&Value::Object(obj.clone())).unwrap()).unwrap();
    std::fs::rename(&tmp, &outp).unwrap();
    eprintln!("[supply] +{added} collected, {} total -> {}", obj.len(), outp);
}
