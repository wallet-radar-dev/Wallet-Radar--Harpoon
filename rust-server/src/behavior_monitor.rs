// Pure core of server/behavior-monitor.js `diff(base, snap)`: compare a wallet's
// armed baseline behavioural snapshot against a fresh one and list the meaningful
// changes (stance flip, dominant-category change, sell/buy-share jumps, netflow
// sign flip, activity spike). The periodic scan + broadcast is the worker's job in
// main.rs (gated). Golden-tested byte-for-byte.

use serde_json::{json, Value};

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
// Math.sign — -1 / 0 / 1 (netUsd is never NaN here).
fn sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// behavior-monitor.js `diff`. `base` is the stored baseline (`Null` when unset →
/// no changes); `snap` is the fresh snap_of output.
pub fn diff(base: &Value, snap: &Value) -> Vec<Value> {
    let mut ch: Vec<Value> = Vec::new();
    if base.is_null() {
        return ch;
    }
    if base["stance"] != snap["stance"] {
        ch.push(json!({"type": "stance", "from": base["stance"], "to": snap["stance"]}));
    }
    if base["dominant"] != snap["dominant"] {
        ch.push(json!({"type": "dominant", "from": base["dominant"], "to": snap["dominant"]}));
    }
    if f(&snap["sellShare"]) - f(&base["sellShare"]) >= 0.25 {
        ch.push(json!({"type": "sell_up"}));
    }
    if f(&snap["buyShare"]) - f(&base["buyShare"]) >= 0.25 {
        ch.push(json!({"type": "buy_up"}));
    }
    let sn = f(&snap["netUsd"]);
    if sign(f(&base["netUsd"])) != sign(sn) && sn.abs() > 1000.0 {
        ch.push(json!({"type": "netflow_flip"}));
    }
    let btx = f(&base["txPerDay"]);
    if btx > 0.1 && f(&snap["txPerDay"]) >= btx * 2.0 {
        ch.push(json!({"type": "activity_spike"}));
    }
    ch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("bdiff-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("bdiff-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = diff(&c["base"], &c["snap"]);
            assert_eq!(Value::Array(got), golden[i], "case {i}");
        }
    }
}
