//! Port of the pure core of server/assetflow.js `_computeAssetFlow` — the shared
//! asset-flow analysis: whale entry/exit points, PnL phase, liquidity tier,
//! profit leaderboard and a three-colour pump signal.
//!
//! `compute_asset_flow` is deterministic given its inputs and golden-tested; the
//! non-deterministic external inputs (`mc` = CoinGecko marketData, `social`) are
//! fetched in main.rs. `get_social_free` is the keyless CoinGecko social fallback
//! (without the persisted history module, matching a cold process: no rising
//! deltas, `haveHistory:false`).

use crate::jsmath::{js_num, js_round, round_fixed, to_precision};
use serde_json::{json, Value};
use std::cmp::Ordering::Equal;

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `?? null` — absent/JSON-null → Null, else the value.
fn nn(v: &Value) -> Value {
    if v.is_null() {
        Value::Null
    } else {
        v.clone()
    }
}

/// `a || b || ""` over string fields.
fn str_or(a: &Value, b: &Value) -> String {
    a.as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| b.as_str().filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string()
}

/// Nearest series price to `ts` (first-seen wins on ties), but ONLY within `PRICE_TOL_MS`; 0 if none.
///
/// The tolerance is the fix: the search used to be unbounded, so a transfer older than the chart
/// window (160 weekly candles ≈ 3 years) silently took the price of the FIRST candle — a whale's
/// "average entry" was then computed against a number nobody paid. Candles are weekly, so anything
/// inside the covered range is at most ~3.5 days from one; beyond the tolerance we return 0 and the
/// caller skips the transfer, which is honest about the gap instead of inventing an entry.
const PRICE_TOL_MS: f64 = 4.0 * 86_400_000.0;
fn price_at(ts: f64, series: &[Value]) -> f64 {
    let mut best = 0.0;
    let mut bd = f64::INFINITY;
    for p in series {
        let d = (p["ts"].as_f64().unwrap_or(0.0) - ts).abs();
        if d < bd {
            bd = d;
            best = p["price"].as_f64().unwrap_or(0.0);
        }
    }
    if bd <= PRICE_TOL_MS {
        best
    } else {
        0.0
    }
}

/// assetflow.js `getSocialFree(contract, mc)` — keyless CoinGecko social fallback,
/// without the persisted history (cold process: `haveHistory:false`, `spike` from
/// sentiment only).
pub fn get_social_free(_contract: &str, mc: &Value) -> Value {
    if !mc.is_object() {
        return Value::Null;
    }
    let (su, wl, ra) = (&mc["sentimentUp"], &mc["watchlistUsers"], &mc["redditActive48h"]);
    if su.is_null() && wl.is_null() && ra.is_null() {
        return Value::Null;
    }
    let spike = su.as_f64().map(|v| v >= 75.0).unwrap_or(false);
    json!({"available": true, "source": "coingecko", "galaxyScore": Value::Null, "sentiment": nn(su), "socialVolume24h": nn(ra), "watchlistUsers": nn(wl), "spike": spike, "haveHistory": false})
}

