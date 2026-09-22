//! Port of the pure core of server/coordinated-moves.js — the strongest
//! collusion signal: when ≥3 members of one coordinated network move the SAME
//! asset within a 72-hour window, that's a synchronised action (accumulation /
//! distribution), not noise.
//!
//! The network fetch (`bs.transfersRich` per member), persisted state, alerting,
//! and `Date.now()` bookkeeping stay in the JS. This takes the coordinated
//! networks with each member's already-fetched recent transfers attached
//! (`_per`, aligned to `members`) and reproduces the per-asset synchronous-window
//! detection exactly. Output matches Node byte-for-byte.

use crate::jsmath::{js_num, js_round};
use indexmap::IndexSet;
use serde_json::{Map, Value};

const WINDOW_MS: f64 = 72.0 * 3600.0 * 1000.0;
const MIN_ACTORS: usize = 3;

/// detectCoordinatedMoves()'s detection loop. `nets` are coordinated networks
/// (`members:[{addr}]`, `sharedAssets:[{symbol,marketCap}]`, `conductor`), each
/// carrying `_per`: an array of transfer arrays aligned to `members`.
pub fn detect_coordinated_moves_core(nets: &[Value]) -> Value {
    let mut moves: Vec<(f64, Value)> = Vec::new();

    for net in nets.iter().take(10) {
        let empty = vec![];
        let net_members = net["members"].as_array().unwrap_or(&empty);
        let members: Vec<&str> = net_members.iter().take(15).map(|m| m["addr"].as_str().unwrap_or("")).collect();

        // syms = new Set(sharedAssets.map(a => a.symbol)) — unique, insertion order.
        let shared_assets = net["sharedAssets"].as_array().unwrap_or(&empty);
        let mut syms: IndexSet<String> = IndexSet::new();
        for a in shared_assets {
            syms.insert(a["symbol"].as_str().unwrap_or("").to_string());
        }
        if syms.is_empty() {
            continue;
        }

        let per = net["_per"].as_array().unwrap_or(&empty);

        for sym in &syms {
            // For each member, its latest transfer of this asset (stable — earliest
            // index wins on a tie, matching sort((a,b)=>b.ts-a.ts)[0]).
            let mut actors: Vec<(&str, f64, bool)> = Vec::new();
            for (i, &member) in members.iter().enumerate() {
                let txs = per.get(i).and_then(|v| v.as_array()).unwrap_or(&empty);
                let mut best: Option<&Value> = None;
                for t in txs {
                    if t["asset"].as_str() == Some(sym.as_str()) {
                        let ts = t["ts"].as_f64().unwrap_or(f64::NAN);
                        if best.is_none() || ts > best.unwrap()["ts"].as_f64().unwrap_or(f64::NAN) {
                            best = Some(t);
                        }
                    }
                }
                if let Some(tx) = best {
                    // `if (tx && tx.ts)` — ts must be truthy (nonzero, non-NaN).
                    if let Some(ts) = tx["ts"].as_f64().filter(|&v| v != 0.0 && !v.is_nan()) {
                        let is_in = tx["to"].as_str() == Some(member);
                        actors.push((member, ts, is_in));
                    }
                }
            }
            if actors.len() < MIN_ACTORS {
                continue;
            }

            actors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let latest = actors[0].1;
            let within: Vec<&(&str, f64, bool)> = actors.iter().filter(|a| latest - a.1 < WINDOW_MS).collect();
            if within.len() < MIN_ACTORS {
                continue;
            }

            let ins = within.iter().filter(|a| a.2).count();
            let outs = within.len() - ins;
            let direction = if ins >= outs { "accumulate" } else { "distribute" };

            // key prefix: conductor, else the first member's address.
            let conductor = net["conductor"].as_str().filter(|s| !s.is_empty());
            let members0 = net_members.first().map(|m| m["addr"].as_str().unwrap_or("")).unwrap_or("");
            let key_prefix = conductor.unwrap_or(members0);
            let key = format!("{key_prefix}:{sym}:{}", js_round(latest / WINDOW_MS) as i64);

            // marketCap: the shared asset's, `|| null` (0/absent → null).
            let market_cap = shared_assets
                .iter()
                .find(|a| a["symbol"].as_str() == Some(sym.as_str()))
                .and_then(|a| a["marketCap"].as_f64())
                .filter(|&v| v != 0.0 && !v.is_nan())
                .map(js_num)
                .unwrap_or(Value::Null);

            let members_out: Vec<Value> = within.iter().map(|a| Value::String(a.0.to_string())).collect();
            let mv = json_obj(&[
                ("key", Value::String(key)),
                ("conductor", conductor.map(|c| Value::String(c.to_string())).unwrap_or(Value::Null)),
                ("asset", Value::String(sym.clone())),
                ("direction", Value::String(direction.to_string())),
                ("count", js_num(within.len() as f64)),
                ("totalMembers", js_num(members.len() as f64)),
                ("members", Value::Array(members_out)),
                ("latestTs", js_num(latest)),
                ("marketCap", market_cap),
            ]);
            moves.push((latest, mv));
        }
    }

    // sort by latestTs desc (stable); top 50.
    moves.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Value::Array(moves.into_iter().take(50).map(|(_, v)| v).collect())
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
    fn coordinated_moves_match_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/moves-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/moves-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = detect_coordinated_moves_core(c["nets"].as_array().unwrap());
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
