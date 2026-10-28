//! Port of the pure cores of server/conductor.js + server/coordination.js —
//! the coordination-forensics kernels that are the product's core edge:
//!
//!   - `conductor_network`: the funding network a coordinator pays out to
//!     (outgoing native transfers → sub-wallets, excluding exchanges).
//!   - `coordination_score`: an evidence-based 0-100 "coordination confidence"
//!     over that network (reach / capital / synchronicity / consistency).
//!   - `wallet_cex_netflow`: a wallet's direct net flow to/from exchanges.
//!
//! The network fetches, persisted monitor state, and Telegram alerting stay in
//! the JS; these take the fetched transfers (and price) and reproduce the
//! reductions and scoring exactly, matching Node byte-for-byte.

use crate::cex::is_cex;
use crate::jsmath::{js_num, js_round, round_fixed};
use indexmap::IndexMap;
use serde_json::{Map, Value};

/// conductor.js `conductorNetwork`, minus the fetch. `per` is one transfer array
/// per chain (native transfers with `from`/`to`/`amount`/`ts`); `px` the ETH
/// price. Only outgoing transfers to a distinct, non-exchange recipient count.
pub fn conductor_network(address: &str, per: &[Value], px: f64) -> Value {
    let address = address.to_lowercase();

    // recip: to-address → (eth summed, tx count, latest ts). First-seen order.
    let mut recip: IndexMap<String, (f64, u64, f64)> = IndexMap::new();
    for tr in per {
        let Some(arr) = tr.as_array() else { continue };
        for t in arr {
            let from = t["from"].as_str();
            let to = t["to"].as_str();
            // from === address && to truthy && to !== address && !CEX.has(to)
            let to = match to {
                Some(s) if !s.is_empty() && s != address && from == Some(address.as_str()) && !is_cex(s) => s,
                _ => continue,
            };
            let amount = t["amount"].as_f64().unwrap_or(0.0);
            let ts = t["ts"].as_f64().unwrap_or(0.0);
            let e = recip.entry(to.to_string()).or_insert((0.0, 0, 0.0));
            e.0 += amount;
            e.1 += 1;
            if ts > e.2 {
                e.2 = ts;
            }
        }
    }

    // Map → filter usd>100 → sort by usd desc (stable) → top 40.
    let mut network: Vec<(String, f64, i64, u64, f64)> = recip
        .into_iter()
        .map(|(addr, (eth, tx, last))| (addr, round_fixed(eth, 2), js_round(eth * px) as i64, tx, last))
        .filter(|x| x.2 > 100)
        .collect();
    network.sort_by(|a, b| b.2.cmp(&a.2));
    network.truncate(40);

    let total: i64 = network.iter().map(|x| x.2).sum();
    let net_arr: Vec<Value> = network
        .iter()
        .map(|(addr, eth, usd, tx, last)| {
            json_obj(&[
                ("addr", Value::String(addr.clone())),
                ("eth", js_num(*eth)),
                ("usd", js_num(*usd as f64)),
                ("tx", js_num(*tx as f64)),
                ("last", js_num(*last)),
            ])
        })
        .collect();

    json_obj(&[
        ("address", Value::String(address)),
        ("funded", js_num(net_arr.len() as f64)),
        ("totalFundedUsd", js_num(total as f64)),
        ("network", Value::Array(net_arr)),
    ])
}

