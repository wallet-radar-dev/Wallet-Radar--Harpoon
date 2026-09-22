//! Port of server/liquidations.js — forced-liquidation monitor on Bybit's public v5
//! WebSocket (keyless). Aggregates 5m/1h/24h windows (longs vs shorts + top symbols)
//! and pushes a live-feed event when a 5-minute flush crosses the threshold.
//!
//! Bybit rather than Binance: the Binance futures stream handshakes but delivers no
//! frames from some regions.
//!
//! In-memory only — it writes no dataset file, so unlike the file-refreshing workers
//! it needs no ENABLE_* gate: running it alongside Node just means both read the same
//! public stream.

use crate::jsmath::{js_num, js_round, to_fixed};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const WS_URL: &str = "wss://stream.bybit.com/v5/public/linear";
/// No all-market stream exists, so we subscribe to the most liquid symbols.
const SYMBOLS: [&str; 30] = [
    "BTCUSDT", "ETHUSDT", "SOLUSDT", "XRPUSDT", "DOGEUSDT", "1000PEPEUSDT", "WIFUSDT", "ADAUSDT",
    "AVAXUSDT", "LINKUSDT", "SUIUSDT", "ENAUSDT", "AAVEUSDT", "LTCUSDT", "BNBUSDT", "TRXUSDT",
    "TONUSDT", "NEARUSDT", "APTUSDT", "ARBUSDT", "OPUSDT", "INJUSDT", "TIAUSDT", "SEIUSDT",
    "1000BONKUSDT", "FARTCOINUSDT", "HYPEUSDT", "ORDIUSDT", "WLDUSDT", "CRVUSDT",
];
const CAP: usize = 8000;
const MAX_AGE_MS: i64 = 24 * 3600 * 1000;
const FLUSH_COOLDOWN_MS: i64 = 10 * 60 * 1000;

#[derive(Clone)]
struct Entry {
    ts: i64,
    symbol: String,
    side: &'static str,
    usd: f64,
}

#[derive(Default)]
struct State {
    buf: VecDeque<Entry>,
    connected: bool,
    connected_at: i64,
    last_flush_alert: i64,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// `Number(process.env.LIQ_FLUSH_USD) || 20_000_000`.
fn flush_threshold() -> f64 {
    std::env::var("LIQ_FLUSH_USD")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| *n != 0.0 && n.is_finite())
        .unwrap_or(20_000_000.0)
}

/// liquidations.js `parseLiquidations` — one Bybit `allLiquidation` frame → entries.
/// `S:"Sell"` means a LONG was liquidated; `S:"Buy"` means a SHORT was.
pub fn parse_liquidations(raw: &str) -> Vec<Value> {
    let Ok(m) = serde_json::from_str::<Value>(raw) else { return vec![] };
    if !m["topic"].as_str().unwrap_or("").starts_with("allLiquidation") {
        return vec![];
    }
    let empty = vec![];
    m["data"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|d| {
            // Number(d.v) * Number(d.p) — both arrive as strings.
            let qty = num(&d["v"]);
            let price = num(&d["p"]);
            let usd = qty * price;
            // `!usd || !isFinite(usd)` — a zero or a NaN is skipped.
            if usd == 0.0 || !usd.is_finite() {
                return None;
            }
            let side = if d["S"].as_str() == Some("Sell") { "long" } else { "short" };
            let symbol = d["s"].as_str().unwrap_or("");
            Some(json!({"symbol": strip_quote(symbol), "side": side, "usd": usd}))
        })
        .collect()
}

/// `String(d.s).replace(/USDT$|USDC$|PERP$/, "")` — one suffix, anchored at the end.
fn strip_quote(s: &str) -> &str {
    for suffix in ["USDT", "USDC", "PERP"] {
        if let Some(base) = s.strip_suffix(suffix) {
            return base;
        }
    }
    s
}

/// `Number(x)` over a JSON value that may be a string or a number.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// liquidations.js `agg(sinceMs)` — longs/shorts/total/count + the top 6 symbols.
fn agg(buf: &VecDeque<Entry>, since_ms: i64) -> Value {
    let (mut long, mut short, mut count) = (0.0f64, 0.0f64, 0usize);
    // Insertion-ordered, so ties keep first-seen order like the JS Map.
    let mut by_sym: Vec<(String, f64)> = Vec::new();
    for e in buf.iter().filter(|e| e.ts >= since_ms) {
        count += 1;
        if e.side == "long" {
            long += e.usd;
        } else {
            short += e.usd;
        }
        match by_sym.iter_mut().find(|(s, _)| *s == e.symbol) {
            Some((_, u)) => *u += e.usd,
            None => by_sym.push((e.symbol.clone(), e.usd)),
        }
    }
    by_sym.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top: Vec<Value> = by_sym
        .into_iter()
        .take(6)
        .map(|(symbol, usd)| json!({"symbol": symbol, "usd": js_num(js_round(usd))}))
        .collect();
    json!({
        "long": js_num(js_round(long)),
        "short": js_num(js_round(short)),
        "total": js_num(js_round(long + short)),
        "count": count,
        "top": top,
    })
}

