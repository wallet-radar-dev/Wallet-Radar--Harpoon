// Pure kernel of `GET /api/whale-positions` (server/whale-positions.js
// whalePositions): aggregate, across the analysed-asset dataset files
// (asset-forensics.json + wallet-asset-scan.json), a map wallet → every asset in
// which it is a coordinated-group member, keeping the max share per asset. Only
// non-public whales (not CEX, no known entity) in >= 2 assets. Deterministic over
// the files (the file reads are the route's job). Golden-tested byte-for-byte.

use crate::jsmath::{js_num, round_fixed};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::sync::OnceLock;

fn addr_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"^0x[0-9a-f]{40}$").unwrap())
}

/// `files` are the `.assets` arrays of each dataset file, in read order
/// (forensics first, then scan) — matching the JS iteration order.
pub fn whale_positions(files: &[&[Value]]) -> Value {
    // addr -> (symbol -> max share), both insertion-ordered like the JS Maps
    let mut agg: IndexMap<String, IndexMap<String, f64>> = IndexMap::new();
    for assets in files {
        for a in *assets {
            let symbol = a["symbol"].as_str().unwrap_or("");
            let empty = vec![];
            for g in a["groups"].as_array().unwrap_or(&empty) {
                for m in g["members"].as_array().unwrap_or(&empty) {
                    let ad = m["addr"].as_str().unwrap_or("").to_lowercase();
                    if !addr_re().is_match(&ad) || crate::cex::is_cex(&ad) || crate::entities::entity_of(&ad).is_some() {
                        continue;
                    }
                    let share = m["share"].as_f64().unwrap_or(0.0);
                    let e = agg.entry(ad).or_default();
                    let cur = e.get(symbol).copied().unwrap_or(0.0);
                    e.insert(symbol.to_string(), cur.max(share));
                }
            }
        }
    }

    struct Pos {
        addr: String,
        asset_count: usize,
        total_share: f64, // +sum(raw).toFixed(2)
        assets: Vec<(String, f64)>,
    }
    let mut positions: Vec<Pos> = agg.iter().map(|(addr, m)| {
        let total = round_fixed(m.values().sum(), 2);
        // each share rounded, then sorted by rounded share desc (stable)
        let mut assets: Vec<(String, f64)> = m.iter().map(|(s, v)| (s.clone(), round_fixed(*v, 2))).collect();
        assets.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
        Pos { addr: addr.clone(), asset_count: m.len(), total_share: total, assets }
    }).filter(|p| p.asset_count >= 2).collect();

    positions.sort_by(|a, b| b.asset_count.cmp(&a.asset_count).then(b.total_share.partial_cmp(&a.total_share).unwrap_or(Ordering::Equal)));
    positions.truncate(50);

    let out: Vec<Value> = positions.iter().map(|p| json!({
        "addr": p.addr,
        "assetCount": p.asset_count,
        "totalShare": js_num(p.total_share),
        "assets": p.assets.iter().map(|(s, v)| json!({"symbol": s, "share": js_num(*v)})).collect::<Vec<_>>(),
    })).collect();

    json!({ "count": out.len(), "positions": out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whale_positions_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("wpos-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("wpos-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let case = c.as_array().unwrap();
            let arrs: Vec<&[Value]> = case.iter().map(|a| a.as_array().map(|v| v.as_slice()).unwrap_or(&[])).collect();
            let got = whale_positions(&arrs);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