/// coordination.js `coordinationScore`, minus the internal `conductorNetwork`
/// call — `net` is that call's result. Produces the confidence, label, and the
/// evidence breakdown.
pub fn coordination_score(address: &str, net: &Value) -> Value {
    let address = address.to_lowercase();
    let n = or0(net["funded"].as_f64());
    let total_funded = or0(net["totalFundedUsd"].as_f64());

    if n == 0.0 {
        let ev = evidence(0.0, total_funded, 0, 0, 0, 0);
        return json_obj(&[
            ("address", Value::String(address)),
            ("confidence", js_num(0.0)),
            ("label", Value::String("Не координатор".into())),
            ("labelKey", Value::String("coord_none".into())),
            ("evidence", ev),
            ("network", Value::Array(vec![])),
        ]);
    }

    let empty = vec![];
    let network = net["network"].as_array().unwrap_or(&empty);

    // 1) reach — log-scaled count, ~40 wallets saturates.
    let reach = (((n + 1.0).log10()) / 41f64.log10()).min(1.0);
    // 2) capital — total funded, $1M saturates.
    let capital = (total_funded / 1e6).min(1.0);
    // 3) synchronicity — funding clustered within ±14d of the median time.
    let mut times: Vec<f64> = network.iter().filter_map(|w| truthy(w["last"].as_f64())).collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut sync = 0.0;
    if times.len() >= 3 {
        let med = times[times.len() / 2];
        let win = 14.0 * 24.0 * 3600.0 * 1000.0;
        sync = times.iter().filter(|&&t| (t - med).abs() <= win).count() as f64 / times.len() as f64;
    }
    // 4) consistency — steady per-wallet payout count.
    let txs: Vec<f64> = network.iter().map(|w| or0(w["tx"].as_f64())).collect();
    let avg_tx = txs.iter().sum::<f64>() / if txs.is_empty() { 1.0 } else { txs.len() as f64 };
    let consistency = if avg_tx > 0.0 {
        (txs.iter().filter(|&&x| x >= 1.0).count() as f64 / txs.len() as f64).min(1.0)
    } else {
        0.0
    };

    let ev = evidence(
        n,
        total_funded,
        js_round(reach * 100.0) as i64,
        js_round(capital * 100.0) as i64,
        js_round(sync * 100.0) as i64,
        js_round(consistency * 100.0) as i64,
    );

    let confidence = js_round(100.0 * (0.30 * reach + 0.20 * capital + 0.35 * sync + 0.15 * consistency)) as i64;
    let (label, label_key) = if confidence >= 70 {
        ("Координатор (висока впевненість)", "coord_high")
    } else if confidence >= 45 {
        ("Ймовірний координатор", "coord_likely")
    } else if confidence >= 20 {
        ("Слабкий сигнал координації", "coord_weak")
    } else {
        ("Не координатор", "coord_none")
    };

    let net_out: Vec<Value> = network
        .iter()
        .take(10)
        .map(|w| {
            json_obj(&[
                ("addr", w["addr"].clone()),
                ("usd", w["usd"].clone()),
                ("tx", w["tx"].clone()),
            ])
        })
        .collect();

    json_obj(&[
        ("address", Value::String(address)),
        ("confidence", js_num(confidence as f64)),
        ("label", Value::String(label.into())),
        ("labelKey", Value::String(label_key.into())),
        ("evidence", ev),
        ("network", Value::Array(net_out)),
    ])
}

/// conductor.js `walletCexNetflow`, minus the fetch. `nat`/`tok` are the native
/// and token transfers; net per asset = withdrawals (from CEX) − deposits (to CEX).
pub fn wallet_cex_netflow(address: &str, nat: &[Value], tok: &[Value], px: f64) -> Value {
    let address = address.to_lowercase();
    let mut net: IndexMap<String, f64> = IndexMap::new();
    let mut add = |asset: &str, usd: f64| *net.entry(asset.to_string()).or_insert(0.0) += usd;

    for t in nat {
        let from = t["from"].as_str();
        let to = t["to"].as_str();
        let amount = t["amount"].as_f64().unwrap_or(0.0);
        if from == Some(address.as_str()) && to.is_some_and(|s| !s.is_empty() && is_cex(s)) {
            add("ETH", -(amount * px));
        } else if to == Some(address.as_str()) && from.is_some_and(|s| !s.is_empty() && is_cex(s)) {
            add("ETH", amount * px);
        }
    }
    for t in tok {
        let from = t["from"].as_str();
        let to = t["to"].as_str();
        let asset = t["asset"].as_str().unwrap_or("");
        let usd = t["usd"].as_f64().unwrap_or(0.0);
        if from == Some(address.as_str()) && to.is_some_and(|s| !s.is_empty() && is_cex(s)) {
            add(asset, -usd);
        } else if to == Some(address.as_str()) && from.is_some_and(|s| !s.is_empty() && is_cex(s)) {
            add(asset, usd);
        }
    }

    let mut netflows: Vec<(String, i64)> =
        net.into_iter().map(|(asset, usd)| (asset, js_round(usd) as i64)).filter(|x| x.1.abs() >= 1000).collect();
    netflows.sort_by(|a, b| b.1.abs().cmp(&a.1.abs()));
    netflows.truncate(8);

    let arr: Vec<Value> =
        netflows.iter().map(|(asset, n)| json_obj(&[("asset", Value::String(asset.clone())), ("net", js_num(*n as f64))])).collect();
    let top = arr.first().cloned().unwrap_or(Value::Null);

    json_obj(&[("address", Value::String(address)), ("netflows", Value::Array(arr)), ("top", top)])
}

