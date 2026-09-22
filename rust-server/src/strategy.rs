// Pure kernel of `GET /api/strategy` (server/strategy.js `assetStrategy`, the
// part after computeAssetFlow): map the whale exit-ratio E into a DCA phase /
// action / colour and entry-exit ladder. The route (main.rs) computes the
// asset-flow object `f` and the `rising` trend flag (history-based; a cold
// process falls back to `distributed > 0`). Golden-tested byte-for-byte.

use crate::jsmath::{js_num, js_round, round_fixed};
use serde_json::{json, Value};

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// strategy.js `PARAMS.exitSteps` — E threshold → cumulative sold target.
const EXIT_STEPS: [(f64, f64); 4] = [(0.15, 0.25), (0.25, 0.50), (0.35, 0.75), (0.48, 1.00)];
const ENTRY_CUTOFF_E: f64 = 0.10;
const ENTRY_TRANCHES: i64 = 4;

/// strategy.js `assetStrategy` after the fetch: `f` is the computeAssetFlow
/// result, `rising` the distribution-trend flag.
pub fn asset_strategy(af: &Value, rising: bool) -> Value {
    let e = f(&af["exitRatio"]); // f.exitRatio || 0
    let accumulated = f(&af["accumulated"]);
    let recent_in = f(&af["recentIn"]);
    let distributed = f(&af["distributed"]);

    let (phase, action, color, sold_target): (&str, String, &str, f64) = if accumulated <= 0.0 {
        ("neutral", "Недостатньо даних по китах для сигналу".to_string(), "#94a3b8", 0.0)
    } else if e < ENTRY_CUTOFF_E && (recent_in > 0.0 || distributed == 0.0) {
        ("accumulation", "DCA-IN: кити накопичують — додавай транші слідом (орієнтир ціни — середній вхід китів)".to_string(), "#34d399", 0.0)
    } else if e < EXIT_STEPS[0].0 {
        ("markup", "ТРИМАТИ: рух у розпалі, суттєвої роздачі ще немає".to_string(), "#5b9dff", 0.0)
    } else {
        let mut st = 0.0;
        for (se, ss) in EXIT_STEPS {
            if e >= se {
                st = ss;
            }
        }
        if e >= 0.48 {
            ("exit", "ВИХІД: 48% накопиченої емісії вийшло — повністю закрити позицію".to_string(), "#f87171", st)
        } else {
            ("distribution", format!("DCA-OUT: кити роздають — позиція має бути продана на {}%", js_round(st * 100.0) as i64), "#fb923c", st)
        }
    };

    let entry_plan: Vec<Value> = (0..ENTRY_TRANCHES)
        .map(|i| json!({"tranche": i + 1, "pct": js_round(100.0 / ENTRY_TRANCHES as f64) as i64}))
        .collect();
    let exit_plan: Vec<Value> = EXIT_STEPS
        .iter()
        .map(|(se, ss)| json!({"atE": js_round(se * 100.0) as i64, "sellTo": js_round(ss * 100.0) as i64, "done": e >= *se, "final": *ss >= 1.0}))
        .collect();

    json!({
        "asset": af["token"].clone(),
        "marketCap": af["marketCap"].clone(),
        "currentPrice": af["currentPrice"].clone(),
        "exitRatio": js_num(round_fixed(e * 100.0, 1)),
        "rising": rising,
        "accumulated": js_num(js_round(accumulated)),
        "distributed": js_num(js_round(distributed)),
        "avgEntry": af["avgEntry"].clone(),
        "avgExit": af["avgExit"].clone(),
        "lastEntry": af["lastEntry"].clone(),
        "holdersAnalyzed": af["holdersAnalyzed"].clone(),
        "phase": phase,
        "action": action,
        "color": color,
        "soldTarget": js_round(sold_target * 100.0) as i64,
        "dca": { "entry": entry_plan, "exit": exit_plan, "finalExitE": 48 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_strategy_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/strategy-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/strategy-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = asset_strategy(&c["f"], c["rising"].as_bool().unwrap());
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
