//! Port of server/balances.js `walletBalances` — a wallet's holdings across the
//! 15 highest-cap EVM chains: Blockscout token + native balances, with DefiLlama
//! repricing of anything Blockscout left unrated, aggregated by symbol.
//!
//! The fetch/reprice orchestration is `wallet_balances`; the deterministic
//! reduction (`aggregate_balances`) is golden-tested byte-for-byte against Node.
//! Every quirk is preserved: `totalUsd` sums the RAW per-entry USD then rounds
//! once (not the sum of rounded tokens); the `usd > 0 && usd < 1e9` spam gate;
//! per-symbol `chains` counts distinct chains; symbols keep first-seen order under
//! the stable value-desc sort.

use crate::jsmath::{js_num, js_round};
use futures::future::join_all;
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// balances.js `BAL_CHAINS` — the chains scanned, in order.
const BAL_CHAINS: [&str; 17] = [
    "eth", "base", "arbitrum", "optimism", "polygon", "gnosis", "zksync", "scroll", "mode", "zora", "linea", "blast", "mantle", "sonic",
    "celo", "bnb", "avalanche",
];

/// balances.js `add` — fold `usd` into `symbol`'s running total (skipping
/// non-positive), recording the contributing chain.
fn add(agg: &mut IndexMap<String, (f64, HashSet<String>)>, total: &mut f64, symbol: &str, usd: f64, chain: &str) {
    if !(usd > 0.0) {
        return;
    }
    let e = agg.entry(symbol.to_string()).or_insert((0.0, HashSet::new()));
    e.0 += usd;
    if !chain.is_empty() {
        e.1.insert(chain.to_string());
    }
    *total += usd;
}

/// balances.js reduction. `per_chain` is `[{c, toks:[{symbol,contract,balance,usd}], nat:{symbol,valueUsd}|null}]`;
/// `px` maps `"{chain}:{contract}" → price` for tokens Blockscout didn't rate.
pub fn aggregate_balances(per_chain: &[Value], px: &HashMap<String, f64>, chains_scanned: usize) -> Value {
    let mut agg: IndexMap<String, (f64, HashSet<String>)> = IndexMap::new();
    let mut total = 0.0f64;
    let empty = vec![];

    for entry in per_chain {
        let c = entry["c"].as_str().unwrap_or("");
        let nat = &entry["nat"];
        if nat.is_object() {
            add(&mut agg, &mut total, nat["symbol"].as_str().unwrap_or(""), nat["valueUsd"].as_f64().unwrap_or(0.0), c);
        }
        for t in entry["toks"].as_array().unwrap_or(&empty) {
            let mut usd = t["usd"].as_f64().unwrap_or(0.0);
            if usd <= 0.0 {
                let contract = t["contract"].as_str().unwrap_or("");
                usd = t["balance"].as_f64().unwrap_or(0.0) * px.get(&format!("{c}:{contract}")).copied().unwrap_or(0.0);
            }
            if usd > 0.0 && usd < 1e9 {
                add(&mut agg, &mut total, t["symbol"].as_str().unwrap_or(""), usd, c);
            }
        }
    }

    let mut tokens: Vec<Value> = agg
        .iter()
        .map(|(sym, (v, chains))| {
            json!({"symbol": sym, "name": "", "balance": js_num(0.0), "valueUsd": js_num(js_round(*v)), "chains": js_num(chains.len() as f64)})
        })
        .filter(|t| t["valueUsd"].as_f64().unwrap_or(0.0) > 0.0)
        .collect();
    tokens.sort_by(|a, b| {
        b["valueUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["valueUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
    });

    json!({"totalUsd": js_num(js_round(total)), "tokens": Value::Array(tokens), "chainsScanned": js_num(chains_scanned as f64)})
}

/// balances.js `walletBalances` — fetch the 15-chain balances, reprice unrated
/// tokens via DefiLlama, then aggregate. Per chain the token + native fetches run
/// in series (as the JS awaits do); chains run concurrently.
pub async fn wallet_balances(address: &str) -> Value {
    // Cache the full multi-chain balance aggregate: a fresh wallet is ~19 live
    // Blockscout+DefiLlama calls (30-40s). Shared by /api/wallet-assets and
    // /api/report; a background warmer keeps tracked wallets hot.
    let ck = format!("wbal:{}", address.to_lowercase());
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(600)) {
        return v;
    }
    let v = wallet_balances_uncached(address).await;
    if v["tokens"].as_array().is_some_and(|a| !a.is_empty()) || v["totalUsd"].as_f64().unwrap_or(0.0) > 0.0 {
        crate::bs::cache_value(&ck, &v);
    }
    v
}

async fn wallet_balances_uncached(address: &str) -> Value {
    let a = address.to_lowercase();
    let per: Vec<Value> = join_all(BAL_CHAINS.iter().map(|&c| {
        let a = a.clone();
        async move {
            let toks = crate::bs::token_balances_raw(c, &a).await;
            let nat = crate::bs::native_balance(c, &a).await.unwrap_or(Value::Null);
            json!({"c": c, "toks": Value::Array(toks), "nat": nat})
        }
    }))
    .await;

    // Tokens Blockscout left unrated (usd <= 0) with a contract → reprice via DefiLlama.
    let empty = vec![];
    let mut need: Vec<(String, String)> = Vec::new();
    for entry in &per {
        let c = entry["c"].as_str().unwrap_or("");
        for t in entry["toks"].as_array().unwrap_or(&empty) {
            if t["usd"].as_f64().unwrap_or(0.0) <= 0.0 {
                let contract = t["contract"].as_str().unwrap_or("");
                if !contract.is_empty() {
                    need.push((c.to_string(), contract.to_string()));
                }
            }
        }
    }
    let px = crate::prices::token_prices_current(&need).await;
    aggregate_balances(&per, &px, BAL_CHAINS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_balances_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("balances-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("balances-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let px: HashMap<String, f64> =
                c["px"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap())).collect();
            let got = aggregate_balances(c["perChain"].as_array().unwrap(), &px, c["chainsScanned"].as_u64().unwrap() as usize);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
