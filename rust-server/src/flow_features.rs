//! Port of the pure core of server/flow-features.js — behavioural "flow"
//! features (category shares of value, buy/sell share, tempo) used by the
//! smart-money behaviour ML.
//!
//! The JS fetches transfers across chains and prices them; this takes the
//! resulting `byChain` map and reproduces the reduction and `categoryOf`
//! counterparty classification. Every JS quirk is preserved: `txTotal`
//! increments before the self/empty-counterparty skip; `it.usd` is summed
//! without a truthiness guard; `|| 1` protects each divisor; `txPerDay` divides
//! by the *raw* span (not the rounded `timespanDays`).

use crate::jsmath::{js_num, js_round, round_fixed};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// The four category probes, checked in order (cex → bridge → dex → defi). Names
/// are lowercased before matching, so the patterns are plain lowercase.
struct Cats {
    cex: Regex,
    bridge: Regex,
    dex: Regex,
    defi: Regex,
}

fn cats() -> &'static Cats {
    static C: OnceLock<Cats> = OnceLock::new();
    C.get_or_init(|| Cats {
        cex: Regex::new(r"binance|coinbase|okx|kraken|bybit|huobi|kucoin|gate\.io|gateio|bitfinex|gemini|bitstamp|mexc|bitget|upbit|htx|crypto\.com|cryptocom").unwrap(),
        bridge: Regex::new(r"bridge|wormhole|portal|across|\bhop\b|celer|stargate|synapse|orbiter|debridge|layerzero|relay|gateway").unwrap(),
        dex: Regex::new(r"router|uniswap|sushiswap|1inch|paraswap|kyber|cowswap|aggregator|swap|pancake|odos|dexalot").unwrap(),
        defi: Regex::new(r"protocol|vault|pool|staking|\blend|aave|compound|maker|curve|balancer|lido|rocketpool").unwrap(),
    })
}

/// Category index: cex=0, dex=1, bridge=2, defi=3, eoa=4.
pub fn category_of(name: &str, is_contract: bool) -> usize {
    let s = name.to_lowercase();
    let c = cats();
    if c.cex.is_match(&s) {
        0
    } else if c.bridge.is_match(&s) {
        2
    } else if c.dex.is_match(&s) {
        1
    } else if c.defi.is_match(&s) || is_contract {
        3
    } else {
        4
    }
}

/// flow-features.js `_extractFlow`, minus the fetch/price awaits. `by_chain` maps
/// chain id → priced transfer array (objects with `from`/`to`/`fromName`/
/// `toName`/`fromContract`/`toContract`/`usd`/`ts`). `address` is compared
/// case-sensitively against the lowercased `from`/`to`.
pub fn extract_flow(address: &str, by_chain: &Map<String, Value>) -> Value {
    let a = address.to_lowercase();

    let mut in_usd = 0.0;
    let mut out_usd = 0.0;
    let mut tx_total = 0i64;
    let mut cps: IndexMap<String, bool> = IndexMap::new();
    let mut ts: Vec<f64> = Vec::new();
    let mut c_in = [0.0f64; 5];
    let mut c_out = [0.0f64; 5];

    for (_chain, trs) in by_chain {
        let Some(arr) = trs.as_array() else { continue };
        for it in arr {
            tx_total += 1;
            let out = it["from"].as_str() == Some(a.as_str());
            // o = out ? it.to : it.from
            let o = if out { it["to"].as_str() } else { it["from"].as_str() };
            // `if (!o || o === a) continue` — undefined/empty/self are skipped.
            let o = match o {
                Some(s) if !s.is_empty() && s != a => s,
                _ => continue,
            };
            let name = if out { it["toName"].as_str() } else { it["fromName"].as_str() }.unwrap_or("");
            let contract = if out { it["toContract"].as_bool() } else { it["fromContract"].as_bool() }.unwrap_or(false);
            let cat = category_of(name, contract);
            let usd = it["usd"].as_f64().unwrap_or(f64::NAN);
            if out {
                out_usd += usd;
                c_out[cat] += usd;
            } else {
                in_usd += usd;
                c_in[cat] += usd;
            }
            let e = cps.entry(o.to_string()).or_insert(false);
            *e = *e || contract;
            if let Some(v) = it["ts"].as_f64() {
                if v != 0.0 && !v.is_nan() {
                    ts.push(v);
                }
            }
        }
    }

    // JS `x || 1`: zero or NaN falls back to 1.
    let or1 = |x: f64| if x == 0.0 || x.is_nan() { 1.0 } else { x };
    let total = or1(in_usd + out_usd);
    let uniq = cps.len();
    let span = if ts.len() >= 2 {
        let mn = ts.iter().cloned().fold(f64::INFINITY, f64::min);
        let mx = ts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        (mx - mn) / 86_400_000.0
    } else {
        0.0
    };
    let contract_count = cps.values().filter(|&&c| c).count();

    let share = |a: f64, b: f64| round_fixed((a + b) / total, 3);

    let mut o = Map::new();
    o.insert("txTotal".into(), js_num(tx_total as f64));
    o.insert("uniqueCps".into(), js_num(uniq as f64));
    o.insert("chains".into(), js_num(by_chain.len() as f64));
    o.insert("inUsd".into(), js_num(js_round(in_usd)));
    o.insert("outUsd".into(), js_num(js_round(out_usd)));
    o.insert("netUsd".into(), js_num(js_round(in_usd - out_usd)));
    o.insert("buyShare".into(), js_num(round_fixed(c_in[0] / or1(in_usd), 3)));
    o.insert("sellShare".into(), js_num(round_fixed(c_out[0] / or1(out_usd), 3)));
    o.insert("cexShare".into(), js_num(share(c_in[0], c_out[0])));
    o.insert("dexShare".into(), js_num(share(c_in[1], c_out[1])));
    o.insert("bridgeShare".into(), js_num(share(c_in[2], c_out[2])));
    o.insert("defiShare".into(), js_num(share(c_in[3], c_out[3])));
    o.insert("eoaShare".into(), js_num(share(c_in[4], c_out[4])));
    let contract_ratio = if uniq != 0 { contract_count as f64 / uniq as f64 } else { 0.0 };
    o.insert("contractRatio".into(), js_num(round_fixed(contract_ratio, 3)));
    o.insert("timespanDays".into(), js_num(round_fixed(span, 2)));
    o.insert("txPerDay".into(), js_num(round_fixed(tx_total as f64 / span.max(0.01), 2)));
    o.insert("avgTxUsd".into(), js_num(js_round(total / (if tx_total == 0 { 1.0 } else { tx_total as f64 }))));
    Value::Object(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_features_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("flow-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("flow-golden.json")).unwrap();

        for (i, case) in inputs.as_array().unwrap().iter().enumerate() {
            let address = case["address"].as_str().unwrap();
            let by_chain = case["byChain"].as_object().unwrap();
            let got = extract_flow(address, by_chain);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
