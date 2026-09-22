//! Port of server/strategy-backtest.js `runStrategyBacktest` — the "DCA-behind-a-
//! whale" strategy backtest over a fixed universe of liquid ERC-20s.
//!
//! `simulate` (one asset's DCA-in / laddered-exit vs buy-and-hold), `asset_result`
//! (an asset's row across the exit thresholds), and `strategy_summary` (the whole
//! response: averages, calibration, best threshold) are pure and golden-tested.
//! The per-asset event reconstruction (holders × token transfers × price chart)
//! stays in main.rs. Only +−×÷ arithmetic, so results are bit-exact vs Node.

use crate::jsmath::{js_num, js_round, round_fixed};
use serde_json::{json, Map, Value};

const TRANCHE: f64 = 100.0; // $ per DCA-in tranche
const ENTRY_CUTOFF: f64 = 0.10; // buy while E < 10%

/// JS `String(number)` for the values interpolated as byThr keys / fields.
fn numstr(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// strategy-backtest.js `simulate(events, finalE)` — DCA accumulate while the
/// distribution ratio E<10%, exit on the 15/25/35→final ladder, vs buy-and-hold.
/// `Null` when no entry ever fires. The `finalE` step is applied LAST, so it wins
/// ties over the earlier ladder rungs exactly as JS's sequential `target = frac`.
pub fn simulate(events: &[Value], final_e: f64) -> Value {
    let steps = [(0.15, 0.25), (0.25, 0.50), (0.35, 0.75), (final_e, 1.0)];
    let (mut bought, mut sold, mut units, mut invested, mut proceeds, mut sold_frac, mut peak_units) =
        (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let mut entered = false;
    let (mut first_price, mut last_price) = (0.0f64, 0.0f64);

    for e in events {
        let price = e["price"].as_f64().unwrap_or(0.0);
        if price == 0.0 {
            continue; // !e.price
        }
        last_price = price;
        let amount = e["amount"].as_f64().unwrap_or(0.0);
        let is_in = e["side"].as_str() == Some("in");
        if is_in {
            bought += amount;
        } else {
            sold += amount;
        }
        let ratio = if bought > 0.0 { sold / bought } else { 0.0 };
        if is_in && ratio < ENTRY_CUTOFF {
            units += TRANCHE / price;
            invested += TRANCHE;
            peak_units = units;
            if !entered {
                entered = true;
                first_price = price;
            }
        }
        if entered {
            let mut target = 0.0;
            for (thr, frac) in steps {
                if ratio >= thr {
                    target = frac;
                }
            }
            if target > sold_frac {
                proceeds += (target - sold_frac) * peak_units * price;
                sold_frac = target;
            }
        }
    }
    if !entered || invested <= 0.0 {
        return Value::Null;
    }
    proceeds += (1.0 - sold_frac) * peak_units * last_price; // remainder at last price
    let strat_ret = proceeds / invested - 1.0;
    let hold_ret = if first_price > 0.0 { last_price / first_price - 1.0 } else { 0.0 };
    json!({
        "stratRet": js_num(strat_ret),
        "holdRet": js_num(hold_ret),
        "invested": js_num(js_round(invested)),
        "finalE": js_num(if bought > 0.0 { sold / bought } else { 0.0 }),
        "exitedFully": sold_frac >= 1.0,
    })
}

/// One asset's backtest row: base run at 0.48 plus per-threshold strat returns, or
/// a `{sym, skipped}` marker (`<4` events, or no entry). `thresholds` are the
/// exit-cutoff sweep.
pub fn asset_result(sym: &str, events: &[Value], thresholds: &[f64]) -> Value {
    if events.len() < 4 {
        return json!({"sym": sym, "skipped": "мало даних"});
    }
    let base = simulate(events, 0.48);
    if base.is_null() {
        return json!({"sym": sym, "skipped": "немає входу"});
    }
    let strat = base["stratRet"].as_f64().unwrap_or(0.0);
    let hold = base["holdRet"].as_f64().unwrap_or(0.0);
    let mut by_thr = Map::new();
    for &thr in thresholds {
        let r = simulate(events, thr);
        let v = if r.is_null() { Value::Null } else { js_num(round_fixed(r["stratRet"].as_f64().unwrap_or(0.0) * 100.0, 1)) };
        by_thr.insert(numstr(thr), v);
    }
    json!({
        "sym": sym,
        "events": js_num(events.len() as f64),
        "stratRet": js_num(round_fixed(strat * 100.0, 1)),
        "holdRet": js_num(round_fixed(hold * 100.0, 1)),
        "edge": js_num(round_fixed((strat - hold) * 100.0, 1)),
        "exitedFully": base["exitedFully"].clone(),
        "byThr": Value::Object(by_thr),
    })
}

/// strategy-backtest.js `runStrategyBacktest` assembly: averages, win/beat rates,
/// per-threshold calibration and the best threshold, over the built `per_asset`.
pub fn strategy_summary(per_asset: &[Value], thresholds: &[f64], universe_total: usize) -> Value {
    let valid: Vec<&Value> = per_asset.iter().filter(|x| x["stratRet"].is_number()).collect();
    let avg = |arr: &[f64]| if arr.is_empty() { 0.0 } else { round_fixed(arr.iter().sum::<f64>() / arr.len() as f64, 1) };
    let median = |arr: &[f64]| {
        if arr.is_empty() {
            return 0.0;
        }
        let mut s = arr.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        round_fixed(s[s.len() / 2], 1)
    };
    let strat_rets: Vec<f64> = valid.iter().map(|x| x["stratRet"].as_f64().unwrap_or(0.0)).collect();
    let hold_rets: Vec<f64> = valid.iter().map(|x| x["holdRet"].as_f64().unwrap_or(0.0)).collect();
    let edges: Vec<f64> = valid.iter().map(|x| x["edge"].as_f64().unwrap_or(0.0)).collect();

    let calibration: Vec<Value> = thresholds
        .iter()
        .map(|&thr| {
            let vals: Vec<f64> = valid.iter().filter_map(|x| x["byThr"][numstr(thr)].as_f64()).collect();
            json!({"threshold": js_num(js_round(thr * 100.0)), "avgRet": js_num(avg(&vals))})
        })
        .collect();
    // best = calibration.slice().sort(avgRet desc)[0]  (stable; original left unsorted)
    let best = {
        let mut c = calibration.clone();
        c.sort_by(|a, b| b["avgRet"].as_f64().unwrap_or(0.0).partial_cmp(&a["avgRet"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
        c.into_iter().next().unwrap_or(Value::Null)
    };

    let n = valid.len() as f64;
    let win_rate = if valid.is_empty() { 0.0 } else { round_fixed((strat_rets.iter().filter(|&&v| v > 0.0).count() as f64 / n) * 100.0, 0) };
    let beat_hold = if valid.is_empty() {
        0.0
    } else {
        round_fixed((valid.iter().filter(|x| x["stratRet"].as_f64().unwrap_or(0.0) > x["holdRet"].as_f64().unwrap_or(0.0)).count() as f64 / n) * 100.0, 0)
    };

    json!({
        "assetsTested": js_num(valid.len() as f64),
        "assetsTotal": js_num(universe_total as f64),
        "summary": {
            "avgStrategyRet": js_num(avg(&strat_rets)),
            "medianStrategyRet": js_num(median(&strat_rets)),
            "avgHoldRet": js_num(avg(&hold_rets)),
            "avgEdge": js_num(avg(&edges)),
            "winRate": js_num(win_rate),
            "beatHoldRate": js_num(beat_hold),
        },
        "calibration": calibration,
        "bestThreshold": best,
        "params": {"trancheUsd": js_num(TRANCHE), "entryCutoff": js_num(ENTRY_CUTOFF * 100.0), "exitLadder": "15/25/35→25/50/75%, final=поріг"},
        "perAsset": per_asset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_result_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/stratbt-asset-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/stratbt-asset-golden.json")).unwrap();
        let thr = [0.30, 0.40, 0.48, 0.55, 0.70];
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(asset_result(c["sym"].as_str().unwrap(), c["events"].as_array().unwrap(), &thr), golden[i], "case {i}");
        }
    }

    #[test]
    fn strategy_summary_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/stratbt-summary-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/stratbt-summary-golden.json")).unwrap();
        let thr = [0.30, 0.40, 0.48, 0.55, 0.70];
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(strategy_summary(c["perAsset"].as_array().unwrap(), &thr, c["universeTotal"].as_u64().unwrap() as usize), golden[i], "case {i}");
        }
    }
}
