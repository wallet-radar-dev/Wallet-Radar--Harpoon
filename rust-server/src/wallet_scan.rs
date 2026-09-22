//! Port of server/wallet-asset-scan.js + the `classifyTokens` kernel of
//! server/wallet-asset-groups.js — the background scan that takes every token a
//! watched wallet has ACQUIRED (current and historical) and looks for coordinated
//! whale groups inside each one.
//!
//! ── The airdrop-spam filter is a security control, not a data-quality nicety ──
//! Treating "everything that ever landed in the wallet" as its assets made Vitalik
//! Buterin whale #1 with 268 "assets" — WDOGE, SHIBAREBORN, GINUX… He bought none of
//! them. Anyone can send a token to a known address for the price of gas and thereby
//! inject themselves into our feed: poisoning the signal costs cents. So a token
//! counts only with EVIDENCE of acquisition — a swap in the same tx, the wallet
//! sending the token itself, or a materially-sized position.
//!
//! Network I/O (holders, transfers, group detection) stays in main.rs; everything
//! here is pure and unit-tested.

use crate::jsmath::{js_num, js_round};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// wallet-asset-groups.js:27 — below this a never-touched token is worthless spam.
const MIN_POSITION_USD: f64 = 1000.0;

#[derive(Default)]
struct Info {
    symbol: String,
    sent: bool,
    swapped: bool,
    in_usd: f64,
}

/// wallet-asset-groups.js `classifyTokens` — split a wallet's tokens into ones it
/// actually ACQUIRED and ones that merely arrived. Returns `(acquired, rejected)`.
///
/// `balances` are the current holdings (`{contract, symbol, usd}`), `transfers` the
/// rich token transfers (`{from, to, token, asset, usd, hash}`).
pub fn classify_tokens(balances: &[Value], transfers: &[Value], address: &str, min_usd: f64) -> (Vec<Value>, Vec<Value>) {
    let a = address.to_lowercase();

    // held: contract → (symbol, usd)
    let mut held: IndexMap<String, (String, f64)> = IndexMap::new();
    for b in balances {
        let Some(c) = b["contract"].as_str().filter(|s| !s.is_empty()) else { continue };
        held.insert(
            c.to_lowercase(),
            (b["symbol"].as_str().unwrap_or("").to_string(), b["usd"].as_f64().unwrap_or(0.0)),
        );
    }

    // Which tokens LEFT the wallet in each tx — the other leg of a swap.
    let mut sent_in_hash: IndexMap<String, HashSet<String>> = IndexMap::new();
    for t in transfers {
        let (from, token, hash) = (t["from"].as_str(), t["token"].as_str(), t["hash"].as_str());
        let (Some(token), Some(hash)) = (token.filter(|s| !s.is_empty()), hash.filter(|s| !s.is_empty())) else { continue };
        if from != Some(a.as_str()) {
            continue;
        }
        sent_in_hash.entry(hash.to_string()).or_default().insert(token.to_lowercase());
    }

    let mut info: IndexMap<String, Info> = IndexMap::new();
    for t in transfers {
        let Some(c) = t["token"].as_str().filter(|s| !s.is_empty()).map(str::to_lowercase) else { continue };
        let e = info.entry(c.clone()).or_insert_with(|| Info {
            symbol: t["asset"].as_str().unwrap_or("?").to_string(),
            ..Default::default()
        });
        if t["from"].as_str() == Some(a.as_str()) {
            e.sent = true;
        }
        if t["to"].as_str() == Some(a.as_str()) {
            e.in_usd = e.in_usd.max(t["usd"].as_f64().unwrap_or(0.0));
            // A swap: a DIFFERENT token left the wallet in this very tx → a purchase,
            // not a gift.
            if let Some(out) = t["hash"].as_str().and_then(|h| sent_in_hash.get(h)) {
                if out.iter().any(|x| *x != c) {
                    e.swapped = true;
                }
            }
        }
    }
    // A currently-held token with no transfer history still gets a row.
    for (c, h) in &held {
        info.entry(c.clone()).or_insert_with(|| Info { symbol: h.0.clone(), ..Default::default() });
    }

    let (mut acquired, mut rejected) = (Vec::new(), Vec::new());
    for (contract, e) in &info {
        let (pos_usd, sym) = held.get(contract).map(|h| (h.1, h.0.clone())).unwrap_or((0.0, e.symbol.clone()));
        let symbol = if sym.is_empty() { e.symbol.clone() } else { sym };
        let material = pos_usd >= min_usd || e.in_usd >= min_usd;
        let reason = if e.swapped {
            "swap"
        } else if e.sent {
            "sent"
        } else if material {
            "material"
        } else {
            // Arrived on its own, never touched, worth ~$0 → spam.
            rejected.push(json!({"contract": contract, "symbol": symbol, "reason": "airdrop-spam"}));
            continue;
        };
        acquired.push(json!({
            "contract": contract,
            "symbol": symbol,
            "acquiredVia": reason,
            "positionUsd": js_num(js_round(pos_usd)),
        }));
    }
    (acquired, rejected)
}

