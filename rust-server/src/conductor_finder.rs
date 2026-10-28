//! Port of the pure core of server/conductor-finder.js — the cross-asset
//! conductor detector. Aggregates common funders from the forensics datasets and
//! finds coordinators who back holder groups in ≥2 assets from a single capital
//! source.
//!
//! The behavioural filter is the whole point. Public labels catch almost nothing
//! here (48 of 50 "conductors" found had no label at all), so the discriminator is
//! the product thesis itself: a CONDUCTOR leads the SAME team of wallets across
//! several assets, while an exchange or dispenser funds NEW addresses every time
//! and therefore has zero wallet overlap between assets. On real data 63 of 100
//! candidates had an empty intersection — those are dispensers, and they are dropped.
//!
//! The file reads (`asset-forensics.json`, `wallet-asset-scan.json`) and the
//! per-candidate on-chain verification (network-bound) stay in the caller. This
//! takes the combined `assets` list and reproduces `collect()` plus the candidate
//! map/filter/sort/slice, using the same infra guards (address shape, CEX, known
//! entity, infra label). Output matches Node.

use crate::cex::is_cex;
use crate::entities::entity_of;
use crate::jsmath::{js_num, js_round};
use crate::labels::Labels;
use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// conductor-finder.js:24 — a funder must reuse ≥2 wallets across its assets to be a
/// conductor rather than a dispenser.
const MIN_REUSED_HOLDERS: usize = 2;

/// Стеля фан-ауту: понад стільки активів — це вже не диригент, а роздавач.
///
/// Фільтр інфраструктури (нижче) б'є точно, але лише по тому, що є в базі міток. Топовий
/// «диригент» фінансував 185 активів і мітки не мав зовсім — а фандер такого розмаху це біржа,
/// бридж або деплоєр за самою побудовою, ким би він не був підписаний. Медіана — 4 активи, p90 —
/// 35, тож 40 відсікає хвіст, не чіпаючи нікого справжнього. Та сама стеля, що й для фандерських
/// зв'язків у whale_clusters: людина не диригує сорока активами.
const MAX_FANOUT_ASSETS: usize = 40;

fn addr_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^0x[0-9a-f]{40}$").unwrap())
}

struct Agg {
    /// Holders kept PER ASSET, not in one bag: it is the intersection between assets
    /// that separates a conductor (same team) from a dispenser (fresh addresses).
    by_asset: IndexMap<String, IndexSet<String>>,
    max_backer: f64,
    total_backer: f64,
    n: u64,
}

/// The candidate list plus the count of funders rejected as dispensers — the JS keeps
/// both, and `droppedAsDispensers` ships in conductors.json as a quality signal.
pub struct Candidates {
    pub list: Vec<Value>,
    pub dropped: i64,
}

/// conductor-finder.js `collect()` + the candidate builder in `buildConductors`, minus
/// the file reads and the verification pass. `assets` is the concatenation of each
/// source file's `assets` array (forensics first, then scan), preserving order. Each
/// asset has `symbol` and `groups[]`; each group has `commonFunders[]` (`{funder,
/// avgUsd}`) and `members[]` (`{addr}`).
pub fn build_conductor_candidates(assets: &[Value], labels: &Labels) -> Candidates {
    // agg: funder → aggregate, in first-seen order (JS Map iteration).
    let mut agg: IndexMap<String, Agg> = IndexMap::new();

    for a in assets {
        let symbol = a["symbol"].as_str().unwrap_or("");
        let Some(groups) = a["groups"].as_array() else { continue };
        for g in groups {
            let empty = vec![];
            for f in g["commonFunders"].as_array().unwrap_or(&empty) {
                let fa = f["funder"].as_str().unwrap_or("").to_lowercase();
                // A conductor is never infrastructure: reject bad shape, exchanges,
                // known entities, and public infra labels.
                if !addr_re().is_match(&fa) || is_cex(&fa) || entity_of(&fa).is_some() || labels.is_infra_addr(&fa) {
                    continue;
                }
                let avg_usd = or0(f["avgUsd"].as_f64());
                let e = agg.entry(fa).or_insert_with(|| Agg {
                    by_asset: IndexMap::new(),
                    max_backer: 0.0,
                    total_backer: 0.0,
                    n: 0,
                });
                let s = e.by_asset.entry(symbol.to_string()).or_default();
                if let Some(members) = g["members"].as_array() {
                    for m in members {
                        s.insert(m["addr"].as_str().unwrap_or("").to_string());
                    }
                }
                e.max_backer = e.max_backer.max(avg_usd);
                e.total_backer += avg_usd;
                e.n += 1;
            }
        }
    }

    // score → filter(assetCount ≥ 2) → filter(reusedHolders ≥ 2) → sort → top 60.
    let scored: Vec<(usize, usize, i64, Value)> = agg
        .into_iter()
        .map(|(funder, e)| {
            let asset_count = e.by_asset.len();
            // holder → in how many of this funder's assets it appears.
            let mut seen: IndexMap<String, usize> = IndexMap::new();
            for s in e.by_asset.values() {
                for h in s {
                    *seen.entry(h.clone()).or_insert(0) += 1;
                }
            }
            let reused: Vec<&String> = seen.iter().filter(|(_, n)| **n >= 2).map(|(h, _)| h).collect();
            let max_backer_usd = js_round(e.max_backer) as i64;
            let avg_backer_usd = js_round(e.total_backer / if e.n == 0 { 1.0 } else { e.n as f64 }) as i64;

            let obj = json_obj(&[
                ("funder", Value::String(funder)),
                ("assetCount", js_num(asset_count as f64)),
                ("assets", Value::Array(e.by_asset.keys().cloned().map(Value::String).collect())),
                ("holderCount", js_num(seen.len() as f64)),
                ("holders", Value::Array(seen.keys().take(12).cloned().map(Value::String).collect())),
                ("reusedHolders", js_num(reused.len() as f64)),
                // The evidence, not just a flag: which wallets actually recur.
                ("reusedSample", Value::Array(reused.iter().take(8).map(|h| Value::String((*h).clone())).collect())),
                ("maxBackerUsd", js_num(max_backer_usd as f64)),
                ("avgBackerUsd", js_num(avg_backer_usd as f64)),
            ]);
            (asset_count, reused.len(), max_backer_usd, obj)
        })
        .filter(|(asset_count, _, _, _)| *asset_count >= 2 && *asset_count <= MAX_FANOUT_ASSETS)
        .collect();

    let multi_asset = scored.len() as i64;
    let mut list: Vec<(usize, usize, i64, Value)> =
        scored.into_iter().filter(|(_, reused, _, _)| *reused >= MIN_REUSED_HOLDERS).collect();
    // (reusedHolders desc, assetCount desc, maxBackerUsd desc)
    list.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)).then(b.2.cmp(&a.2)));
    list.truncate(60);

    Candidates { dropped: multi_asset - list.len() as i64, list: list.into_iter().map(|(_, _, _, v)| v).collect() }
}