/// liquidations.js `getLiquidations()`.
pub fn get_liquidations(now: i64) -> Value {
    let s = state().lock().unwrap();
    json!({
        "connected": s.connected,
        "connectedAt": s.connected_at,
        "events": s.buf.len(),
        "w5m": agg(&s.buf, now - 5 * 60 * 1000),
        "w1h": agg(&s.buf, now - 60 * 60 * 1000),
        "w24h": agg(&s.buf, now - 24 * 3600 * 1000),
        "flushThreshold": js_num(js_round(flush_threshold())),
        "ts": now,
    })
}

/// liquidations.js `fUsd` — the money formatter used in the alert copy.
fn f_usd(n: f64) -> String {
    // to_fixed, not Rust's `{:.N}`: JS rounds half AWAY from zero (4.5 → "5"),
    // Rust rounds half to even (4.5 → "4").
    if n >= 1e9 {
        format!("${}B", to_fixed(n / 1e9, 2))
    } else if n >= 1e6 {
        format!("${}M", to_fixed(n / 1e6, 1))
    } else if n >= 1e3 {
        format!("${}k", to_fixed(n / 1e3, 0))
    } else {
        format!("${}", js_num(js_round(n)))
    }
}

/// liquidations.js `onMessage` + `prune` + `checkFlush`. Returns the alert to
/// broadcast, if this frame tipped a 5-minute window over the threshold.
fn ingest(raw: &str, now: i64) -> Option<Value> {
    let items = parse_liquidations(raw);
    if items.is_empty() {
        return None;
    }
    let mut s = state().lock().unwrap();
    for it in &items {
        s.buf.push_back(Entry {
            ts: now,
            symbol: it["symbol"].as_str().unwrap_or("").to_string(),
            side: if it["side"] == json!("long") { "long" } else { "short" },
            usd: it["usd"].as_f64().unwrap_or(0.0),
        });
    }
    // prune: drop anything older than 24h, then cap.
    let cut = now - MAX_AGE_MS;
    while s.buf.front().is_some_and(|e| e.ts < cut) {
        s.buf.pop_front();
    }
    while s.buf.len() > CAP {
        s.buf.pop_front();
    }

    // checkFlush
    if now - s.last_flush_alert < FLUSH_COOLDOWN_MS {
        return None;
    }
    let a = agg(&s.buf, now - 5 * 60 * 1000);
    let total = a["total"].as_f64().unwrap_or(0.0);
    if total < flush_threshold() {
        return None;
    }
    s.last_flush_alert = now;
    let (long, short) = (a["long"].as_f64().unwrap_or(0.0), a["short"].as_f64().unwrap_or(0.0));
    let dir = if long > short { "long" } else { "short" };
    let empty = vec![];
    let top = a["top"].as_array().unwrap_or(&empty);
    let syms: Vec<&str> = top.iter().filter_map(|x| x["symbol"].as_str()).filter(|s| !s.is_empty()).collect();
    let top_syms = syms.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    let body = if top_syms.is_empty() {
        format!("Лонги {} · Шорти {}", f_usd(long), f_usd(short))
    } else {
        format!("Лонги {} · Шорти {} · топ: {}", f_usd(long), f_usd(short), top_syms)
    };
    // alerts.js groupAlert shape, minus the Telegram push (the bot is not ported).
    Some(json!({
        "type": "group-alert",
        "category": "liquidation",
        "icon": "💥",
        "title": format!("Ліквідаційний флеш: {} за 5хв", f_usd(total)),
        "body": body,
        "valueUsd": total,
        "assets": syms.iter().take(6).collect::<Vec<_>>(),
        "meta": {"long": long, "short": short, "dir": dir, "top": top},
        "ts": now,
    }))
}

