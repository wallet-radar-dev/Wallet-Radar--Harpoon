#![allow(dead_code, unused_imports)]
//! Shared app state (`App`, `ForensicsState`, `Ctx`) and cross-cutting helper
//! primitives used by both the HTTP routes and the background workers.
use crate::{dataset::Dataset, store::Store, labels::Labels, expand, jsmath, telegram, model};
use axum::{http::StatusCode, Json};
use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use std::path::PathBuf;

pub const APP_VERSION: &str = "1.0.0"; // server/index.js:75


/// server/index.js:661 — a second, narrower exchange filter applied to the
/// human-readable label, on top of the entity and infra checks.
pub const CEX_LABEL_RE: &str = r"(?i)binance|coinbase|okx|kraken|bybit|huobi|kucoin|gate|bitfinex|gemini|bitstamp|exchange|mexc|bitget|upbit|htx|crypto\.com|tether|circle";


pub struct App {
    pub dataset: Dataset,
    pub store: Store,
    pub labels: Labels,
    pub cex_label: Regex,
    pub started: Instant,
    // real-time backbone (index.js WebSocket + webhook)
    pub events_tx: tokio::sync::broadcast::Sender<String>,
    pub seen: std::sync::Mutex<HashMap<String, i64>>,
    pub webhook_secret: String,
    // behavior-monitor.js recent-alerts ring (newest-first, cap 50)
    pub alerts: std::sync::Mutex<Vec<Value>>,
    // index.js:201 idemCache — Idempotency-Key → (at, status, body), 24h TTL.
    pub idem: std::sync::Mutex<HashMap<String, (i64, u16, Value)>>,
    // conductor-finder.js:47 `pushed` — conductors already announced. Seeded from the
    // existing conductors.json so a restart does not re-announce the whole file.
    pub pushed_conductors: std::sync::Mutex<HashSet<String>>,
    // forensics-worker.js accumulated state (results, fingerprints, funders, alpha feed).
    pub forensics: Arc<std::sync::Mutex<ForensicsState>>,
    // expand-worker.js persistent crawl graph.
    pub graph: Arc<std::sync::Mutex<expand::Graph>>,
    // alerts.js `seq` — makes two alerts in the same millisecond distinct ids.
    pub alert_seq: std::sync::atomic::AtomicU64,
}


/// forensics-worker.js module state — it ACCUMULATES across cycles (that is the point:
/// the cross-asset funder map is what conductor detection is built on), so it is loaded
/// from asset-forensics.json at boot rather than starting empty.
#[derive(Default)]
pub struct ForensicsState {
    pub running: bool,
    pub cycle: u64,
    pub last_asset: String,
    pub processed: usize,
    pub total: usize,
    pub clusters: usize,
    pub updated_at: i64,
    /// symbol → the asset's analysis
    pub results: indexmap::IndexMap<String, Value>,
    /// addr → {tokens, totalUsd, top}
    pub fingerprints: indexmap::IndexMap<String, Value>,
    /// funder → the holders it backs, across every asset seen so far
    pub global_funders: indexmap::IndexMap<String, indexmap::IndexSet<String>>,
    pub alpha: Vec<Value>,
}


impl ForensicsState {
    pub fn load(dataset: &Dataset) -> Self {
        let mut st = Self::default();
        if let Some(j) = dataset.read("asset-forensics.json") {
            for a in j["assets"].as_array().unwrap_or(&vec![]) {
                if let Some(sym) = a["symbol"].as_str() {
                    st.results.insert(sym.to_string(), a.clone());
                }
            }
            if let Some(fp) = j["fingerprints"].as_object() {
                for (k, v) in fp {
                    st.fingerprints.insert(k.clone(), v.clone());
                }
            }
            if let Some(fs) = j["funders"].as_object() {
                for (f, arr) in fs {
                    let set = arr.as_array().map_or_else(indexmap::IndexSet::new, |a| {
                        a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
                    });
                    st.global_funders.insert(f.clone(), set);
                }
            }
        }
        st.alpha = dataset.read("alpha-feed.json").and_then(|v| v.as_array().cloned()).unwrap_or_default();
        st
    }
}


pub const IDEM_TTL_MS: i64 = 24 * 60 * 60 * 1000;


/// index.js:202 `idemGet` — a hit that has outlived the TTL is dropped, not served.
pub fn idem_get(s: &App, key: &str, now: i64) -> Option<(u16, Value)> {
    let mut m = s.idem.lock().unwrap();
    match m.get(key) {
        Some((at, _, _)) if now - at > IDEM_TTL_MS => {
            m.remove(key);
            None
        }
        Some((_, status, body)) => Some((*status, body.clone())),
        None => None,
    }
}


/// index.js:208 `idemSet` — store, and sweep expired entries once the map gets big.
pub fn idem_set(s: &App, key: String, status: u16, body: Value, now: i64) {
    let mut m = s.idem.lock().unwrap();
    m.insert(key, (now, status, body));
    if m.len() > 5000 {
        m.retain(|_, (at, _, _)| now - *at <= IDEM_TTL_MS);
    }
}


pub const WEBHOOK_DEDUP_TTL_MS: i64 = 6 * 60 * 60 * 1000;


/// index.js `broadcastMsg(obj)` — push any JSON object to every /ws client
/// (behavior alerts + capital-group events) alongside `{type:"event"}` frames.
pub fn broadcast_msg(s: &App, obj: &Value) {
    let _ = s.events_tx.send(obj.to_string());
}


