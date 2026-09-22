//! Moralis — ERC-20 balances for chains whose Blockscout instance no longer exists.
//!
//! New in the port; Node has no equivalent. `blockscout.js` was the only source of token
//! balances, and the instances for linea/blast/mantle/sonic/fraxtal are gone (404), so a
//! wallet's holdings on those chains came back as an empty list — indistinguishable from
//! a wallet that holds nothing. Four of them (linea, blast, mantle, sonic) sit in
//! `BAL_CHAINS`, so this was a silent hole in every balance figure we report.
//!
//! Moralis only closes part of it: it answers for linea but rejects blast/mantle/sonic
//! outright ("chain must be a valid enum value"). Chains it does not know return `None`
//! here so the caller keeps looking rather than believing an empty list.

use crate::jsmath::js_num;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

/// Moralis' chain enum. Verified live against our key — blast (0x13e31), mantle (0x1388)
/// and sonic (0x92) are deliberately ABSENT: Moralis rejects all three, and listing them
/// would spend a request to learn that every time.
fn chain_param(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" => "0x1",
        "base" => "0x2105",
        "arbitrum" => "0xa4b1",
        "optimism" => "0xa",
        "polygon" => "0x89",
        "gnosis" => "0x64",
        "linea" => "0xe708",
        _ => return None,
    })
}

fn api_key() -> Option<&'static str> {
    static K: OnceLock<Option<String>> = OnceLock::new();
    K.get_or_init(|| std::env::var("MORALIS_API_KEY").ok().filter(|s| !s.is_empty())).as_deref()
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("wallet-radar/rust")
            .build()
            .expect("reqwest client builds from static config")
    })
}

/// `result[]` → the `token_balances_raw` shape `{symbol, contract, balance, usd}`.
///
/// Two things must be dropped or the totals lie:
/// - `native_token` entries (Moralis reports the native coin as 0xeee…eee in this list,
///   while Blockscout's `/tokens` is ERC-20 only and the native balance is fetched
///   separately) — keeping them double-counts ETH.
/// - non-positive balances, exactly as the Blockscout branch does.
pub fn parse_balances(d: &Value) -> Vec<Value> {
    let empty = vec![];
    let mut out = Vec::new();
    for it in d["result"].as_array().unwrap_or(&empty) {
        if it["native_token"].as_bool().unwrap_or(false) {
            continue;
        }
        let dec = it["decimals"].as_f64().unwrap_or(18.0);
        let raw = it["balance"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let bal = raw / 10f64.powf(dec);
        if bal <= 0.0 {
            continue;
        }
        let symbol = it["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?").to_uppercase();
        let contract = it["token_address"].as_str().unwrap_or("").to_lowercase();
        let usd = it["usd_value"].as_f64().unwrap_or(0.0);
        out.push(json!({
            "symbol": symbol,
            "contract": contract,
            "balance": js_num(bal),
            "usd": js_num(if usd > 0.0 { usd } else { 0.0 }),
        }));
    }
    out
}

/// `None` = no key, chain Moralis does not know, or the fetch failed — all cases where
/// the caller must NOT read the result as "this wallet holds nothing".
pub async fn token_balances(chain: &str, address: &str) -> Option<Vec<Value>> {
    let param = chain_param(chain)?;

    // Суворий режим: ключ живе на сателіті, кор його не має й мати не повинен.
    let d = if crate::satellite::strict() {
        crate::satellite::fetch_json(
            "moralis",
            &format!("/api/v2.2/wallets/{address}/tokens"),
            &[("chain", param.to_string())],
        )
        .await
        .ok()?
    } else {
        let key = api_key()?;
        let url = format!("https://deep-index.moralis.io/api/v2.2/wallets/{address}/tokens?chain={param}");
        let resp = http().get(&url).header("X-API-Key", key).header("accept", "application/json").send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<Value>().await.ok()?
    };
    // A body carrying `message` instead of `result` is Moralis refusing (unknown chain,
    // too many balances, bad key) — a failure, not an empty wallet.
    if !d["result"].is_array() {
        return None;
    }
    Some(parse_balances(&d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_entry_is_dropped_and_amounts_scale() {
        let d = json!({"result": [
            // Moralis lists the native coin here; Blockscout's /tokens does not, and the
            // native balance is fetched separately — keeping this would double-count ETH.
            {"token_address": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "symbol": "ETH",
             "decimals": 18, "balance": "1000000000000000000", "usd_value": 3000.0, "native_token": true},
            {"token_address": "0xA0B86991C6218B36C1D19D4A2E9EB0CE3606EB48", "symbol": "usdc",
             "decimals": 6, "balance": "2500000", "usd_value": 2.5, "native_token": false},
            // Non-positive balances are dropped, matching the Blockscout branch.
            {"token_address": "0xdead", "symbol": "ZERO", "decimals": 18, "balance": "0", "native_token": false},
        ]});
        let out = parse_balances(&d);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["symbol"], "USDC");
        assert_eq!(out[0]["contract"], "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
        assert_eq!(out[0]["balance"], 2.5);
        assert_eq!(out[0]["usd"], 2.5);
    }

    #[test]
    fn chains_moralis_rejects_are_absent() {
        // Verified live: Moralis answers "chain must be a valid enum value" for these.
        for c in ["blast", "mantle", "sonic"] {
            assert!(chain_param(c).is_none(), "{c} must not be claimed — Moralis rejects it");
        }
        assert_eq!(chain_param("linea"), Some("0xe708"));
    }
}