/// liquidations.js `startLiquidationMonitor` — connect, subscribe, ping every 20s,
/// reconnect with the JS's doubling backoff (2s → 60s cap).
pub async fn run(tx: tokio::sync::broadcast::Sender<String>) {
    let mut delay = Duration::from_secs(2);
    tracing::info!("[liq] monitor started (Bybit v5 WS, {} symbols, flush {}/5min)", SYMBOLS.len(), f_usd(flush_threshold()));
    loop {
        match tokio_tungstenite::connect_async(WS_URL).await {
            Ok((mut ws, _)) => {
                delay = Duration::from_secs(2);
                {
                    let mut s = state().lock().unwrap();
                    s.connected = true;
                    s.connected_at = crate::now_ms();
                }
                let args: Vec<String> = SYMBOLS.iter().map(|s| format!("allLiquidation.{s}")).collect();
                let sub = json!({"op": "subscribe", "args": args}).to_string();
                if ws.send(tokio_tungstenite::tungstenite::Message::Text(sub.into())).await.is_err() {
                    // fall through to the reconnect path
                }
                let mut ping = tokio::time::interval(Duration::from_secs(20));
                ping.tick().await; // the first tick fires immediately
                loop {
                    tokio::select! {
                        _ = ping.tick() => {
                            let p = json!({"op": "ping"}).to_string();
                            if ws.send(tokio_tungstenite::tungstenite::Message::Text(p.into())).await.is_err() {
                                break;
                            }
                        }
                        msg = ws.next() => match msg {
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                                if let Some(alert) = ingest(&t, crate::now_ms()) {
                                    tracing::info!("[liq] flush alert: {}", alert["title"]);
                                    let _ = tx.send(alert.to_string());
                                }
                            }
                            Some(Ok(_)) => {}
                            _ => break, // close or error
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("[liq] connect failed: {e}"),
        }
        state().lock().unwrap().connected = false;
        tokio::time::sleep(delay).await;
        delay = std::cmp::min(Duration::from_secs(60), delay * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_bybit_frame() {
        let raw = r#"{"topic":"allLiquidation.BTCUSDT","data":[
            {"T":1,"s":"BTCUSDT","S":"Sell","v":"0.5","p":"60000"},
            {"T":2,"s":"ETHUSDC","S":"Buy","v":"10","p":"3000"},
            {"T":3,"s":"XRPUSDT","S":"Buy","v":"0","p":"2"}
        ]}"#;
        let out = parse_liquidations(raw);
        assert_eq!(out.len(), 2, "a zero-notional entry is dropped");
        // S=Sell → a long was liquidated; the USDT suffix is stripped.
        assert_eq!(out[0], json!({"symbol": "BTC", "side": "long", "usd": 30000.0}));
        // S=Buy → a short; USDC is stripped too.
        assert_eq!(out[1], json!({"symbol": "ETH", "side": "short", "usd": 30000.0}));
    }

    #[test]
    fn ignores_frames_that_are_not_liquidations() {
        assert!(parse_liquidations(r#"{"topic":"orderbook.1.BTCUSDT","data":[]}"#).is_empty());
        assert!(parse_liquidations(r#"{"op":"pong"}"#).is_empty());
        assert!(parse_liquidations("not json").is_empty());
    }

    #[test]
    fn agg_windows_split_longs_from_shorts_and_rank_symbols() {
        let mut buf = VecDeque::new();
        let mut push = |ts, symbol: &str, side, usd| {
            buf.push_back(Entry { ts, symbol: symbol.into(), side, usd });
        };
        push(1_000, "BTC", "long", 100.0);
        push(2_000, "BTC", "short", 50.0);
        push(3_000, "ETH", "long", 400.0);
        push(500, "SOL", "long", 999.0); // outside the window below

        let a = agg(&buf, 1_000);
        assert_eq!(a["long"], json!(500)); // 100 + 400, the SOL entry excluded
        assert_eq!(a["short"], json!(50));
        assert_eq!(a["total"], json!(550));
        assert_eq!(a["count"], json!(3));
        // Ranked by usd desc: ETH 400, BTC 150.
        assert_eq!(a["top"], json!([{"symbol": "ETH", "usd": 400}, {"symbol": "BTC", "usd": 150}]));
    }

    #[test]
    fn money_formatter_matches_the_js() {
        assert_eq!(f_usd(2_500_000_000.0), "$2.50B");
        assert_eq!(f_usd(20_000_000.0), "$20.0M");
        assert_eq!(f_usd(4_500.0), "$5k"); // toFixed(0) rounds half away from zero
        assert_eq!(f_usd(12.4), "$12");
    }
}