/// assetflow.js `_computeAssetFlow`, minus the fetches. `holders` are the filtered
/// top holders (`{addr, share}`); `per` the aligned per-holder transfers; `series`
/// the price chart; `mc` the marketData; `social` the resolved social object (or
/// Null). `now` is `Date.now()` (only used when the series is empty, which yields
/// no priced transfers anyway).
#[allow(clippy::too_many_arguments)]
pub fn compute_asset_flow(contract: &str, info: &Value, holders: &[Value], per: &Value, series: &[Value], mc: &Value, social: &Value, now: f64) -> Value {
    let span_start = if series.is_empty() { 0.0 } else { series[0]["ts"].as_f64().unwrap_or(0.0) };
    let span_end = if series.is_empty() { now } else { series[series.len() - 1]["ts"].as_f64().unwrap_or(0.0) };
    let recent_cut = span_end - (span_end - span_start) * 0.25;

    let (mut in_amt, mut in_val, mut in_tw) = (0.0f64, 0.0, 0.0);
    let (mut out_amt, mut out_val, mut out_tw) = (0.0f64, 0.0, 0.0);
    let (mut rin_amt, mut rin_val, mut rin_tw) = (0.0f64, 0.0, 0.0);
    let mut tx_count = 0i64;
    let mut per_holder: Vec<(String, Value, f64, f64, f64, f64)> = Vec::new(); // addr, share, hInA, hInV, hOutA, hOutV
    let empty = vec![];
    let per_arr = per.as_array().unwrap_or(&empty);

    for (i, h) in holders.iter().enumerate() {
        let haddr = h["addr"].as_str();
        let (mut h_in_a, mut h_in_v, mut h_out_a, mut h_out_v) = (0.0f64, 0.0, 0.0, 0.0);
        for t in per_arr.get(i).and_then(|v| v.as_array()).unwrap_or(&empty) {
            let ts = t["ts"].as_f64().unwrap_or(0.0);
            let px = price_at(ts, series);
            if px == 0.0 || ts == 0.0 {
                continue; // !px || !t.ts
            }
            let amount = t["amount"].as_f64().unwrap_or(0.0);
            let is_in = t["to"].as_str() == haddr;
            let is_out = t["from"].as_str() == haddr;
            if is_in && !is_out {
                in_amt += amount;
                in_val += amount * px;
                in_tw += amount * ts;
                tx_count += 1;
                h_in_a += amount;
                h_in_v += amount * px;
                if ts >= recent_cut {
                    rin_amt += amount;
                    rin_val += amount * px;
                    rin_tw += amount * ts;
                }
            } else if is_out && !is_in {
                out_amt += amount;
                out_val += amount * px;
                out_tw += amount * ts;
                tx_count += 1;
                h_out_a += amount;
                h_out_v += amount * px;
            }
        }
        per_holder.push((haddr.unwrap_or("").to_string(), h["share"].clone(), h_in_a, h_in_v, h_out_a, h_out_v));
    }

    let mk = |amt: f64, val: f64, tw: f64| -> Value {
        if amt > 0.0 {
            json!({"price": js_num(val / amt), "ts": js_num(js_round(tw / amt))})
        } else {
            Value::Null
        }
    };
    let last_price = if series.is_empty() { 0.0 } else { series[series.len() - 1]["price"].as_f64().unwrap_or(0.0) };
    let cp = if last_price != 0.0 { last_price } else { mc["price"].as_f64().unwrap_or(0.0) };
    let avg_entry = mk(in_amt, in_val, in_tw);
    let avg_exit = mk(out_amt, out_val, out_tw);
    let last_entry = mk(rin_amt, rin_val, rin_tw);

    let wp = |pt: &Value| -> Value {
        match pt["price"].as_f64() {
            Some(p) if pt.is_object() && p != 0.0 => js_num((cp - p) / p),
            _ => Value::Null,
        }
    };
    let avg_entry_pct = wp(&avg_entry);
    let last_entry_pct = wp(&last_entry);
    let phase = match avg_entry_pct.as_f64() {
        None => "unknown",
        Some(v) if v < 0.0 => "accumulation",
        Some(v) if v > 1.0 => "distribution_risk",
        Some(_) => "in_profit",
    };
    // `basis`/`survivorshipBias` are not decoration: this is computed from the CURRENT top holders,
    // so whales who sold at a profit and left are absent by construction. The average entry is
    // therefore biased upward and the phase biased toward "accumulation". The unbiased estimate is
    // the event-study backtest (server/backtest.js), which reads the full transfer history.
    let whale_pnl = json!({
        "current": js_num(cp), "avgEntryPct": avg_entry_pct, "lastEntryPct": last_entry_pct, "phase": phase,
        "basis": "current-top-holders", "survivorshipBias": true,
    });

    let (vol24, mcap) = (&mc["volume24h"], &mc["marketCap"]);
    let vol_to_mcap = if truthy(vol24) && truthy(mcap) {
        js_num(round_fixed(vol24.as_f64().unwrap() / mcap.as_f64().unwrap(), 4))
    } else {
        Value::Null
    };
    let liq_tier = match vol_to_mcap.as_f64() {
        None => "unknown",
        Some(v) if v >= 0.10 => "deep",
        Some(v) if v >= 0.03 => "moderate",
        Some(_) => "thin",
    };
    let drawdown_context = liq_tier == "thin" && phase == "accumulation";
    let liquidity = json!({"volume24h": nn(vol24), "marketCap": nn(mcap), "volToMcap": vol_to_mcap, "tier": liq_tier, "drawdownContext": drawdown_context});

    let mut leaderboard: Vec<Value> = Vec::new();
    for (addr, share, h_in_a, h_in_v, h_out_a, h_out_v) in &per_holder {
        if !(*h_in_a > 0.0 && *h_in_v > 0.0) {
            continue;
        }
        let avg_entry_h = h_in_v / h_in_a;
        let held = (h_in_a - h_out_a).max(0.0);
        let realized = h_out_v - h_out_a * avg_entry_h;
        let unrealized = held * (cp - avg_entry_h);
        let profit = js_round(realized + unrealized);
        if !profit.is_finite() {
            continue;
        }
        let multiple = if avg_entry_h > 0.0 && cp > 0.0 { js_num(round_fixed(cp / avg_entry_h, 2)) } else { Value::Null };
        leaderboard.push(json!({
            "addr": addr, "share": share, "avgEntry": js_num(to_precision(avg_entry_h, 4)),
            "currentValue": js_num(js_round(held * cp)), "profitUsd": js_num(profit), "multiple": multiple,
        }));
    }
    leaderboard.sort_by(|a, b| b["profitUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["profitUsd"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));
    leaderboard.truncate(12);

    let exit_ratio_raw = if in_amt > 0.0 { (out_amt / in_amt).min(1.0) } else { 0.0 };
    let recent_buy_ratio = if in_amt > 0.0 { round_fixed(rin_amt / in_amt, 2) } else { 0.0 };

    let social_spike = if social.is_object() {
        if !social["spike"].is_null() {
            social["spike"].clone()
        } else if !social["galaxyScore"].is_null() {
            json!(social["galaxyScore"].as_f64().unwrap_or(0.0) >= 60.0)
        } else {
            Value::Null
        }
    } else {
        Value::Null
    };

    let mut pump_phase = "neutral";
    if phase != "unknown" {
        if exit_ratio_raw >= 0.5 || phase == "distribution_risk" {
            pump_phase = "distribution";
        } else if phase == "in_profit" && exit_ratio_raw < 0.4 {
            pump_phase = "pump";
        } else if phase == "accumulation" && exit_ratio_raw < 0.3 {
            pump_phase = "prepump";
        }
    }
    let sg = |k: &str| if social.is_object() { nn(&social[k]) } else { Value::Null };
    let social_source = if social.is_object() {
        if !social["source"].is_null() { social["source"].clone() } else { json!("lunarcrush") }
    } else {
        Value::Null
    };
    let pump_signal = json!({
        "phase": pump_phase, "exitRatio": js_num(round_fixed(exit_ratio_raw, 2)), "recentBuyRatio": js_num(recent_buy_ratio),
        "socialSpike": social_spike, "galaxyScore": sg("galaxyScore"), "sentiment": sg("sentiment"),
        "socialVolume24h": sg("socialVolume24h"), "socialEnabled": social.is_object(), "socialSource": social_source,
    });

    json!({
        "token": {"symbol": str_or(&info["symbol"], &mc["symbol"]), "name": str_or(&info["name"], &mc["name"]), "contract": contract},
        "marketCap": nn(&mc["marketCap"]), "fdv": nn(&mc["fdv"]),
        "chart": series, "currentPrice": js_num(cp),
        "avgEntry": avg_entry, "avgExit": avg_exit, "lastEntry": last_entry, "whalePnl": whale_pnl,
        "liquidity": liquidity, "pumpSignal": pump_signal, "leaderboard": leaderboard,
        "accumulated": js_num(in_amt), "distributed": js_num(out_amt), "recentIn": js_num(rin_amt),
        "exitRatio": js_num(exit_ratio_raw),
        "holdersAnalyzed": js_num(holders.len() as f64), "txCount": js_num(tx_count as f64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_asset_flow_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/assetflow-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/assetflow-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = compute_asset_flow(
                c["contract"].as_str().unwrap(),
                &c["info"],
                c["holders"].as_array().unwrap(),
                &c["per"],
                c["series"].as_array().unwrap(),
                &c["mc"],
                &c["social"],
                c["now"].as_f64().unwrap(),
            );
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
