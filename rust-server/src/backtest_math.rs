//! Port of server/backtest-math.js — pure backtest math, no network.
//! Used by portfolio-backtest.js. All rounding goes through jsmath so the
//! user-facing metrics match `+(x).toFixed(n)` exactly.

use crate::jsmath::round_fixed;
use indexmap::IndexMap;
use std::collections::HashMap;

pub struct Ev {
    pub ts: f64,
    pub side_in: bool, // side === "in"
    pub amount: f64,
}

/// backtest-math.js:5-11. Events assumed pre-sorted by ts; two-pointer.
pub fn e_timeline(events: &[Ev], times: &[f64]) -> Vec<f64> {
    let mut bought = 0.0;
    let mut sold = 0.0;
    let mut j = 0;
    times
        .iter()
        .map(|&t| {
            while j < events.len() && events[j].ts <= t {
                let e = &events[j];
                j += 1;
                if e.side_in {
                    bought += e.amount;
                } else {
                    sold += e.amount;
                }
            }
            if bought > 0.0 {
                (sold / bought).min(1.0)
            } else {
                0.0
            }
        })
        .collect()
}

/// backtest-math.js:14-18. DCA-out ladder.
pub fn position_frac(e: f64, exit_e: f64) -> f64 {
    if e >= exit_e {
        return 0.0;
    }
    let mut sold = 0.0;
    for (thr, frac) in [(0.15, 0.25), (0.25, 0.50), (0.35, 0.75)] {
        if e >= thr {
            sold = frac;
        }
    }
    1.0 - sold
}

/// backtest-math.js:21-23. Nearest price by |ts-t|; `d < bd` strict, so on a tie
/// the earlier series point wins. None when series is empty.
pub fn sample_monthly(series: &[(f64, f64)], times: &[f64]) -> Vec<Option<f64>> {
    times
        .iter()
        .map(|&t| {
            let mut best = None;
            let mut bd = f64::INFINITY;
            for &(ts, price) in series {
                let d = (ts - t).abs();
                if d < bd {
                    bd = d;
                    best = Some(price);
                }
            }
            best
        })
        .collect()
}

pub struct Sim {
    pub equity: Vec<f64>,
    pub rets: Vec<f64>,
}

/// backtest-math.js:26-34. `for (const s in wmap)` iterates insertion order, so
/// the per-step return sum accumulates in that order — IndexMap preserves it.
/// `p0 && p1` skips missing or zero prices (JS truthiness).
pub fn simulate(wmap: &IndexMap<String, f64>, series_map: &HashMap<String, Vec<f64>>, n_times: usize) -> Sim {
    let mut val = 1.0;
    let mut equity = vec![1.0];
    let mut rets = Vec::new();
    for i in 1..n_times {
        let mut r = 0.0;
        for (s, w) in wmap {
            if let Some(series) = series_map.get(s) {
                let p0 = series.get(i - 1).copied().unwrap_or(0.0);
                let p1 = series.get(i).copied().unwrap_or(0.0);
                if p0 != 0.0 && p1 != 0.0 {
                    r += w * (p1 / p0 - 1.0);
                }
            }
        }
        val *= 1.0 + r;
        equity.push(val);
        rets.push(r);
    }
    Sim { equity, rets }
}

pub struct Metrics {
    pub total_ret: f64,
    pub cagr: f64,
    pub max_dd: f64,
    pub vol_ann: f64,
    pub sharpe: Option<f64>,
}

/// backtest-math.js:37-46. Population std; CAGR floored at total=-0.999.
pub fn metrics(sim: &Sim, months: f64) -> Metrics {
    let total = sim.equity.last().copied().unwrap_or(1.0) - 1.0;
    let yrs = (months / 12.0).max(0.1);
    let cagr = (1.0 + total.max(-0.999)).powf(1.0 / yrs) - 1.0;

    let mut peak = f64::NEG_INFINITY;
    let mut mdd = 0.0;
    for &v in &sim.equity {
        if v > peak {
            peak = v;
        }
        let dd = (v - peak) / peak;
        if dd < mdd {
            mdd = dd;
        }
    }

    let n = sim.rets.len().max(1) as f64;
    let mean = sim.rets.iter().sum::<f64>() / n;
    let var = sim.rets.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / n;
    let sd = var.sqrt();
    let vol_ann = sd * 12f64.sqrt();

    Metrics {
        total_ret: round_fixed(total * 100.0, 1),
        cagr: round_fixed(cagr * 100.0, 1),
        max_dd: round_fixed(mdd * 100.0, 1),
        vol_ann: round_fixed(vol_ann * 100.0, 1),
        sharpe: if vol_ann > 0.0 { Some(round_fixed(cagr / vol_ann, 2)) } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "{a} vs {b}");
    }

    #[test]
    fn e_timeline_golden() {
        let events = [
            Ev { ts: 10.0, side_in: true, amount: 100.0 },
            Ev { ts: 20.0, side_in: false, amount: 30.0 },
            Ev { ts: 30.0, side_in: false, amount: 50.0 },
            Ev { ts: 40.0, side_in: true, amount: 20.0 },
        ];
        let got = e_timeline(&events, &[5.0, 15.0, 25.0, 35.0, 45.0]);
        let want = [0.0, 0.0, 0.3, 0.8, 0.6666666666666666];
        for (g, w) in got.iter().zip(want) {
            approx(*g, w);
        }
    }

    #[test]
    fn position_frac_ladder() {
        let want = [1.0, 0.75, 0.75, 0.5, 0.5, 0.25, 0.25, 0.0];
        for (e, w) in [0.1, 0.15, 0.2, 0.25, 0.3, 0.35, 0.5, 0.9].iter().zip(want) {
            approx(position_frac(*e, 0.9), w);
        }
    }

    #[test]
    fn sample_monthly_nearest() {
        let series = [(0.0, 1.5), (12.0, 2.0), (24.0, 1.2)];
        let got = sample_monthly(&series, &[3.0, 11.0, 30.0, 100.0]);
        assert_eq!(got, vec![Some(1.5), Some(2.0), Some(1.2), Some(1.2)]);
    }

    #[test]
    fn simulate_and_metrics_golden() {
        let mut wmap = IndexMap::new();
        wmap.insert("BTC".to_string(), 0.6);
        wmap.insert("ETH".to_string(), 0.4);
        let mut sm = HashMap::new();
        sm.insert("BTC".to_string(), vec![100.0, 110.0, 105.0, 120.0]);
        sm.insert("ETH".to_string(), vec![50.0, 55.0, 60.0, 54.0]);
        let sim = simulate(&wmap, &sm, 4);
        // equity[3] from Node: 1.160742857142857
        approx(*sim.equity.last().unwrap(), 1.160742857142857);
        approx(sim.rets[0], 0.10000000000000009);

        let m = metrics(&sim, 6.0);
        approx(m.total_ret, 16.1);
        approx(m.cagr, 34.7);
        approx(m.max_dd, 0.0);
        approx(m.vol_ann, 12.9);
        approx(m.sharpe.unwrap(), 2.68);
    }

    #[test]
    fn metrics_empty_rets() {
        let m = metrics(&Sim { equity: vec![1.0], rets: vec![] }, 6.0);
        approx(m.total_ret, 0.0);
        approx(m.cagr, 0.0);
        approx(m.vol_ann, 0.0);
        assert_eq!(m.sharpe, None);
    }
}
