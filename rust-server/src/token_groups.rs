//! Port of the pure core of server index.js `/api/asset-groups` — coordinated
//! holder groups of a token, clustered by shared external sources (union-find,
//! with high-frequency "hub" addresses excluded).
//!
//! `token_groups` is pure and golden-tested; the tokenInfo/holders/counterparty
//! fetches stay in main.rs. `classify` is index.js's simple holder classifier
//! (KNOWN labels + share tiers).

use crate::jsmath::{js_num, round_fixed};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::cmp::Ordering::Equal;
use std::collections::{HashMap, HashSet};

/// index.js KNOWN — the ten hard-coded exchange/burn labels `(name, cls)`.
fn known(a: &str) -> Option<(String, String)> {
    let s = wr_store::snapshot();
    let a = a.to_lowercase();
    if !s.wallet_has_tag("eth", &a, "token_groups:known") {
        return None;
    }
    let kind = s.wallet_kind("eth", &a)?;
    Some((s.wallet_label("eth", &a).unwrap_or(kind).to_string(), kind.to_string()))
}

/// index.js `classify(addr, share, isContract)` → `(cls, label)`.
fn classify(addr: &str, share: f64, is_contract: bool) -> (&'static str, String) {
    if let Some((name, cls)) = known(addr) {
        // Вид приходить із бази рядком, а сигнатура обіцяє `&'static str`.
        // Зіставляємо явно: невідомий вид не ковтаємо мовчки — хай піде
        // далі звичайною класифікацією, як адреса без мітки.
        let st = match cls.as_str() {
            "cex" => Some("cex"),
            "burn" => Some("burn"),
            "bridge" => Some("bridge"),
            "defi" | "issuer" | "pool" | "contract" => Some("contract"),
            _ => None,
        };
        if let Some(st) = st {
            return (st, name);
        }
    }
    if addr.len() == 42 && addr.starts_with("0x") && addr[2..].bytes().all(|b| b == b'0') {
        return ("burn", "Null".to_string()); // /^0x0{40}$/
    }
    if is_contract {
        ("contract", String::new())
    } else if share >= 5.0 {
        ("whale", String::new())
    } else if share >= 1.0 {
        ("large", String::new())
    } else {
        ("holder", String::new())
    }
}

fn find(parent: &mut HashMap<String, String>, mut x: String) -> String {
    while parent[&x] != x {
        let p = parent[&x].clone();
        let gp = parent[&p].clone();
        parent.insert(x.clone(), gp.clone());
        x = gp;
    }
    x
}

/// index.js `/api/asset-groups` reduction. `big` are the analysed top holders
/// (`{address, share}`); `cps` maps each holder → its counterparties. Clusters by
/// direct link or ≥2 shared non-hub, non-member sources; hubs are addresses seen
/// across `> max(2, floor(N*0.34))` holders.
pub fn token_groups(contract: &str, symbol: &str, big: &[Value], cps: &Value) -> Value {
    let ids: Vec<String> = big.iter().map(|h| h["address"].as_str().unwrap_or("").to_string()).collect();
    let n = big.len();
    let big_set: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    let sets: HashMap<&str, HashSet<&str>> = ids
        .iter()
        .map(|id| {
            let s: HashSet<&str> = cps[id.as_str()].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            (id.as_str(), s)
        })
        .collect();

    // hubs: addresses appearing across many holders' counterparties.
    let mut hub: HashMap<&str, i64> = HashMap::new();
    for id in &ids {
        if let Some(s) = sets.get(id.as_str()) {
            for x in s {
                *hub.entry(x).or_insert(0) += 1;
            }
        }
    }
    let hub_limit = 2.0f64.max((n as f64 * 0.34).floor()) as i64;
    let is_hub = |x: &str| hub.get(x).copied().unwrap_or(0) > hub_limit;

    let mut parent: HashMap<String, String> = ids.iter().map(|a| (a.clone(), a.clone())).collect();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            let (a, b) = (&ids[i], &ids[j]);
            let sa = sets.get(a.as_str());
            let sb = sets.get(b.as_str());
            let direct = sa.is_some_and(|s| s.contains(b.as_str())) || sb.is_some_and(|s| s.contains(a.as_str()));
            let shared = match (sa, sb) {
                (Some(sa), Some(sb)) => sa.iter().filter(|&&x| !big_set.contains(x) && !is_hub(x) && sb.contains(x)).count(),
                _ => 0,
            };
            if direct || shared >= 2 {
                let (ra, rb) = (find(&mut parent, a.clone()), find(&mut parent, b.clone()));
                parent.insert(ra, rb);
            }
        }
    }

    let mut gmap: IndexMap<String, Vec<Value>> = IndexMap::new();
    for h in big {
        let r = find(&mut parent, h["address"].as_str().unwrap_or("").to_string());
        gmap.entry(r).or_default().push(h.clone());
    }
    let mut groups: Vec<Value> = gmap
        .values()
        .map(|g| {
            let mut gsorted = g.clone();
            gsorted.sort_by(|a, b| b["share"].as_f64().unwrap_or(0.0).partial_cmp(&a["share"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));
            let cum: f64 = g.iter().map(|h| h["share"].as_f64().unwrap_or(0.0)).sum();
            let members: Vec<Value> = gsorted
                .iter()
                .map(|h| {
                    let addr = h["address"].as_str().unwrap_or("");
                    let (cls, label) = classify(addr, h["share"].as_f64().unwrap_or(0.0), false);
                    json!({"address": addr, "share": h["share"].clone(), "label": label, "cls": cls})
                })
                .collect();
            json!({"size": js_num(g.len() as f64), "cumShare": js_num(round_fixed(cum, 2)), "members": members})
        })
        .collect();
    groups.sort_by(|a, b| b["cumShare"].as_f64().unwrap_or(0.0).partial_cmp(&a["cumShare"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));

    let largest = groups.first().and_then(|g| g["cumShare"].as_f64()).unwrap_or(0.0);
    let total_big: f64 = big.iter().map(|h| h["share"].as_f64().unwrap_or(0.0)).sum();
    json!({
        "contract": contract, "symbol": symbol, "analyzed": js_num(n as f64),
        "totalBigShare": js_num(round_fixed(total_big, 2)), "groups": groups,
        "flags": {"reach40": largest >= 40.0, "reach60": largest >= 60.0, "reach70": largest >= 70.0},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_groups_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/tokengroups-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/tokengroups-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = token_groups(c["contract"].as_str().unwrap(), c["symbol"].as_str().unwrap(), c["big"].as_array().unwrap(), &c["cps"]);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