// ── money-trace (traceMoneyFlow) deterministic kernels ──
// The BFS driver (level fetch, 45s deadline, Promise.race timeout) stays in
// main.rs; these two pure pieces reproduce its per-node reduction and final
// assembly, golden-tested byte-for-byte against Node.

/// conductor.js `entityOf(addr)?.name || "Біржа"` — the exchange display name for
/// a CEX address, or the generic fallback when it isn't in the entities table.
fn exchange_name(addr: &str) -> String {
    crate::entities::entity_of(addr).map(|e| e.name.to_string()).unwrap_or_else(|| "Біржа".to_string())
}

/// One BFS node of conductor.js `traceMoneyFlow` (lines 87-120): given `addr`'s
/// native + token transfers, the ETH price, its `depth`, and the `visited` set as
/// of before this node, produce this node's `edges`, CEX `cexExits` /
/// `cexWithdrawals`, and the `newNodes` to enqueue (breadth-capped at 6). Every JS
/// quirk is kept: edges are pushed for every non-CEX outflow ≥ $200 *before* the
/// breadth cap stops enqueueing; withdrawals carry the current `depth` while exits
/// and edges carry `depth + 1`; token deposits/withdrawals aggregate by
/// `to|asset` / `from|asset` in first-seen order.
pub fn money_trace_node(addr: &str, native: &[Value], tok: &[Value], px: f64, depth: i64, visited: &std::collections::HashSet<String>) -> Value {
    const MIN_USD: f64 = 200.0;
    const BREADTH: usize = 6;
    let a = addr;

    // out: to → summed ETH (from === addr, to truthy & not self), first-seen order.
    let mut out: IndexMap<String, f64> = IndexMap::new();
    for t in native {
        if t["from"].as_str() == Some(a) {
            if let Some(to) = t["to"].as_str() {
                if !to.is_empty() && to != a {
                    *out.entry(to.to_string()).or_insert(0.0) += t["amount"].as_f64().unwrap_or(0.0);
                }
            }
        }
    }
    // sort by ETH desc (stable — ties keep first-seen order, matching V8).
    let mut sorted: Vec<(String, f64)> = out.into_iter().collect();
    sorted.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut edges: Vec<Value> = Vec::new();
    let mut cex_exits: Vec<Value> = Vec::new();
    let mut cex_withdrawals: Vec<Value> = Vec::new();
    let mut new_nodes: Vec<String> = Vec::new();
    let mut added = 0usize;
    for (to, eth) in &sorted {
        let usd = js_round(eth * px);
        if usd < MIN_USD {
            break;
        }
        if is_cex(to) {
            cex_exits.push(json_obj(&[
                ("from", Value::String(a.to_string())),
                ("to", Value::String(to.clone())),
                ("exchange", Value::String(exchange_name(to))),
                ("asset", Value::String("ETH".into())),
                ("amount", js_num(round_fixed(*eth, 3))),
                ("usd", js_num(usd)),
                ("depth", js_num((depth + 1) as f64)),
            ]));
            continue;
        }
        edges.push(json_obj(&[
            ("from", Value::String(a.to_string())),
            ("to", Value::String(to.clone())),
            ("eth", js_num(round_fixed(*eth, 3))),
            ("usd", js_num(usd)),
            ("depth", js_num((depth + 1) as f64)),
        ]));
        if !visited.contains(to) {
            new_nodes.push(to.clone());
            added += 1;
            if added >= BREADTH {
                break;
            }
        }
    }

    // Token deposits to a CEX, aggregated by (to, asset) in first-seen order.
    let mut tagg: IndexMap<String, (String, String, f64, f64)> = IndexMap::new();
    for t in tok {
        if t["from"].as_str() == Some(a) {
            if let Some(to) = t["to"].as_str() {
                if !to.is_empty() && is_cex(to) {
                    let asset = t["asset"].as_str().unwrap_or("");
                    let e = tagg.entry(format!("{to}|{asset}")).or_insert((to.to_string(), asset.to_string(), 0.0, 0.0));
                    e.2 += t["amount"].as_f64().unwrap_or(0.0);
                    e.3 += t["usd"].as_f64().unwrap_or(0.0);
                }
            }
        }
    }
    for (_k, (to, asset, amount, usd)) in &tagg {
        cex_exits.push(json_obj(&[
            ("from", Value::String(a.to_string())),
            ("to", Value::String(to.clone())),
            ("exchange", Value::String(exchange_name(to))),
            ("asset", Value::String(asset.clone())),
            ("amount", js_num(round_fixed(*amount, 2))),
            ("usd", js_num(js_round(*usd))),
            ("depth", js_num((depth + 1) as f64)),
        ]));
    }

    // ETH withdrawals from a CEX (to === addr, from is CEX), first-seen order.
    let mut w_eth: IndexMap<String, f64> = IndexMap::new();
    for t in native {
        if t["to"].as_str() == Some(a) {
            if let Some(from) = t["from"].as_str() {
                if !from.is_empty() && is_cex(from) {
                    *w_eth.entry(from.to_string()).or_insert(0.0) += t["amount"].as_f64().unwrap_or(0.0);
                }
            }
        }
    }
    for (cex, eth) in &w_eth {
        let usd = js_round(eth * px);
        if usd >= MIN_USD {
            cex_withdrawals.push(json_obj(&[
                ("from", Value::String(cex.clone())),
                ("to", Value::String(a.to_string())),
                ("exchange", Value::String(exchange_name(cex))),
                ("asset", Value::String("ETH".into())),
                ("amount", js_num(round_fixed(*eth, 3))),
                ("usd", js_num(usd)),
                ("depth", js_num(depth as f64)),
            ]));
        }
    }

    // Token withdrawals from a CEX, aggregated by (from, asset), first-seen order.
    let mut w_tok: IndexMap<String, (String, String, f64, f64)> = IndexMap::new();
    for t in tok {
        if t["to"].as_str() == Some(a) {
            if let Some(from) = t["from"].as_str() {
                if !from.is_empty() && is_cex(from) {
                    let asset = t["asset"].as_str().unwrap_or("");
                    let e = w_tok.entry(format!("{from}|{asset}")).or_insert((from.to_string(), asset.to_string(), 0.0, 0.0));
                    e.2 += t["amount"].as_f64().unwrap_or(0.0);
                    e.3 += t["usd"].as_f64().unwrap_or(0.0);
                }
            }
        }
    }
    for (_k, (from, asset, amount, usd)) in &w_tok {
        cex_withdrawals.push(json_obj(&[
            ("from", Value::String(from.clone())),
            ("to", Value::String(a.to_string())),
            ("exchange", Value::String(exchange_name(from))),
            ("asset", Value::String(asset.clone())),
            ("amount", js_num(round_fixed(*amount, 2))),
            ("usd", js_num(js_round(*usd))),
            ("depth", js_num(depth as f64)),
        ]));
    }

    json_obj(&[
        ("edges", Value::Array(edges)),
        ("cexExits", Value::Array(cex_exits)),
        ("cexWithdrawals", Value::Array(cex_withdrawals)),
        ("newNodes", Value::Array(new_nodes.into_iter().map(Value::String).collect())),
    ])
}

