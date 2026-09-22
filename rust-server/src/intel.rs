// Pure kernel of `GET /api/wallet-intel/:id` (server/intel.js `walletIntel`):
// summarise a tracked wallet's event history into flow totals + a heuristic
// behaviour class + an activity score. Deterministic over the event list (the
// store fetch and the optional ML overlay are the route's job in main.rs).
// Golden-tested byte-for-byte.

use crate::jsmath::{js_num, js_round};
use serde_json::{json, Value};
use std::collections::HashSet;

fn value_usd(e: &Value) -> f64 {
    e["valueUsd"].as_f64().unwrap_or(0.0)
}

/// intel.js `walletIntel(id)` after the store fetch — `all_events` is
/// `store.getEvents(9999)`; the kernel filters to this wallet.
pub fn wallet_intel(id: &str, all_events: &[Value]) -> Value {
    let evs: Vec<&Value> = all_events.iter().filter(|e| e["walletId"].as_str() == Some(id)).collect();

    let in_usd: f64 = evs.iter().filter(|e| e["kind"].as_str() == Some("in")).map(|e| value_usd(e)).sum();
    let out_usd: f64 = evs.iter().filter(|e| e["kind"].as_str() == Some("out")).map(|e| value_usd(e)).sum();
    let counterparties = evs.iter().map(|e| e["counterparty"].as_str().unwrap_or("").to_string()).collect::<HashSet<_>>().len();
    let tokens = evs.iter().map(|e| e["asset"].as_str().unwrap_or("").to_string()).collect::<HashSet<_>>().len();
    let total = evs.len();
    let turnover = in_usd + out_usd;
    let net_flow = in_usd - out_usd;

    // heuristic behaviour classification (priority order, verbatim)
    let behavior = if total < 3 {
        "low_data"
    } else if counterparties >= 12 && tokens >= 4 {
        "exchange"
    } else if total >= 12 && tokens >= 5 {
        "mm"
    } else if out_usd > 0.0 && in_usd / out_usd >= 2.5 {
        "accumulator"
    } else if in_usd > 0.0 && out_usd / in_usd >= 2.5 {
        "distributor"
    } else {
        "trader"
    };

    let whale = turnover > 1_000_000.0;
    // score = clamp[1,99]( round( log10(turnover+10)*14 + min(cp,30) + min(total,30) ) )
    let raw = (turnover + 10.0).log10() * 14.0 + counterparties.min(30) as f64 + total.min(30) as f64;
    let score = js_round(raw).clamp(1.0, 99.0) as i64;

    json!({
        "id": id,
        "txCount": total,
        "inUsd": js_num(in_usd),
        "outUsd": js_num(out_usd),
        "netFlowUsd": js_num(net_flow),
        "counterparties": counterparties,
        "tokens": tokens,
        "behavior": behavior,
        "whale": whale,
        "score": score,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_intel_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/intel-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/intel-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let empty = vec![];
            let events = c["events"].as_array().unwrap_or(&empty);
            let got = wallet_intel(c["id"].as_str().unwrap(), events);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
