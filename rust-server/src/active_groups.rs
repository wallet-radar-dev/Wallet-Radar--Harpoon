// Pure reduction core of server/active-groups.js `analyzeGroup`: given a
// coordinated cluster and its members' fresh (7d) on-chain activity, roll up the
// group's volume/stance/heat/hot flags. The per-member fetch + the scan loop are
// the worker's job in main.rs (gated, smoke-tested — external data, no byte-parity
// live). This reduction is golden-tested byte-for-byte.

use crate::jsmath::{js_num, js_round};
use serde_json::{json, Value};

const HOT_VOL: f64 = 1_000_000.0;
const HOT_TX: i64 = 20;
const MAX_MEMBERS: usize = 8;

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0 && !x.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// analyzeGroup(c, activities, now). `activities[i]` is member i's `{txs,inUsd,
/// outUsd,last}` (or Null when its fetch threw — skipped, as in JS).
pub fn analyze_group(c: &Value, activities: &[Value], now: i64) -> Value {
    let empty = vec![];
    let all_members = c["members"].as_array().unwrap_or(&empty);
    let members: Vec<&Value> = all_members.iter().take(MAX_MEMBERS).collect();

    let (mut txs, mut in_usd, mut out_usd, mut last, mut active) = (0i64, 0.0f64, 0.0f64, 0i64, 0i64);
    for (i, _m) in members.iter().enumerate() {
        let a = activities.get(i).unwrap_or(&Value::Null);
        if a.is_null() {
            continue;
        }
        let atx = a["txs"].as_i64().unwrap_or(0);
        txs += atx;
        in_usd += f(&a["inUsd"]);
        out_usd += f(&a["outUsd"]);
        let al = a["last"].as_i64().unwrap_or(0);
        if al > last {
            last = al;
        }
        if atx > 0 {
            active += 1;
        }
    }
    let vol = in_usd + out_usd;
    let stance = if in_usd > out_usd * 1.2 {
        "accumulate"
    } else if out_usd > in_usd * 1.2 {
        "distribute"
    } else {
        "mixed"
    };
    let recency = if last > 0 { (5.0 - (now - last) as f64 / 86400000.0).max(0.0) } else { 0.0 };
    let heat = js_round(vol / 1e6 * 2.0 + txs as f64 * 0.5 + recency);

    let key = if truthy(&c["key"]) { c["key"].clone() } else { c["id"].clone() };
    let members_out: Vec<Value> = members.iter().map(|m| json!({"addr": m["addr"], "totalUsd": m["totalUsd"]})).collect();

    json!({
        "key": key,
        "label": c["label"],
        "combinedUsd": c["combinedUsd"],
        "assets": if c["assets"].is_array() { c["assets"].clone() } else { json!([]) },
        "memberCount": all_members.len(),
        "activeMembers": active,
        "txs7d": txs,
        "inUsd7d": js_num(js_round(in_usd)),
        "outUsd7d": js_num(js_round(out_usd)),
        "vol7d": js_num(js_round(vol)),
        "stance": stance,
        "heat": js_num(heat),
        "lastActivityTs": last,
        "hot": vol >= HOT_VOL || txs >= HOT_TX,
        "members": members_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_group_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("activegroups-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("activegroups-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let empty = vec![];
            let acts = c["activities"].as_array().unwrap_or(&empty);
            let got = analyze_group(&c["c"], acts, c["now"].as_i64().unwrap());
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