/// conductor.js `traceMoneyFlow` final assembly (lines 123-135): terminals (visited
/// nodes with no outgoing edge), the top-20 CEX exits/withdrawals by USD with their
/// pre-slice totals, the deepest edges, and the capped edge list.
#[allow(clippy::too_many_arguments)]
pub fn money_trace_assemble(
    start: &str,
    max_depth: i64,
    depth_reached: i64,
    explored: i64,
    timed_out: bool,
    edges: &[Value],
    cex_exits: &[Value],
    cex_withdrawals: &[Value],
    visited: &[String],
    node_depth: &std::collections::HashMap<String, i64>,
) -> Value {
    use std::cmp::Ordering::Equal;
    let usd_desc = |x: &Value, y: &Value| y["usd"].as_f64().unwrap_or(0.0).partial_cmp(&x["usd"].as_f64().unwrap_or(0.0)).unwrap_or(Equal);

    let has_out: std::collections::HashSet<&str> = edges.iter().filter_map(|e| e["from"].as_str()).collect();
    let mut terminals: Vec<Value> = visited
        .iter()
        .filter(|a| a.as_str() != start && !has_out.contains(a.as_str()))
        .map(|a| json_obj(&[("addr", Value::String(a.clone())), ("depth", js_num(*node_depth.get(a).unwrap_or(&0) as f64))]))
        .collect();
    terminals.sort_by(|x, y| y["depth"].as_f64().unwrap_or(0.0).partial_cmp(&x["depth"].as_f64().unwrap_or(0.0)).unwrap_or(Equal));
    terminals.truncate(15);

    let exit_total: f64 = cex_exits.iter().map(|x| x["usd"].as_f64().unwrap_or(0.0)).sum();
    let mut exits_sorted = cex_exits.to_vec();
    exits_sorted.sort_by(&usd_desc);
    exits_sorted.truncate(20);

    let wd_total: f64 = cex_withdrawals.iter().map(|x| x["usd"].as_f64().unwrap_or(0.0)).sum();
    let mut wd_sorted = cex_withdrawals.to_vec();
    wd_sorted.sort_by(&usd_desc);
    wd_sorted.truncate(20);

    let deepest: Vec<Value> =
        edges.iter().filter(|e| e["depth"].as_f64() == Some(depth_reached as f64)).take(10).cloned().collect();
    let edges_list: Vec<Value> = edges.iter().take(300).cloned().collect();

    json_obj(&[
        ("start", Value::String(start.to_string())),
        ("maxDepth", js_num(max_depth as f64)),
        ("depthReached", js_num(depth_reached as f64)),
        ("nodesExplored", js_num(explored as f64)),
        ("totalNodes", js_num(visited.len() as f64)),
        ("edges", js_num(edges.len() as f64)),
        ("timedOut", Value::Bool(timed_out)),
        ("cexExits", Value::Array(exits_sorted)),
        ("cexExitTotalUsd", js_num(exit_total)),
        ("cexWithdrawals", Value::Array(wd_sorted)),
        ("cexWithdrawalTotalUsd", js_num(wd_total)),
        ("deepest", Value::Array(deepest)),
        ("terminals", Value::Array(terminals)),
        ("edgesList", Value::Array(edges_list)),
    ])
}