/// The default `minUsd` — exposed so callers do not each re-declare the threshold.
pub fn min_position_usd() -> f64 {
    MIN_POSITION_USD
}

// ── The scan's persisted state (wallet-asset-scan.js:17-22) ─────────────────────

struct State {
    file: PathBuf,
    result: Value,
    /// Contracts already announced, so a restart re-announces nobody.
    pushed: HashSet<String>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State { file: PathBuf::from("wallet-asset-scan.json"), result: empty_result(), pushed: HashSet::new() })
    })
}

fn empty_result() -> Value {
    json!({"generated": 0, "status": {"running": false, "processed": 0, "total": 0}, "assets": []})
}

pub fn init(dataset_dir: &Path) {
    let file = dataset_dir.join("wallet-asset-scan.json");
    let mut result = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(empty_result);
    // wallet-asset-scan.js:19 — clear a `running` flag left behind by a crash, or the
    // scan would refuse to start forever.
    if result["status"].is_object() {
        result["status"]["running"] = json!(false);
    }
    let pushed = result["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|x| x["hasGroups"] == json!(true))
                .filter_map(|x| x["contract"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut s = state().lock().unwrap();
    s.file = file;
    s.result = result;
    s.pushed = pushed;
}

/// `getWalletScan()` — served by /api/wallet-scan.
pub fn get() -> Value {
    state().lock().unwrap().result.clone()
}

/// True if a scan is already in flight (the JS guards its `run()` the same way).
pub fn is_running() -> bool {
    state().lock().unwrap().result["status"]["running"] == json!(true)
}

pub fn begin(total: usize) {
    let mut s = state().lock().unwrap();
    s.result["status"] = json!({"running": true, "processed": js_num(0.0), "total": js_num(total as f64)});
    persist(&mut s);
}

pub fn finish() {
    let mut s = state().lock().unwrap();
    s.result["status"]["running"] = json!(false);
    persist(&mut s);
}

/// Record one asset's result. ACCUMULATIVE: assets are merged by contract, never
/// wiped, so the cross-asset history a conductor is detected from survives a rescan.
/// Returns true if this contract is newly worth alerting on.
pub fn record(item: Value, now: i64) -> bool {
    let mut s = state().lock().unwrap();
    let contract = item["contract"].as_str().unwrap_or("").to_string();

    let mut by_c: IndexMap<String, Value> = s.result["assets"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["contract"].as_str().map(|c| (c.to_string(), x.clone()))).collect())
        .unwrap_or_default();
    by_c.insert(contract.clone(), item.clone());

    let mut assets: Vec<Value> = by_c.into_values().collect();
    assets.sort_by(|a, b| {
        let f = |v: &Value| v["topShare"].as_f64().unwrap_or(0.0);
        f(b).partial_cmp(&f(a)).unwrap_or(std::cmp::Ordering::Equal)
    });

    s.result["assets"] = Value::Array(assets);
    s.result["status"]["processed"] = js_num(s.result["status"]["processed"].as_f64().unwrap_or(0.0) + 1.0);
    s.result["generated"] = js_num(now as f64);
    persist(&mut s);

    // wallet-asset-scan.js:50 — "strong" is what earns a push, and only once.
    let strong = item["hasGroups"] == json!(true)
        && (item["topShare"].as_f64().unwrap_or(0.0) >= 5.0 || item["topBacker"].as_f64().unwrap_or(0.0) >= 500_000.0);
    strong && s.pushed.insert(contract)
}

fn persist(s: &mut std::sync::MutexGuard<State>) {
    if let Ok(text) = serde_json::to_string_pretty(&s.result) {
        let _ = crate::atomicw::write(&s.file, text);
    }
}

/// wallet-asset-scan.js:53 — the whale-trace alert for a newly-strong asset.
pub fn trace_alert(item: &Value, wallets: &[String], now: i64) -> Value {
    let empty = vec![];
    let top = item["groups"].as_array().unwrap_or(&empty).first().cloned().unwrap_or(Value::Null);
    let members = top["members"].as_array().cloned().unwrap_or_default();
    let backer = item["topBacker"].as_f64().unwrap_or(0.0);
    let backer_txt = if backer > 0.0 { format!(" · бекер ${}M", crate::jsmath::to_fixed(backer / 1e6, 2)) } else { String::new() };
    let sym = item["symbol"].as_str().unwrap_or("");
    json!({
        "type": "group-alert",
        "category": "whale-trace",
        "icon": "🐋",
        "title": format!("Слід китів у {sym}"),
        "body": format!(
            "{} холдерів = {}% емісії{backer_txt} · гаманці: {}",
            members.len(), item["topShare"], wallets.join(", ")
        ),
        "assets": [sym],
        "members": members.iter().map(|m| m["addr"].clone()).collect::<Vec<Value>>(),
        "valueUsd": js_num(backer),
        "ts": now,
    })
}

/// wallet-asset-scan.js:53 — the Telegram body for a whale-trace alert.
pub fn trace_telegram(item: &Value, wallets: &[String]) -> String {
    let empty = vec![];
    let top = item["groups"].as_array().unwrap_or(&empty).first().cloned().unwrap_or(Value::Null);
    let members = top["members"].as_array().map(|m| m.len()).unwrap_or(0);
    let backer = item["topBacker"].as_f64().unwrap_or(0.0);
    let backer_txt =
        if backer > 0.0 { format!(", бекер ${}M", crate::jsmath::to_fixed(backer / 1e6, 2)) } else { String::new() };
    format!(
        "🐋 <b>Слід китів</b> у <b>{}</b> (на гаманцях: {})\nГрупа: {members} холдерів = <b>{}%</b> емісії{backer_txt}",
        item["symbol"].as_str().unwrap_or(""),
        wallets.join(", "),
        item["topShare"]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "0xvitalik";
    const USDC: &str = "0xusdc";
    const GOOD: &str = "0xgood";
    const SPAM: &str = "0xspam";

    fn t(from: &str, to: &str, token: &str, asset: &str, usd: f64, hash: &str) -> Value {
        json!({"from": from, "to": to, "token": token, "asset": asset, "usd": usd, "hash": hash})
    }

    /// The attack this filter exists to stop: a worthless token is pushed into a famous
    /// wallet for the price of gas, and the wallet's "assets" become an attacker-chosen list.
    #[test]
    fn an_unsolicited_worthless_token_is_spam_not_an_asset() {
        let balances = json!([{"contract": SPAM, "symbol": "GINUX", "usd": 3}]);
        let transfers = json!([t("0xattacker", ME, SPAM, "GINUX", 0.0, "0xh1")]);
        let (acq, rej) = classify_tokens(balances.as_array().unwrap(), transfers.as_array().unwrap(), ME, MIN_POSITION_USD);

        assert!(acq.is_empty(), "never touched, worth $3 → not an asset");
        assert_eq!(rej.len(), 1);
        assert_eq!(rej[0]["reason"], json!("airdrop-spam"));
    }

    #[test]
    fn a_token_is_acquired_by_swap_by_sending_it_or_by_being_material() {
        let balances = json!([
            {"contract": GOOD, "symbol": "GOOD", "usd": 42},          // tiny position…
            {"contract": "0xheld", "symbol": "HELD", "usd": 5000},    // …but this one is material
            {"contract": "0xsold", "symbol": "SOLD", "usd": 0},       // sold out, still an asset
        ]);
        let transfers = json!([
            // swap: USDC leaves and GOOD arrives in the same tx → a purchase
            t(ME, "0xdex", USDC, "USDC", 100.0, "0xswap"),
            t("0xdex", ME, GOOD, "GOOD", 100.0, "0xswap"),
            // the wallet sent SOLD itself → it interacted with it
            t(ME, "0xbuyer", "0xsold", "SOLD", 900.0, "0xh2"),
        ]);
        let (acq, rej) = classify_tokens(balances.as_array().unwrap(), transfers.as_array().unwrap(), ME, MIN_POSITION_USD);

        let via = |c: &str| {
            acq.iter().find(|x| x["contract"] == json!(c)).map(|x| x["acquiredVia"].as_str().unwrap().to_string())
        };
        assert_eq!(via(GOOD).as_deref(), Some("swap"), "a $42 position still counts — it was bought");
        assert_eq!(via("0xsold").as_deref(), Some("sent"));
        assert_eq!(via("0xheld").as_deref(), Some("material"));
        assert_eq!(via(USDC).as_deref(), Some("sent"), "the swap's other leg left the wallet");
        assert!(rej.is_empty());
    }

    /// Receiving a token in a tx where the wallet sent nothing (or sent the SAME token
    /// onward) is not a swap — the JS compares against `x !== c` for exactly this.
    #[test]
    fn a_self_forward_in_one_hash_is_not_a_swap() {
        let balances = json!([]);
        let transfers = json!([
            t("0xsomeone", ME, SPAM, "SPAM", 0.0, "0xh"),
            t(ME, "0xelse", SPAM, "SPAM", 0.0, "0xh"), // same token out → sent, but not swapped
        ]);
        let (acq, _) = classify_tokens(balances.as_array().unwrap(), transfers.as_array().unwrap(), ME, MIN_POSITION_USD);
        assert_eq!(acq.len(), 1);
        assert_eq!(acq[0]["acquiredVia"], json!("sent"), "sent, not swapped");
    }

    #[test]
    fn a_large_incoming_transfer_is_material_even_if_the_position_is_now_zero() {
        let balances = json!([]);
        let transfers = json!([t("0xwhale", ME, GOOD, "GOOD", 50_000.0, "0xh")]);
        let (acq, rej) = classify_tokens(balances.as_array().unwrap(), transfers.as_array().unwrap(), ME, MIN_POSITION_USD);
        assert_eq!(acq.len(), 1);
        assert_eq!(acq[0]["acquiredVia"], json!("material"));
        assert_eq!(acq[0]["positionUsd"], json!(0));
        assert!(rej.is_empty());
    }
}