/// conductor-finder.js:99 — the honest flag. "Checked against labels and metadata,
/// and it is not infrastructure" — NOT "this is definitely a conductor". The field
/// used to read `verified: true` for everyone, because an absent label counted as a pass.
pub fn verification(c: &Value) -> Value {
    json!({
        "infraLabel": "none",
        "isContract": false,
        "evidence": format!(
            "{} гаманців повторюються у {} активах",
            c["reusedHolders"].as_i64().unwrap_or(0),
            c["assetCount"].as_i64().unwrap_or(0)
        ),
    })
}

fn or0(v: Option<f64>) -> f64 {
    match v {
        Some(n) if n != 0.0 && !n.is_nan() => n,
        _ => 0.0,
    }
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
    use std::path::Path;

    #[test]
    fn conductor_candidates_match_node() {
        let labels = Labels::load(Path::new("../dataset"));
        let inputs: Value = serde_json::from_str(&fixture!("cfinder-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("cfinder-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = build_conductor_candidates(c.as_array().unwrap(), &labels);
            assert_eq!(Value::Array(got.list), golden[i]["candidates"], "case {i} candidates");
            assert_eq!(got.dropped, golden[i]["dropped"].as_i64().unwrap(), "case {i} dropped");
        }
    }

    /// The dispenser filter is the whole point of the module, so it gets its own test:
    /// two funders back two assets each, but only one reuses the same wallets.
    #[test]
    fn a_funder_who_never_reuses_a_wallet_is_a_dispenser_not_a_conductor() {
        let labels = Labels::load(Path::new("/nonexistent")); // no labels → behaviour alone decides
        let conductor = "0x1111111111111111111111111111111111111111";
        let dispenser = "0x2222222222222222222222222222222222222222";
        let g = |funder: &str, members: [&str; 2]| {
            json!({"commonFunders": [{"funder": funder, "avgUsd": 1000}],
                   "members": [{"addr": members[0]}, {"addr": members[1]}]})
        };
        let assets = json!([
            {"symbol": "UNI", "groups": [g(conductor, ["0xaa", "0xbb"]), g(dispenser, ["0xc1", "0xc2"])]},
            {"symbol": "LINK", "groups": [g(conductor, ["0xaa", "0xbb"]), g(dispenser, ["0xd1", "0xd2"])]},
        ]);
        let got = build_conductor_candidates(assets.as_array().unwrap(), &labels);

        assert_eq!(got.list.len(), 1, "only the wallet-reusing funder survives");
        assert_eq!(got.list[0]["funder"], json!(conductor));
        assert_eq!(got.list[0]["reusedHolders"], json!(2));
        assert_eq!(got.list[0]["reusedSample"], json!(["0xaa", "0xbb"]));
        assert_eq!(got.dropped, 1, "the dispenser is counted, not silently vanished");
    }

    /// Зонд по ЖИВОМУ датасету (`cargo test live_probe -- --ignored --nocapture`): чи справді стеля
    /// фан-ауту тримає на тому, що зараз на диску. Файл conductors.json міг лишитися від старого
    /// бінарника, і дивитися треба на КОД, а не на його вчорашній слід.
    #[test]
    #[ignore]
    fn live_probe() {
        let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "..".into());
        let root = std::path::Path::new(&root).join("dataset");
        let labels = crate::labels::Labels::load(&root);
        let mut assets: Vec<Value> = Vec::new();
        for f in ["asset-forensics.json", "wallet-asset-scan.json"] {
            let v: Value = serde_json::from_slice(&std::fs::read(root.join(f)).unwrap()).unwrap();
            assets.extend(v["assets"].as_array().cloned().unwrap_or_default());
        }
        let got = build_conductor_candidates(&assets, &labels);
        let mut counts: Vec<i64> = got.list.iter().filter_map(|c| c["assetCount"].as_i64()).collect();
        counts.sort_unstable_by(|a, b| b.cmp(a));
        println!("кандидатів: {}  відкинуто як роздавачів: {}", got.list.len(), got.dropped);
        println!("топ assetCount: {:?}  (стеля {MAX_FANOUT_ASSETS})", &counts[..counts.len().min(6)]);
    }
}