/// `x || 0` for an optional number: falsy (`None`/`0`/`NaN`) → 0.
fn or0(v: Option<f64>) -> f64 {
    match v {
        Some(n) if n != 0.0 && !n.is_nan() => n,
        _ => 0.0,
    }
}

/// `filter(Boolean)` for a numeric field: keep only truthy (nonzero, non-NaN).
fn truthy(v: Option<f64>) -> Option<f64> {
    v.filter(|&n| n != 0.0 && !n.is_nan())
}

fn evidence(funded: f64, total_funded: f64, reach: i64, capital: i64, sync: i64, consistency: i64) -> Value {
    json_obj(&[
        ("funded", js_num(funded)),
        ("totalFundedUsd", js_num(total_funded)),
        ("reach", js_num(reach as f64)),
        ("capital", js_num(capital as f64)),
        ("synchronicity", js_num(sync as f64)),
        ("consistency", js_num(consistency as f64)),
    ])
}

fn json_obj(fields: &[(&str, Value)]) -> Value {
    let mut o = Map::new();
    for (k, v) in fields {
        o.insert((*k).to_string(), v.clone());
    }
    Value::Object(o)
}

// ── Conductor monitoring (conductor.js:138-173) ─────────────────────────────────
//
// Wallets labelled "диригент"/"conductor" in the watchlist are polled for NEW outgoing
// funding. The state file is what keeps this from being a spam cannon: `seen` dedupes
// transfers across restarts, and `init` marks an address as already-scanned so its
// entire history does not fire on first sight.

