//! Port of the pure core of server/walletscore.js — realized PnL / ROI /
//! win-rate / hold-time scoring for a wallet, computed from its priced
//! transfers. This is both the `/api/wallet-score` core and the label source
//! for the smart-money ML self-supervision.
//!
//! The two network/pricing awaits in the JS produce `byChain` (a map of chain →
//! priced transfers); this takes that map and reproduces the average-cost PnL
//! ladder and the scoring exactly, including JS truthiness on `usd`/`ts`, the
//! `amt || 1` price guard, `Math.round` half-up, and `toFixed(1)` on ROI.

use crate::jsmath::{js_num, js_round, round_fixed};
use indexmap::IndexMap;
use serde_json::{Map, Value};

struct Event {
    dir_in: bool,
    amt: f64,
    usd: f64,
    ts: f64,
}

struct Position {
    sym: String,
    evs: Vec<Event>,
}

/// walletscore.js `computeWalletScore`, minus the transfer fetch and pricing.
/// `by_chain` maps a chain id to its already-priced transfer array (objects with
/// `from`/`to`/`asset`/`token`/`amount`/`usd`/`ts`). Iteration follows the map's
/// key order and each chain's array order, matching Node's `Object.entries`.
pub fn compute_wallet_score(address: &str, by_chain: &Map<String, Value>) -> Value {
    let address = address.to_lowercase();

    // Build positions keyed by "chain:token-or-asset", preserving first-seen order.
    let mut positions: IndexMap<String, Position> = IndexMap::new();
    for (c, trs) in by_chain {
        let Some(arr) = trs.as_array() else { continue };
        for t in arr {
            // `if (!t.usd || !t.ts) continue` — both must be truthy (nonzero, non-NaN).
            let usd = match truthy_num(&t["usd"]) {
                Some(v) => v,
                None => continue,
            };
            let ts = match truthy_num(&t["ts"]) {
                Some(v) => v,
                None => continue,
            };
            let dir_in = if t["to"].as_str() == Some(address.as_str()) {
                true
            } else if t["from"].as_str() == Some(address.as_str()) {
                false
            } else {
                continue;
            };
            let asset = t["asset"].as_str().unwrap_or("");
            let token = t["token"].as_str().filter(|s| !s.is_empty());
            let key = format!("{c}:{}", token.unwrap_or(asset));
            let amt = t["amount"].as_f64().unwrap_or(f64::NAN);
            positions
                .entry(key)
                .or_insert_with(|| Position { sym: asset.to_string(), evs: Vec::new() })
                .evs
                .push(Event { dir_in, amt, usd, ts });
        }
    }

    let mut realized = 0.0;
    let mut invested = 0.0;
    let mut wins = 0i64;
    let mut losses = 0i64;
    let mut closed = 0i64;
    let mut hold_ms_w = 0.0;
    let mut hold_w = 0.0;
    let mut top: Vec<(String, i64)> = Vec::new();
    let mut open: IndexMap<String, f64> = IndexMap::new();

    for p in positions.values_mut() {
        // Stable sort by timestamp, matching JS Array.sort.
        p.evs.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap_or(std::cmp::Ordering::Equal));
        let mut qty = 0.0;
        let mut cost = 0.0;
        let mut first_buy_ts = 0.0;
        let mut token_realized = 0.0;
        let mut token_invested = 0.0;
        let mut sold = false;
        for e in &p.evs {
            // price = usd / (amt || 1): a zero/NaN amount falls back to 1.
            let denom = if e.amt != 0.0 && !e.amt.is_nan() { e.amt } else { 1.0 };
            let price = e.usd / denom;
            if e.dir_in {
                if first_buy_ts == 0.0 {
                    first_buy_ts = e.ts;
                }
                qty += e.amt;
                cost += e.usd;
                token_invested += e.usd;
            } else if qty > 1e-9 {
                let sell_amt = e.amt.min(qty);
                let avg_cost = cost / qty;
                token_realized += (price - avg_cost) * sell_amt;
                qty -= sell_amt;
                cost -= avg_cost * sell_amt;
                sold = true;
                if first_buy_ts != 0.0 {
                    hold_ms_w += (e.ts - first_buy_ts) * e.usd;
                    hold_w += e.usd;
                }
            }
        }
        realized += token_realized;
        invested += token_invested;
        if token_invested > 50.0 && sold {
            closed += 1;
            if token_realized > 0.0 {
                wins += 1;
            } else if token_realized < 0.0 {
                losses += 1;
            }
            top.push((p.sym.clone(), js_round(token_realized) as i64));
        }
        if qty > 1e-9 && cost > 0.0 {
            *open.entry(p.sym.clone()).or_insert(0.0) += cost;
        }
    }

    let win_rate: Option<i64> = if closed != 0 { Some(js_round(wins as f64 / closed as f64 * 100.0) as i64) } else { None };
    let roi: Option<f64> = if invested > 0.0 { Some(round_fixed(realized / invested * 100.0, 1)) } else { None };
    let hold_days: Option<i64> = if hold_w > 0.0 { Some(js_round(hold_ms_w / hold_w / 86_400_000.0) as i64) } else { None };

    let mut score = 50.0;
    if let Some(r) = roi {
        score += (r * 0.4).min(40.0).max(-30.0);
    }
    if let Some(w) = win_rate {
        score += (w as f64 - 50.0) * 0.4;
    }
    let score = js_round(score).min(99.0).max(1.0) as i64;

    // Stable descending sort by pnl for best/worst.
    top.sort_by(|a, b| b.1.cmp(&a.1));
    let best: Vec<Value> = top.iter().take(3).map(top_entry).collect();
    let worst: Vec<Value> = {
        let start = top.len().saturating_sub(3);
        top[start..].iter().rev().map(top_entry).collect()
    };

    let open_positions: Vec<Value> = open
        .iter()
        .map(|(sym, cost)| (sym.clone(), js_round(*cost) as i64))
        .filter(|(_, c)| *c > 50)
        .map(|(sym, c)| json_obj(&[("sym", Value::String(sym)), ("costUsd", js_num(c as f64))]))
        .collect();

    json_obj(&[
        ("address", Value::String(address)),
        ("score", js_num(score as f64)),
        ("roi", roi.map(|r| js_num(r)).unwrap_or(Value::Null)),
        ("winRate", win_rate.map(|w| js_num(w as f64)).unwrap_or(Value::Null)),
        ("realizedUsd", js_num(js_round(realized))),
        ("investedUsd", js_num(js_round(invested))),
        ("positions", js_num(positions.len() as f64)),
        ("closed", js_num(closed as f64)),
        ("wins", js_num(wins as f64)),
        ("losses", js_num(losses as f64)),
        ("holdDays", hold_days.map(|h| js_num(h as f64)).unwrap_or(Value::Null)),
        ("best", Value::Array(best)),
        ("worst", Value::Array(worst)),
        ("openPositions", Value::Array(open_positions)),
    ])
}

/// JS truthiness for a numeric field: `Some(v)` iff `v` is a nonzero, non-NaN
/// number; `None` (falsy) otherwise.
fn truthy_num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|&n| n != 0.0 && !n.is_nan())
}

fn top_entry((sym, pnl): &(String, i64)) -> Value {
    json_obj(&[("sym", Value::String(sym.clone())), ("pnl", js_num(*pnl as f64))])
}

fn json_obj(fields: &[(&str, Value)]) -> Value {
    let mut o = Map::new();
    for (k, v) in fields {
        o.insert((*k).to_string(), v.clone());
    }
    Value::Object(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_score_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("wscore-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("wscore-golden.json")).unwrap();

        for (i, case) in inputs.as_array().unwrap().iter().enumerate() {
            let address = case["address"].as_str().unwrap();
            let by_chain = case["byChain"].as_object().unwrap();
            let got = compute_wallet_score(address, by_chain);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
