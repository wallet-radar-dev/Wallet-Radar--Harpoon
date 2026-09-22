// Pure assembly of `GET /api/portfolio-backtest` (server/portfolio-backtest.js
// runPortfolioBacktest): monthly-rebalanced Core-Satellite backtest with metrics
// vs benchmarks (BTC/ETH/60-40/equal-weight) plus a whale-phase dynamic backtest.
// The network fetches — buildPortfolio (`pf`), per-symbol priceChart (`charts`),
// per-satellite assetWhaleEvents (`whale_events`) — are the route's job; this
// kernel does the deterministic assembly over already-ported backtest_math.
// Golden-tested byte-for-byte. buildPortfolio is called with no flowTilt, so this
// route carries no phase-worker dependency.

use crate::backtest_math::{self, Ev, Sim};
use crate::jsmath::{js_num, round_fixed};
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub fn is_stable(s: &str) -> bool {
    matches!(s, "USDC" | "USDT" | "DAI" | "FDUSD" | "USDE" | "TUSD")
}

fn metrics_json(sim: &Sim, months: f64) -> Value {
    let m = backtest_math::metrics(sim, months);
    json!({
        "totalRet": js_num(m.total_ret),
        "cagr": js_num(m.cagr),
        "maxDD": js_num(m.max_dd),
        "volAnn": js_num(m.vol_ann),
        "sharpe": m.sharpe.map(js_num).unwrap_or(Value::Null),
    })
}

