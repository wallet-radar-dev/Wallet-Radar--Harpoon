// Pure kernel of `GET /api/wallet-clusters` (server/index.js:487-501): given the
// already-built counterparty map + monthly buckets + historical-pricing coverage
// + sampled EOA average balance, classify counterparties into behavioural
// clusters (cex/dex/bridge/defi/eoa), derive a trading intent/verdict, and emit a
// monthly timeline. The multi-chain fetch, historical repricing and RPC balance
// sampling are the route's job in main.rs. Golden-tested byte-for-byte.

use crate::jsmath::{js_num, js_round};
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

// Math.round -> integer JSON value (result is always integral).
fn ri(x: f64) -> Value {
    js_num(js_round(x))
}

// categoryOf(name, isContract) — server/index.js:432, verbatim regex set.
fn cex_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)binance|coinbase|okx|kraken|bybit|huobi|kucoin|gate\.io|gateio|bitfinex|gemini|bitstamp|mexc|bitget|upbit|htx|crypto\.com|cryptocom").unwrap())
}
fn bridge_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)bridge|wormhole|portal|across|\bhop\b|celer|stargate|synapse|orbiter|debridge|layerzero|relay|gateway").unwrap())
}
fn dex_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)router|uniswap|sushiswap|1inch|paraswap|kyber|cowswap|aggregator|swap|pancake|odos|0x ?protocol|dexalot").unwrap())
}
fn defi_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)protocol|vault|pool|staking|\blend|aave|compound|maker|curve|balancer|lido|rocketpool").unwrap())
}

pub fn category_of(name: &str, is_contract: bool) -> &'static str {
    if cex_re().is_match(name) {
        "cex"
    } else if bridge_re().is_match(name) {
        "bridge"
    } else if dex_re().is_match(name) {
        "dex"
    } else if defi_re().is_match(name) || is_contract {
        "defi"
    } else {
        "eoa"
    }
}

const CAT_INTENT_OUT: &[(&str, &str)] = &[("cex", "sell_cex"), ("dex", "swap_dex"), ("bridge", "bridge"), ("defi", "defi"), ("eoa", "transfer")];
fn cat_intent_out(cat: &str) -> &'static str {
    CAT_INTENT_OUT.iter().find(|(k, _)| *k == cat).map(|(_, v)| *v).unwrap_or("transfer")
}

fn f(e: &Value, k: &str) -> f64 {
    e[k].as_f64().unwrap_or(0.0)
}