/// conductor.js:139-142 — the persisted monitor state.
#[derive(Default)]
pub struct MonitorState {
    /// `chain:from:to:ts` → seen. Capped at 5000 on persist (oldest dropped).
    pub seen: indexmap::IndexSet<String>,
    /// address → its first scan is done, so alerts may fire from now on.
    pub init: indexmap::IndexSet<String>,
}

const SEEN_CAP: usize = 5000;
/// conductor.js:161 — below this a funding transfer is noise, not a position.
const MIN_ALERT_USD: f64 = 1000.0;
/// conductor.js:16
pub const CHAINS: [&str; 5] = ["eth", "base", "arbitrum", "optimism", "polygon"];

impl MonitorState {
    pub fn load(file: &std::path::Path) -> Self {
        let keys = |v: &Value, k: &str| {
            v[k].as_object().map(|o| o.keys().cloned().collect::<indexmap::IndexSet<String>>()).unwrap_or_default()
        };
        std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|j| Self { seen: keys(&j, "seen"), init: keys(&j, "init") })
            .unwrap_or_default()
    }

    /// The JS writes `{seen: {key: 1}, init: {addr: 1}}` — keep the shape so either
    /// server can read the other's state file.
    pub fn persist(&mut self, file: &std::path::Path) {
        if self.seen.len() > SEEN_CAP {
            let drop = self.seen.len() - SEEN_CAP;
            self.seen = self.seen.iter().skip(drop).cloned().collect();
        }
        let one = |s: &indexmap::IndexSet<String>| -> Value {
            Value::Object(s.iter().map(|k| (k.clone(), Value::from(1))).collect())
        };
        if let Ok(text) = serde_json::to_string(&serde_json::json!({"seen": one(&self.seen), "init": one(&self.init)})) {
            let _ = crate::atomicw::write(file, text);
        }
    }
}

/// conductor.js:144 — a watchlist wallet is a conductor if its label says so.
pub fn is_conductor_label(label: &str) -> bool {
    let l = label.to_lowercase();
    l.contains("дириген") || l.contains("conductor")
}