/// runPortfolioBacktest after the fetches. `pf` is a buildPortfolio result,
/// `contract_of` maps SYM→contract, `charts` maps SYM→(series array | "stable"),
/// `whale_events` maps satellite SYM→[{ts,side,amount}].
pub fn portfolio_backtest(pf: &Value, contract_of: &Value, charts: &Value, whale_events: &Value) -> Value {
    let empty: Vec<Value> = Vec::new();
    let holdings = |sl: &str| -> Vec<Value> {
        pf["sleeves"][sl]["holdings"].as_array().cloned().unwrap_or_default()
    };

    // weights[sym] += h.weight/100, in core→satellite→stable holding order
    let mut weights: IndexMap<String, f64> = IndexMap::new();
    for sl in ["core", "satellite", "stable"] {
        for h in pf["sleeves"][sl]["holdings"].as_array().unwrap_or(&empty) {
            let sym = h["sym"].as_str().unwrap_or("").to_string();
            *weights.entry(sym).or_insert(0.0) += h["weight"].as_f64().unwrap_or(0.0) / 100.0;
        }
    }
    let syms: Vec<String> = weights.keys().cloned().collect();

    let ranged: Vec<String> = syms.iter().filter(|s| charts.get(s.as_str()).map(|v| v.is_array()).unwrap_or(false)).cloned().collect();
    if ranged.len() < 3 {
        return json!({ "error": "недостатньо цінової історії" });
    }
    let mut start = 0.0f64;
    let mut end = f64::INFINITY;
    for s in &ranged {
        let ser = charts[s.as_str()].as_array().unwrap();
        let first = ser.first().map(|p| p["ts"].as_f64().unwrap_or(0.0)).unwrap_or(0.0);
        let last = ser.last().map(|p| p["ts"].as_f64().unwrap_or(0.0)).unwrap_or(0.0);
        start = start.max(first);
        end = end.min(last);
    }
    if !(end > start) {
        return json!({ "error": "немає спільного діапазону історії" });
    }
    let month = 30.4 * 24.0 * 3600.0 * 1000.0;
    let mut times: Vec<f64> = Vec::new();
    let mut t = start;
    while t <= end {
        times.push(t);
        t += month;
    }
    if times.len() < 4 {
        return json!({ "error": "замало місяців спільної історії" });
    }

    // monthly[sym] as prices (None→0.0, which simulate skips like JS `p0 && p1`)
    let mut monthly: HashMap<String, Vec<f64>> = HashMap::new();
    for s in &ranged {
        let ser: Vec<(f64, f64)> = charts[s.as_str()].as_array().unwrap().iter()
            .map(|p| (p["ts"].as_f64().unwrap_or(0.0), p["price"].as_f64().unwrap_or(0.0)))
            .collect();
        let sampled = backtest_math::sample_monthly(&ser, &times);
        monthly.insert(s.clone(), sampled.iter().map(|o| o.unwrap_or(0.0)).collect());
    }
    for s in &syms {
        if is_stable(s) {
            monthly.insert(s.clone(), times.iter().map(|_| 1.0).collect());
        }
    }

    let have: Vec<String> = syms.iter().filter(|s| monthly.contains_key(*s)).cloned().collect();
    let wsum: f64 = have.iter().map(|s| *weights.get(s).unwrap()).sum::<f64>();
    let wsum = if wsum == 0.0 { 1.0 } else { wsum };
    let mut w_map: IndexMap<String, f64> = IndexMap::new();
    for s in &have {
        w_map.insert(s.clone(), *weights.get(s).unwrap() / wsum);
    }
    let months_n = times.len() - 1;
    let months = months_n as f64;

    let sim_w = |pairs: &[(&str, f64)]| -> Sim {
        let mut m: IndexMap<String, f64> = IndexMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), *v);
        }
        backtest_math::simulate(&m, &monthly, times.len())
    };

    let port = backtest_math::simulate(&w_map, &monthly, times.len());

    let mut benches = Map::new();
    if monthly.contains_key("BTC") {
        benches.insert("BTC hold".into(), metrics_json(&sim_w(&[("BTC", 1.0)]), months));
    }
    if monthly.contains_key("ETH") {
        benches.insert("ETH hold".into(), metrics_json(&sim_w(&[("ETH", 1.0)]), months));
    }
    if monthly.contains_key("BTC") && monthly.contains_key("ETH") {
        benches.insert("60/40 BTC-ETH".into(), metrics_json(&sim_w(&[("BTC", 0.6), ("ETH", 0.4)]), months));
    }
    let mut ew: IndexMap<String, f64> = IndexMap::new();
    for s in &have {
        ew.insert(s.clone(), 1.0 / have.len() as f64);
    }
    benches.insert("Рівна вага".into(), metrics_json(&backtest_math::simulate(&ew, &monthly, times.len()), months));

    // ── dynamic (event-driven) backtest ──
    let exit_e = pf["exitE"].as_f64().filter(|v| *v != 0.0).unwrap_or(48.0) / 100.0;
    let sat_all = holdings("satellite");
    let sat_hold: Vec<&Value> = sat_all.iter().filter(|h| monthly.contains_key(h["sym"].as_str().unwrap_or(""))).collect();
    let mut core_w: IndexMap<String, f64> = IndexMap::new();
    for h in holdings("core").iter() {
        let sym = h["sym"].as_str().unwrap_or("");
        if monthly.contains_key(sym) {
            core_w.insert(sym.to_string(), h["weight"].as_f64().unwrap_or(0.0) / 100.0);
        }
    }
    let stable_syms2: Vec<String> = holdings("stable").iter()
        .map(|h| h["sym"].as_str().unwrap_or("").to_string())
        .filter(|s| monthly.contains_key(s))
        .collect();
    let stable_base: f64 = holdings("stable").iter().map(|h| {
        let sym = h["sym"].as_str().unwrap_or("");
        if monthly.contains_key(sym) { h["weight"].as_f64().unwrap_or(0.0) / 100.0 } else { 0.0 }
    }).sum();
    let sat_target = pf["sleeves"]["satellite"]["target"].as_f64().unwrap_or(0.0) / 100.0;
    let mut sat_base_w: HashMap<String, f64> = HashMap::new();
    for h in &sat_hold {
        sat_base_w.insert(h["sym"].as_str().unwrap_or("").to_string(), h["weight"].as_f64().unwrap_or(0.0) / 100.0);
    }
    let mut e_tl: HashMap<String, Vec<f64>> = HashMap::new();
    for h in &sat_hold {
        let sym = h["sym"].as_str().unwrap_or("");
        let c = contract_of.get(sym).and_then(|v| v.as_str()).unwrap_or("");
        let evs: Vec<Ev> = if !c.is_empty() {
            whale_events.get(sym).and_then(|v| v.as_array()).map(|arr| arr.iter().map(|e| Ev {
                ts: e["ts"].as_f64().unwrap_or(0.0),
                side_in: e["side"].as_str() == Some("in"),
                amount: e["amount"].as_f64().unwrap_or(0.0),
            }).collect()).unwrap_or_default()
        } else {
            vec![]
        };
        e_tl.insert(sym.to_string(), backtest_math::e_timeline(&evs, &times));
    }

    let mut dval = 1.0;
    let mut deq = vec![1.0];
    let mut drets: Vec<f64> = Vec::new();
    for i in 1..times.len() {
        let mut wmap = core_w.clone();
        let mut sat_real = 0.0;
        for h in &sat_hold {
            let sym = h["sym"].as_str().unwrap_or("");
            let w = sat_base_w[sym] * backtest_math::position_frac(e_tl[sym][i - 1], exit_e);
            if w > 0.0 {
                *wmap.entry(sym.to_string()).or_insert(0.0) += w;
                sat_real += w;
            }
        }
        let st_total = stable_base + (sat_target - sat_real).max(0.0);
        let denom = if stable_syms2.is_empty() { 1.0 } else { stable_syms2.len() as f64 };
        for s in &stable_syms2 {
            *wmap.entry(s.clone()).or_insert(0.0) += st_total / denom;
        }
        let mut r = 0.0;
        for (s, w) in &wmap {
            let series = monthly.get(s);
            let p0 = series.and_then(|v| v.get(i - 1)).copied().unwrap_or(0.0);
            let p1 = series.and_then(|v| v.get(i)).copied().unwrap_or(0.0);
            if p0 != 0.0 && p1 != 0.0 {
                r += w * (p1 / p0 - 1.0);
            }
        }
        dval *= 1.0 + r;
        deq.push(dval);
        drets.push(r);
    }
    let port_dyn = Sim { equity: deq.clone(), rets: drets };

    let assets: Vec<Value> = have.iter().map(|s| json!(s)).collect();
    let mut weights_out = Map::new();
    for s in &have {
        weights_out.insert(s.clone(), js_num(round_fixed(w_map[s] * 100.0, 1)));
    }
    let static_eq: Vec<Value> = port.equity.iter().map(|v| js_num(round_fixed(*v, 3))).collect();
    let dynamic_eq: Vec<Value> = deq.iter().map(|v| js_num(round_fixed(*v, 3))).collect();
    let btc_eq = if monthly.contains_key("BTC") {
        Value::Array(sim_w(&[("BTC", 1.0)]).equity.iter().map(|v| js_num(round_fixed(*v, 3))).collect())
    } else {
        Value::Null
    };

    json!({
        "period": { "months": months_n as i64, "years": js_num(round_fixed(months / 12.0, 1)) },
        "assets": assets,
        "weights": Value::Object(weights_out),
        "portfolio": metrics_json(&port, months),
        "portfolioDynamic": metrics_json(&port_dyn, months),
        "benchmarks": Value::Object(benches),
        "equity": { "static": static_eq, "dynamic": dynamic_eq, "btc": btc_eq },
        "rebalance": "щомісяця до цільових ваг",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portfolio_backtest_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/pbacktest-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/pbacktest-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = portfolio_backtest(&c["pf"], &c["contractOf"], &c["charts"], &c["whaleEvents"]);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