/// The kernel. `cp` is the ordered counterparty list (insertion order of the JS
/// Map); each entry: {addr,name,contract,inCount,outCount,inUsd,outUsd,chains}.
/// `monthly` is an object period -> {inUsd,outUsd,txIn,txOut}.
#[allow(clippy::too_many_arguments)]
pub fn wallet_clusters(
    address: &str,
    chain_type: &str,
    cp: &[Value],
    monthly: &Value,
    first_ts: f64,
    last_ts: f64,
    coverage: f64,
    active_chains: &Value,
    eoa_avg_usd: &Value,
) -> Value {
    // cats in fixed insertion order cex,dex,bridge,defi,eoa
    let order = ["cex", "dex", "bridge", "defi", "eoa"];
    let mut cats: Vec<(&str, Vec<&Value>)> = order.iter().map(|k| (*k, Vec::new())).collect();
    for e in cp {
        let name = e["name"].as_str().unwrap_or("");
        let is_contract = e["contract"].as_bool().unwrap_or(false);
        let cat = category_of(name, is_contract);
        cats.iter_mut().find(|(k, _)| *k == cat).unwrap().1.push(e);
    }

    let mut clusters: Vec<Value> = Vec::new();
    for (category, arr) in cats.iter_mut() {
        if arr.is_empty() {
            continue;
        }
        let in_usd = ri(arr.iter().map(|m| f(m, "inUsd")).sum());
        let out_usd = ri(arr.iter().map(|m| f(m, "outUsd")).sum());
        let tx_in: i64 = arr.iter().map(|m| m["inCount"].as_i64().unwrap_or(0)).sum();
        let tx_out: i64 = arr.iter().map(|m| m["outCount"].as_i64().unwrap_or(0)).sum();
        let avg_balance = if *category == "eoa" { eoa_avg_usd.clone() } else { Value::Null };
        // sort desc by (in+out+inCount+outCount); stable
        arr.sort_by(|a, b| {
            let sb = f(b, "inUsd") + f(b, "outUsd") + b["inCount"].as_f64().unwrap_or(0.0) + b["outCount"].as_f64().unwrap_or(0.0);
            let sa = f(a, "inUsd") + f(a, "outUsd") + a["inCount"].as_f64().unwrap_or(0.0) + a["outCount"].as_f64().unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        let top: Vec<Value> = arr.iter().take(6).map(|m| {
            let name = m["name"].as_str().unwrap_or("");
            let in_c = m["inCount"].as_i64().unwrap_or(0);
            let out_c = m["outCount"].as_i64().unwrap_or(0);
            json!({
                "addr": m["addr"],
                "name": if name.is_empty() { Value::Null } else { Value::String(name.to_string()) },
                "inUsd": ri(f(m, "inUsd")),
                "outUsd": ri(f(m, "outUsd")),
                "tx": in_c + out_c,
                "chains": m["chains"].clone(),
            })
        }).collect();
        clusters.push(json!({
            "category": category,
            "count": arr.len(),
            "inUsd": in_usd,
            "outUsd": out_usd,
            "txIn": tx_in,
            "txOut": tx_out,
            "avgBalanceUsd": avg_balance,
            "top": top,
        }));
    }

    // intent
    let mut out_by_cat = Map::new();
    let mut in_by_cat = Map::new();
    for c in &clusters {
        let cat = c["category"].as_str().unwrap().to_string();
        out_by_cat.insert(cat.clone(), c["outUsd"].clone());
        in_by_cat.insert(cat, c["inUsd"].clone());
    }
    let gv = |m: &Map<String, Value>, k: &str| -> f64 { m.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0) };
    let total_out: f64 = out_by_cat.values().map(|v| v.as_f64().unwrap_or(0.0)).sum();
    let total_in: f64 = in_by_cat.values().map(|v| v.as_f64().unwrap_or(0.0)).sum();
    let sell_usd = js_round(gv(&out_by_cat, "cex"));
    let buy_usd = js_round(gv(&in_by_cat, "cex"));
    let net_cex = sell_usd - buy_usd;
    let net_flow = js_round(total_in - total_out);
    let stance = if net_flow >= 0.0 { "accumulating" } else { "distributing" };

    let mut acts: Vec<(&str, f64)> = vec![
        ("sell_cex", gv(&out_by_cat, "cex")),
        ("buy_cex", gv(&in_by_cat, "cex")),
        ("swap_dex", gv(&out_by_cat, "dex") + gv(&in_by_cat, "dex")),
        ("bridge", gv(&out_by_cat, "bridge") + gv(&in_by_cat, "bridge")),
        ("transfer", gv(&out_by_cat, "eoa")),
        ("defi", gv(&out_by_cat, "defi") + gv(&in_by_cat, "defi")),
    ];
    let total_act: f64 = acts.iter().map(|(_, v)| *v).sum();
    let (mut verdict, mut verdict_pct, mut basis) = ("hold".to_string(), 0.0f64, "usd");
    if total_act > 0.0 {
        acts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        verdict = acts[0].0.to_string();
        verdict_pct = js_round(acts[0].1 / total_act * 100.0);
    } else {
        basis = "tx";
        let mut by_tx: Vec<&Value> = clusters.iter().collect();
        by_tx.sort_by(|a, b| b["txOut"].as_f64().unwrap_or(0.0).partial_cmp(&a["txOut"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        let tot_tx: f64 = clusters.iter().map(|c| c["txOut"].as_f64().unwrap_or(0.0)).sum::<f64>().max(1.0);
        if let Some(first) = by_tx.first() {
            let to = first["txOut"].as_f64().unwrap_or(0.0);
            if to > 0.0 {
                verdict = cat_intent_out(first["category"].as_str().unwrap()).to_string();
                verdict_pct = js_round(to / tot_tx * 100.0);
            }
        }
    }

    // timeline: sort periods ascending
    let mut periods: Vec<(&String, &Value)> = monthly.as_object().map(|o| o.iter().collect()).unwrap_or_default();
    periods.sort_by(|a, b| a.0.cmp(b.0));
    let timeline: Vec<Value> = periods.iter().map(|(period, m)| json!({
        "period": period,
        "inUsd": ri(f(m, "inUsd")),
        "outUsd": ri(f(m, "outUsd")),
        "txIn": m["txIn"].as_i64().unwrap_or(0),
        "txOut": m["txOut"].as_i64().unwrap_or(0),
    })).collect();

    json!({
        "address": address,
        "type": chain_type,
        "chains": active_chains,
        "counterparties": cp.len(),
        "coverage": ri(coverage * 100.0),
        "period": { "first": if first_ts != 0.0 { js_num(first_ts) } else { Value::Null }, "last": if last_ts != 0.0 { js_num(last_ts) } else { Value::Null } },
        "timeline": timeline,
        "clusters": clusters,
        "intent": {
            "verdict": verdict,
            "verdictPct": js_num(verdict_pct),
            "basis": basis,
            "buyUsd": js_num(buy_usd),
            "sellUsd": js_num(sell_usd),
            "netCexUsd": js_num(net_cex),
            "netFlowUsd": js_num(net_flow),
            "stance": stance,
            "outByCat": Value::Object(out_by_cat),
            "inByCat": Value::Object(in_by_cat),
            "totalOutUsd": ri(total_out),
            "totalInUsd": ri(total_in),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_clusters_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("wclusters-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("wclusters-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let empty = vec![];
            let cp = c["cp"].as_array().unwrap_or(&empty);
            let got = wallet_clusters(
                c["address"].as_str().unwrap(),
                c["type"].as_str().unwrap(),
                cp,
                &c["monthly"],
                c["firstTs"].as_f64().unwrap_or(0.0),
                c["lastTs"].as_f64().unwrap_or(0.0),
                c["coverage"].as_f64().unwrap_or(0.0),
                &c["activeChains"],
                &c["eoaAvgUsd"],
            );
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