/// conductor.js:155-164 — the new fundings on one chain: outgoing, to a distinct
/// non-exchange address, not seen before. Every new transfer is RECORDED as seen, but
/// only ones over the threshold (and after the first scan) are returned as alertable —
/// that asymmetry is the point, otherwise the first poll after a restart would replay
/// old history the moment a transfer crossed the threshold.
pub fn new_fundings(
    chain: &str,
    addr: &str,
    transfers: &[Value],
    px: f64,
    first_scan: bool,
    seen: &mut indexmap::IndexSet<String>,
) -> Vec<Value> {
    let mut out = Vec::new();
    for t in transfers {
        let (from, to) = (t["from"].as_str().unwrap_or(""), t["to"].as_str().unwrap_or(""));
        if from != addr || to.is_empty() || to == addr || is_cex(to) {
            continue;
        }
        let key = format!("{chain}:{from}:{to}:{}", t["ts"].as_f64().map(js_num).unwrap_or(Value::from(0)));
        if !seen.insert(key) {
            continue; // already alerted on (or recorded), possibly in an earlier process
        }
        let usd = js_round(t["amount"].as_f64().unwrap_or(0.0) * px);
        if !first_scan && usd > MIN_ALERT_USD {
            out.push(json_obj(&[("chain", Value::from(chain)), ("to", Value::from(to)), ("usd", js_num(usd))]));
        }
    }
    out
}

/// conductor.js:162 — the funding alert. `label` is the conductor's watchlist label.
pub fn funding_alert(label: &str, ev: &Value, now: i64) -> Value {
    let to = ev["to"].as_str().unwrap_or("");
    let short: String = to.chars().take(10).collect();
    let usd = ev["usd"].as_f64().unwrap_or(0.0);
    let chain = ev["chain"].as_str().unwrap_or("");
    serde_json::json!({
        "type": "group-alert",
        "category": "conductor-funding",
        "icon": "🎼",
        "title": format!("Диригент {label} профінансував {short}…"),
        "body": format!("${} ({chain}) — можлива нова координована позиція", thousands(usd)),
        "address": to,
        "valueUsd": js_num(usd),
        "ts": now,
    })
}

/// conductor.js:162 — the Telegram body for the same event.
pub fn funding_telegram(label: &str, ev: &Value) -> String {
    let to = ev["to"].as_str().unwrap_or("");
    let short: String = to.chars().take(10).collect();
    format!(
        "🎼 <b>Диригент</b> {label} профінансував <code>{short}…</code> на <b>${}</b> ({}) — можлива нова координована позиція.",
        thousands(ev["usd"].as_f64().unwrap_or(0.0)),
        ev["chain"].as_str().unwrap_or("")
    )
}

