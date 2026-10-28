// Pure assembly kernel of `GET /api/wallet-graph` (server/index.js:408-418):
// given a wallet and its counterparties (each with count / name / contract flag /
// chains, plus already-resolved entity and public-label attribution), build the
// node/edge graph — top-40 by interaction count, classified into cex / fund /
// defi / individual. Ports classifyCp (index.js:366), shortMid (:378) and the
// cex-class of KNOWN (:326). The live counterparty aggregation (transfersRich
// across chains / non-EVM explorers) is in the route (main.rs); entityOf/labelOf
// are resolved there and injected per counterparty so this kernel is hermetic.

use serde_json::{json, Value};
use std::cmp::Ordering::Equal;
use std::sync::OnceLock;

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// index.js:326 `KNOWN` — the ten hard-coded exchange/burn addresses (name, cls).
fn known(addr: &str) -> Option<(String, String)> {
    let s = wr_store::snapshot();
    let a = addr.to_lowercase();
    if !s.wallet_has_tag("eth", &a, "wallet_graph:known") {
        return None;
    }
    let kind = s.wallet_kind("eth", &a)?;
    Some((s.wallet_label("eth", &a).unwrap_or(kind).to_string(), kind.to_string()))
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static regex::Regex {
            static R: OnceLock<regex::Regex> = OnceLock::new();
            R.get_or_init(|| regex::Regex::new($pat).unwrap())
        }
    };
}
// classifyCp's four probes (index.js:366) — matched against the lowercased name.
re!(cex_re, r"binance|coinbase|okx|kraken|bybit|huobi|kucoin|gate|bitfinex|gemini|bitstamp|exchange|mexc|bitget");
re!(fund_re, r"wintermute|jump|cumberland|alameda|market maker|\bfund\b|galaxy");
re!(defi_re, r"router|uniswap|sushiswap|1inch|aggregator|paraswap|kyber|curve|balancer|protocol|swap|seaport|permit2");
re!(burn_re, r"burn|null");

/// index.js:366 `classifyCp(name, isContract, addr)`.
fn classify_cp(name: &str, is_contract: bool, addr: &str) -> String {
    let a = addr.to_lowercase();
    if let Some((_, cls)) = known(&a) {
        return if cls == "burn" { "unknown".to_string() } else { cls };
    }
    let s = name.to_lowercase();
    if cex_re().is_match(&s) {
        "cex"
    } else if fund_re().is_match(&s) {
        "fund"
    } else if defi_re().is_match(&s) {
        "defi"
    } else if burn_re().is_match(&s) {
        "unknown"
    } else if is_contract {
        "defi"
    } else {
        "individual"
    }
    .to_string()
}

/// index.js:378 `shortMid` — `0xabcd…wxyz` (addresses are ASCII).
fn short_mid(a: &str) -> String {
    if a.len() <= 10 {
        return a.to_string();
    }
    format!("{}…{}", &a[..6], &a[a.len() - 4..])
}

/// A non-empty `&str` field, else None (mirrors JS `x || …` falsiness on "").
fn nonempty<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|s| !s.is_empty())
}

/// The wallet-graph assembly kernel. `cp` is the full counterparty list in
/// insertion order; each entry: `{addr, count, name, contract, chains?, ent?,
/// label?}`. `active_chains` is the chains the wallet was active on (used for the
/// stats and as the per-node fallback when an entry has no `chains`).
pub fn wallet_graph(address: &str, cp: &[Value], total_tx: i64, active_chains: &Value, chain_type: &str) -> Value {
    // top 40 by count, descending, stable (V8 Array.sort is stable)
    let mut top: Vec<&Value> = cp.iter().collect();
    top.sort_by(|x, y| f(&y["count"]).partial_cmp(&f(&x["count"])).unwrap_or(Equal));
    top.truncate(40);

    let mut nodes: Vec<Value> = vec![json!({
        "id": address, "label": "wallet", "kind": "wallet", "etype": Value::Null, "count": top.len(),
    })];
    let mut edges: Vec<Value> = Vec::with_capacity(top.len());

    for e in &top {
        let addr = e["addr"].as_str().unwrap_or("");
        let ent = &e["ent"]; // entityOf(addr): {name, type} | null
        let label = &e["label"]; // labelOf(addr): {name} | null
        let ename = e["name"].as_str().unwrap_or("");
        let contract = e["contract"].as_bool().unwrap_or(false);
        let ent_name = nonempty(ent, "name");
        let pub_name = nonempty(label, "name");
        let kn = known(addr);

        // verified = !!(ent || KNOWN[addr] || e.name || pub?.name)
        let verified = !ent.is_null() || kn.is_some() || !ename.is_empty() || pub_name.is_some();

        // label = ent?.name || KNOWN[addr]?.name || e.name || pub?.name || (contract ? "contract" : shortMid)
        let node_label = if let Some(n) = ent_name {
            n.to_string()
        } else if let Some((kname, _)) = kn {
            kname.to_string()
        } else if !ename.is_empty() {
            ename.to_string()
        } else if let Some(pn) = pub_name {
            pn.to_string()
        } else if contract {
            "contract".to_string()
        } else {
            short_mid(addr)
        };

        // etype = ent?.type || classifyCp(e.name || pub?.name, contract, addr)
        let etype = if let Some(t) = nonempty(ent, "type") {
            t.to_string()
        } else {
            let cls_name = if !ename.is_empty() { ename } else { pub_name.unwrap_or("") };
            classify_cp(cls_name, contract, addr)
        };

        // chains = e.chains ? e.chains : activeChains  (a present array, even [], wins)
        let chains = match e.get("chains") {
            Some(c) if !c.is_null() => c.clone(),
            _ => active_chains.clone(),
        };

        nodes.push(json!({
            "id": addr, "label": node_label, "kind": "entity", "etype": etype,
            "verified": verified, "count": e["count"].clone(), "chains": chains,
        }));
        edges.push(json!({ "from": address, "to": addr, "count": e["count"].clone() }));
    }

    json!({
        "address": address,
        "nodes": nodes,
        "edges": edges,
        "stats": { "counterparties": cp.len(), "totalTx": total_tx, "chains": active_chains.clone(), "type": chain_type },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_graph_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("wgraph-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("wgraph-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let empty = vec![];
            let cp = c["cp"].as_array().unwrap_or(&empty);
            let got = wallet_graph(
                c["address"].as_str().unwrap(),
                cp,
                c["totalTx"].as_i64().unwrap(),
                &c["activeChains"],
                c["type"].as_str().unwrap(),
            );
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