/// alerts.js `groupAlert(opts)` — a capital-group signal goes to THREE places, and the
/// Rust workers were only doing one of them (the websocket). It must also land in the
/// persisted event feed (so a click on it opens the full picture) and in Telegram (so the
/// user hears about it while not staring at the tab).
///
/// `alert` is the `{type:"group-alert", category, icon, title, body, …}` object the
/// workers already build; `telegram` is the HTML message, or empty to skip Telegram.
pub fn group_alert(s: &Arc<App>, alert: &Value, telegram: &str) {
    if !telegram.is_empty() {
        let text = telegram.to_string();
        tokio::spawn(async move {
            telegram::send(&text).await;
        });
    }

    // The event carries the compatibility fields a plain transfer event has, so the old
    // renderers and filters do not trip over it (alerts.js:14-22).
    let now = now_ms();
    let seq = s.alert_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let category = alert["category"].as_str().unwrap_or("");
    let address = alert["address"].clone();
    let ev = json!({
        "id": format!("grp:{category}:{now}:{seq}"),
        "kind": "group",
        "category": category,
        "icon": alert["icon"].clone(),
        "title": alert["title"].clone(),
        "body": alert["body"].clone(),
        "address": address,
        "members": alert["members"].as_array().cloned().unwrap_or_default(),
        "assets": alert["assets"].as_array().cloned().unwrap_or_default(),
        "valueUsd": if alert["valueUsd"].is_null() { json!(0) } else { alert["valueUsd"].clone() },
        "meta": Value::Null,
        "timestamp": jsmath::js_num(now as f64),
        "network": "eth",
        "txHash": "",
        "blockNumber": 0,
        "feeUsd": 0,
        "status": "confirmed",
        "walletId": Value::Null,
        "asset": "",
        "amount": 0,
        "counterparty": alert["address"].as_str().unwrap_or(""),
        "counterpartyEntity": {"name": "", "type": "unknown"},
    });
    s.store.add_event(ev.clone());
    broadcast_event(s, &ev);
    broadcast_msg(s, alert);
}


/// index.js `broadcast(ev)` — push `{type:"event",event}` to every /ws client.
pub fn broadcast_event(s: &App, ev: &Value) {
    let msg = json!({"type": "event", "event": ev}).to_string();
    let _ = s.events_tx.send(msg); // Err only when no receivers — fine
}


/// index.js `markSeen` — true if this delivery key was seen within the 6h TTL.
pub fn mark_seen(s: &App, key: &str, now: i64) -> bool {
    let mut seen = s.seen.lock().unwrap();
    if let Some(&at) = seen.get(key) {
        if now - at <= WEBHOOK_DEDUP_TTL_MS {
            return true;
        }
    }
    seen.insert(key.to_string(), now);
    if seen.len() > 20000 {
        seen.retain(|_, ts| now - *ts <= WEBHOOK_DEDUP_TTL_MS);
    }
    false
}


/// JavaScript truthiness, as used by `if (m.isContract || ...)`.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}


pub type Ctx = Arc<App>;


/// conductor-finder.js INFRA_RE (line 16) — the metadata guard, separate from the
/// public-label list: a candidate whose on-chain name/tags smell like an exchange,
/// bridge or market maker is not a conductor.
pub fn meta_infra_re() -> &'static regex::Regex {
    static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(
            r"(?i)binance|coinbase|okx|kraken|bybit|huobi|kucoin|gate|bitfinex|gemini|bitstamp|mexc|bitget|crypto\.com|robinhood|bittrex|poloniex|upbit|bithumb|hitbtc|bitmart|exchange|deposit|hot ?wallet|bridge|router|wintermute|jump|cumberland|alameda|market ?maker|galaxy|amber|gsr|dwf|relay|gateway|hop|across|stargate|celer|multichain|synapse|peg|vault|staking|treasury|fund",
        )
        .expect("INFRA_RE is a compile-time constant")
    })
}


/// JS `Number.prototype.toString(36)` for a positive integer — the wallet id suffix.
pub fn radix36(mut n: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n <= 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("digits are ascii")
}


/// `^0x[0-9a-fA-F]{40}$` — the address guard the JS routes apply.
pub fn valid_addr(a: &str) -> bool {
    a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b.is_ascii_hexdigit())
}


/// The Ukrainian 400 body the JS forensics routes return for a bad address.
pub fn bad_addr() -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібна адреса 0x… (40 hex)"})))
}


/// `x || 0` for a JSON number field: 0, NaN, and missing all collapse to 0.
pub fn num_or0(v: &Value) -> f64 {
    match v.as_f64() {
        Some(n) if n != 0.0 && !n.is_nan() => n,
        _ => 0.0,
    }
}


/// current epoch milliseconds — the `ts` stamp Date.now() writes on an armed
/// baseline (nondeterministic, never part of parity checks).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}


/// model.js Model (dataset/model.json), lazily loaded once. `None` = not ready
/// (matches `model.ready()` being false), which yields a null ML overlay.
pub fn model_js() -> Option<&'static model::Model> {
    static M: std::sync::OnceLock<Option<model::Model>> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        let root = std::env::var("APP_ROOT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".."));
        model::Model::load(&root.join("dataset"))
    })
    .as_ref()
}