/// `Number.prototype.toLocaleString()` for the integers these alerts carry: groups of
/// three, comma-separated (the JS runs under an en-US default locale here).
fn thousands(v: f64) -> String {
    let n = js_round(v) as i64;
    let (sign, digits) = if n < 0 { ("-", (-n).to_string()) } else { ("", n.to_string()) };
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conductor_network_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("cond-network-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("cond-network-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = conductor_network(c["address"].as_str().unwrap(), c["per"].as_array().unwrap(), c["px"].as_f64().unwrap());
            assert_eq!(got, golden[i], "network case {i}");
        }
    }

    #[test]
    fn coordination_score_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("cond-score-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("cond-score-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = coordination_score(c["address"].as_str().unwrap(), &c["net"]);
            assert_eq!(got, golden[i], "score case {i}");
        }
    }

    #[test]
    fn money_trace_node_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("moneytrace-node-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("moneytrace-node-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let visited: std::collections::HashSet<String> =
                c["visited"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            let got = money_trace_node(
                c["addr"].as_str().unwrap(),
                c["native"].as_array().unwrap(),
                c["tok"].as_array().unwrap(),
                c["px"].as_f64().unwrap(),
                c["depth"].as_i64().unwrap(),
                &visited,
            );
            assert_eq!(got, golden[i], "node case {i}");
        }
    }

    #[test]
    fn money_trace_assemble_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("moneytrace-assemble-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("moneytrace-assemble-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let visited: Vec<String> = c["visited"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            let node_depth: std::collections::HashMap<String, i64> =
                c["nodeDepth"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap())).collect();
            let got = money_trace_assemble(
                c["start"].as_str().unwrap(),
                c["maxDepth"].as_i64().unwrap(),
                c["depthReached"].as_i64().unwrap(),
                c["explored"].as_i64().unwrap(),
                c["timedOut"].as_bool().unwrap(),
                c["edges"].as_array().unwrap(),
                c["cexExits"].as_array().unwrap(),
                c["cexWithdrawals"].as_array().unwrap(),
                &visited,
                &node_depth,
            );
            assert_eq!(got, golden[i], "assemble case {i}");
        }
    }

    #[test]
    fn wallet_cex_netflow_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("cond-netflow-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("cond-netflow-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = wallet_cex_netflow(
                c["address"].as_str().unwrap(),
                c["nat"].as_array().unwrap(),
                c["tok"].as_array().unwrap(),
                c["px"].as_f64().unwrap(),
            );
            assert_eq!(got, golden[i], "netflow case {i}");
        }
    }

    // ── monitor (conductor.js:138-173) ──

    fn tr(from: &str, to: &str, amount: f64, ts: i64) -> Value {
        serde_json::json!({"from": from, "to": to, "amount": amount, "ts": ts})
    }
    const C: &str = "0xf30ba13e00000000000000000000000000000001";
    const BINANCE: &str = "0x28c6c06298d514db089934071355e5743bf21d60";

    #[test]
    fn the_first_scan_is_silent_but_still_records_what_it_saw() {
        let mut seen = indexmap::IndexSet::new();
        let ts = [tr(C, "0xaaa", 10.0, 1), tr(C, "0xbbb", 20.0, 2)];

        // first_scan: no alerts, or a restart would replay the wallet's entire history.
        assert!(new_fundings("eth", C, &ts, 3000.0, true, &mut seen).is_empty());
        assert_eq!(seen.len(), 2, "…but both transfers are remembered");

        // Same transfers on the next poll: already seen → still silent.
        assert!(new_fundings("eth", C, &ts, 3000.0, false, &mut seen).is_empty());
    }

    #[test]
    fn only_new_outgoing_non_exchange_transfers_over_1k_alert() {
        let mut seen = indexmap::IndexSet::new();
        let ts = [
            tr(C, "0xaaa", 1.0, 1),        // $3000 → alerts
            tr(C, "0xbbb", 0.1, 2),        // $300 → below the threshold
            tr(C, BINANCE, 5.0, 3),        // exchange deposit → not a coordinated position
            tr(C, C, 9.0, 4),              // self-transfer
            tr("0xdead", C, 9.0, 5),       // incoming
        ];
        let got = new_fundings("eth", C, &ts, 3000.0, false, &mut seen);

        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["to"], serde_json::json!("0xaaa"));
        assert_eq!(got[0]["usd"], serde_json::json!(3000));

        let a = funding_alert("Кит-диригент", &got[0], 7);
        assert_eq!(a["category"], serde_json::json!("conductor-funding"));
        assert_eq!(a["title"], serde_json::json!("Диригент Кит-диригент профінансував 0xaaa…"));
        assert_eq!(a["body"], serde_json::json!("$3,000 (eth) — можлива нова координована позиція"));
    }

    #[test]
    fn conductor_labels_are_recognised_in_both_languages() {
        assert!(is_conductor_label("Диригент #1"));
        assert!(is_conductor_label("CONDUCTOR"));
        assert!(!is_conductor_label("Кит"));
        assert!(!is_conductor_label(""));
    }

    #[test]
    fn persist_caps_seen_at_5000_dropping_the_oldest() {
        let dir = std::env::temp_dir().join("cond-state-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("conductor-state.json");

        let mut st = MonitorState::default();
        for i in 0..5010 {
            st.seen.insert(format!("eth:a:b:{i}"));
        }
        st.init.insert("0xabc".to_string());
        st.persist(&file);

        let back = MonitorState::load(&file);
        assert_eq!(back.seen.len(), SEEN_CAP);
        assert!(!back.seen.contains("eth:a:b:9"), "the oldest keys are dropped");
        assert!(back.seen.contains("eth:a:b:5009"), "the newest are kept");
        assert!(back.init.contains("0xabc"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
