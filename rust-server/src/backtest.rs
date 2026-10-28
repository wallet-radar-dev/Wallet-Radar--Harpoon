//! Port of server/backtest.js — the EVENT backtest of the "whale entered" signal:
//! does a whale's entry actually predict a rise?
//!
//! The previous design reported −37% and a 27% hit-rate, and every one of those
//! numbers was an artefact:
//!   1. SURVIVORSHIP — entries were reconstructed from TODAY's top holders, so a whale
//!      who bought and exited profitably is absent by construction. The average entry
//!      came out above the current price ARITHMETICALLY, signal or no signal.
//!   2. A SHARED EXIT DATE — every asset "sold" today, so returns were correlated and
//!      33 assets carried an effective sample size of a few.
//!   3. NO BENCHMARK — −37% while the market did −40% is a +3% EXCESS.
//!   4. WEEKLY CANDLES + an unbounded nearest-price search — a three-year-old entry
//!      quietly took the price from the start of the chart.
//!
//! Now: the FULL transfer history (so exited whales are in the sample), the entry event
//! is the moment an address first crosses WHALE_USD, and the return is measured over a
//! FIXED horizon from THAT event's own date, on daily candles, minus ETH over the same
//! window. Every event carries its own window — they no longer collapse onto one date.
//!
//! Network I/O stays in main.rs; everything here is pure and unit-tested.

use crate::cex::is_cex;
use crate::entities::entity_of;
use crate::jsmath::{js_num, js_round, round_fixed};
use crate::labels::Labels;
use crate::prices::{price_at_bounded, PRICE_TOLERANCE_MS};
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value};
use std::collections::HashSet;

/// The fixed holding horizon.
pub const HORIZON_DAYS: f64 = 30.0;
/// The position at which a holder counts as a whale in this asset.
pub const WHALE_USD: f64 = 25_000.0;
/// Pages of token transfer history (~50 each).
pub const TRANSFER_PAGES: u32 = 8;
const DAY: f64 = 86_400_000.0;


/// backtest.js `isTradeableHolder` — a whale is a person or a fund, not an exchange, a
/// bridge, or the token contract itself. Without this filter the "whale" is Binance's
/// hot wallet, which simply passes the entire float through itself.
pub fn is_tradeable_holder(addr: &str, contract: &str, labels: &Labels) -> bool {
    let ok_shape = addr.len() == 42
        && addr.starts_with("0x")
        && addr[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !ok_shape || addr == wr_store::addrs::zero() || addr == wr_store::addrs::dead() || addr == contract {
        return false;
    }
    !(is_cex(addr) || entity_of(addr).is_some() || labels.is_infra_addr(addr))
}

/// backtest.js `buildEntryEvents` — replay the whole transfer history in chronological
/// order carrying each address's balance. An event is the moment an address FIRST
/// crosses the whale threshold. One event per address: later top-ups are not "entries",
/// and counting them would overweight the most active traders.
pub fn build_entry_events(transfers: &[Value], series: &[Value], contract: &str, whale_usd: f64, labels: &Labels) -> Vec<Value> {
    let mut chrono: Vec<&Value> = transfers.iter().collect();
    chrono.sort_by(|a, b| {
        a["ts"].as_f64().unwrap_or(0.0).partial_cmp(&b["ts"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut bal: IndexMap<String, f64> = IndexMap::new();
    let mut entered: HashSet<String> = HashSet::new();
    let mut events: Vec<Value> = Vec::new();

    for t in chrono {
        let ts = t["ts"].as_f64().unwrap_or(0.0);
        if ts == 0.0 {
            continue;
        }
        let amount = t["amount"].as_f64().unwrap_or(0.0);
        if let Some(from) = t["from"].as_str().filter(|s| !s.is_empty()) {
            *bal.entry(from.to_string()).or_insert(0.0) -= amount;
        }
        if let Some(to) = t["to"].as_str().filter(|s| !s.is_empty()) {
            *bal.entry(to.to_string()).or_insert(0.0) += amount;
        }
        let Some(a) = t["to"].as_str().filter(|s| !s.is_empty()) else { continue };
        if entered.contains(a) || !is_tradeable_holder(a, contract, labels) {
            continue;
        }
        // No price within tolerance → we do not invent the event.
        let Some(px) = price_at_bounded(series, ts, PRICE_TOLERANCE_MS) else { continue };
        let usd = bal.get(a).copied().unwrap_or(0.0) * px;
        if usd >= whale_usd {
            entered.insert(a.to_string());
            events.push(json!({"addr": a, "ts": js_num(ts), "entryPrice": px, "positionUsd": js_num(js_round(usd))}));
        }
    }
    events
}

pub struct Forward {
    pub rows: Vec<Value>,
    pub unresolved: i64,
    pub no_price: i64,
}

/// backtest.js `forwardRows` — the return over a fixed horizon, minus the benchmark.
/// An event whose horizon has not closed yet is NOT silently dropped: it is counted as
/// `unresolved`, because a pending bet is not a losing bet.
pub fn forward_rows(events: &[Value], series: &[Value], bench: &[Value], horizon_days: f64, now: f64) -> Forward {
    let h = horizon_days * DAY;
    let mut rows = Vec::new();
    let (mut unresolved, mut no_price) = (0i64, 0i64);

    for e in events {
        let ts = e["ts"].as_f64().unwrap_or(0.0);
        let entry = e["entryPrice"].as_f64().unwrap_or(0.0);
        if ts + h > now {
            unresolved += 1;
            continue;
        }
        let (Some(p1), Some(b0), Some(b1)) = (
            price_at_bounded(series, ts + h, PRICE_TOLERANCE_MS),
            price_at_bounded(bench, ts, PRICE_TOLERANCE_MS),
            price_at_bounded(bench, ts + h, PRICE_TOLERANCE_MS),
        ) else {
            no_price += 1;
            continue;
        };
        // The JS guards these with `!p1 || !b0 || !b1`, so a zero price is a miss too.
        if p1 == 0.0 || b0 == 0.0 || b1 == 0.0 {
            no_price += 1;
            continue;
        }
        let ret = ((p1 - entry) / entry) * 100.0;
        let bench_ret = ((b1 - b0) / b0) * 100.0;
        rows.push(json!({
            "addr": e["addr"].clone(),
            "ts": e["ts"].clone(),
            "ret": js_num(round_fixed(ret, 1)),
            "benchRet": js_num(round_fixed(bench_ret, 1)),
            "excess": js_num(round_fixed(ret - bench_ret, 1)),
        }));
    }
    Forward { rows, unresolved, no_price }
}

/// backtest.js `wilson95` — the Wilson interval for a proportion, so a hit-rate is
/// never shown without its error bar. On 33 events the 95% CI is about ±17pp: that is
/// precisely why the old "27% versus 50%" proved nothing at all.
pub fn wilson95(successes: f64, n: f64) -> Value {
    if n == 0.0 {
        return Value::Null;
    }
    let z = 1.959964f64;
    let p = successes / n;
    let d = 1.0 + (z * z) / n;
    let c = p + (z * z) / (2.0 * n);
    let s = z * ((p * (1.0 - p)) / n + (z * z) / (4.0 * n * n)).sqrt();
    json!([
        js_num(js_round(((c - s) / d) * 100.0).max(0.0)),
        js_num(js_round(((c + s) / d) * 100.0).min(100.0)),
    ])
}

pub fn median(xs: &[f64]) -> Value {
    if xs.is_empty() {
        return Value::Null;
    }
    let mut s = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = s.len() >> 1;
    let v = if s.len() % 2 == 1 { s[m] } else { (s[m - 1] + s[m]) / 2.0 };
    js_num(round_fixed(v, 1))
}

pub fn mean(xs: &[f64]) -> Value {
    if xs.is_empty() {
        return Value::Null;
    }
    js_num(round_fixed(xs.iter().sum::<f64>() / xs.len() as f64, 1))
}

/// backtest.js `aggregate` — the signal. `hitRate` is the share of events with a
/// POSITIVE excess over ETH, never the raw return: raw return just measures market beta.
pub fn aggregate(rows: &[Value], extra: &[(&str, Value)]) -> Value {
    if rows.is_empty() {
        return Value::Null;
    }
    let f = |k: &str| -> Vec<f64> { rows.iter().map(|r| r[k].as_f64().unwrap_or(0.0)).collect() };
    let exc = f("excess");
    let raw = f("ret");
    let wins = exc.iter().filter(|x| **x > 0.0).count() as f64;
    let wallets: IndexSet<&str> = rows.iter().filter_map(|r| r["addr"].as_str()).collect();
    let n = rows.len() as f64;

    let mut o = serde_json::Map::new();
    o.insert("samples".into(), js_num(n));
    o.insert("wallets".into(), js_num(wallets.len() as f64));
    o.insert("hitRate".into(), js_num(js_round((wins / n) * 100.0)));
    o.insert("hitRateCi95".into(), wilson95(wins, n));
    o.insert("meanExcess".into(), mean(&exc));
    o.insert("medianExcess".into(), median(&exc));
    o.insert("meanRaw".into(), mean(&raw));
    o.insert("medianRaw".into(), median(&raw));
    o.insert("meanBench".into(), mean(&f("benchRet")));
    for (k, v) in extra {
        o.insert((*k).to_string(), v.clone());
    }
    Value::Object(o)
}

/// backtest.js:176 — the method block, shipped WITH its caveats so the number is never
/// read as more than it is.
pub fn method() -> Value {
    json!({
        "horizonDays": js_num(HORIZON_DAYS),
        "benchmark": "ETH",
        "whaleUsd": js_num(WHALE_USD),
        "priceGranularity": "1d",
        "caveats": [
            format!("історія переказів обмежена {TRANSFER_PAGES} сторінками — для дуже активних токенів це не весь час життя"),
            "події одного активу перекриваються в часі, тож вони не є повністю незалежними спостереженнями",
            "денні свічки за 365 днів: входи, старші за рік, у вибірку не потрапляють",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const C: &str = "0xc0ffee0000000000000000000000000000000000";
    const WHALE: &str = "0xaaaa000000000000000000000000000000000001";
    const BINANCE: &str = "0x28c6c06298d514db089934071355e5743bf21d60";

    fn labels() -> Labels {
        Labels::load(Path::new("/nonexistent"))
    }
    fn day(n: f64) -> f64 {
        1_700_000_000_000.0 + n * DAY
    }
    /// A flat $1/token series over 200 days, so USD == token amount.
    fn series(price: f64) -> Vec<Value> {
        (0..200).map(|i| json!({"ts": js_num(day(i as f64)), "price": price})).collect()
    }
    fn tr(from: &str, to: &str, amount: f64, ts: f64) -> Value {
        json!({"from": from, "to": to, "amount": amount, "ts": js_num(ts)})
    }

    #[test]
    fn an_entry_is_the_first_crossing_of_the_whale_threshold_and_fires_once() {
        store_or_skip!();
        let s = series(1.0);
        let transfers = json!([
            tr("0xmint", WHALE, 10_000.0, day(1.0)),  // $10k — not yet a whale
            tr("0xmint", WHALE, 20_000.0, day(2.0)),  // now $30k → ENTRY
            tr("0xmint", WHALE, 50_000.0, day(3.0)),  // a top-up is not a new entry
        ]);
        let ev = build_entry_events(transfers.as_array().unwrap(), &s, C, WHALE_USD, &labels());

        assert_eq!(ev.len(), 1, "one event per address");
        assert_eq!(ev[0]["ts"], js_num(day(2.0)));
        assert_eq!(ev[0]["positionUsd"], json!(30_000));
    }

    /// The filter that stops an exchange hot wallet from being the "whale".
    #[test]
    fn exchanges_the_zero_address_and_the_contract_itself_are_never_whales() {
        store_or_skip!();
        let s = series(1.0);
        let transfers = json!([
            tr("0xmint", BINANCE, 999_999.0, day(1.0)),
            tr("0xmint", wr_store::addrs::zero(), 999_999.0, day(1.0)),
            tr("0xmint", wr_store::addrs::dead(), 999_999.0, day(1.0)),
            tr("0xmint", C, 999_999.0, day(1.0)),
            tr("0xmint", "0xNOTHEX", 999_999.0, day(1.0)),
        ]);
        assert!(build_entry_events(transfers.as_array().unwrap(), &s, C, WHALE_USD, &labels()).is_empty());
    }

    /// Selling back below the threshold and re-buying does not create a second event —
    /// and, more importantly, a whale who EXITED is still in the sample (no survivorship).
    #[test]
    fn a_whale_who_exits_stays_in_the_sample() {
        store_or_skip!();
        let s = series(1.0);
        let transfers = json!([
            tr("0xmint", WHALE, 30_000.0, day(1.0)), // entry
            tr(WHALE, "0xdex", 30_000.0, day(5.0)),  // fully exits
            tr("0xmint", WHALE, 30_000.0, day(9.0)), // buys back — not a new entry
        ]);
        let ev = build_entry_events(transfers.as_array().unwrap(), &s, C, WHALE_USD, &labels());
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0]["ts"], js_num(day(1.0)));
    }

    #[test]
    fn the_return_is_measured_against_eth_over_the_events_own_window() {
        // Token doubles over the horizon; ETH rises 50% → excess = +50pp.
        let token: Vec<Value> = (0..200).map(|i| json!({"ts": js_num(day(i as f64)), "price": if i as f64 >= 31.0 { 2.0 } else { 1.0 }})).collect();
        let bench: Vec<Value> = (0..200).map(|i| json!({"ts": js_num(day(i as f64)), "price": if i as f64 >= 31.0 { 15.0 } else { 10.0 }})).collect();
        let events = json!([{"addr": WHALE, "ts": js_num(day(1.0)), "entryPrice": 1.0, "positionUsd": 30000}]);

        let f = forward_rows(events.as_array().unwrap(), &token, &bench, HORIZON_DAYS, day(120.0));
        assert_eq!(f.rows.len(), 1);
        assert_eq!(f.rows[0]["ret"], json!(100), "js_num: an integral toFixed(1) is an int, not 100.0");
        assert_eq!(f.rows[0]["benchRet"], json!(50));
        assert_eq!(f.rows[0]["excess"], json!(50));
        assert_eq!(f.unresolved, 0);
    }

    /// A bet whose horizon has not closed is pending, not lost. Silently dropping these
    /// would bias the sample toward whatever already resolved.
    #[test]
    fn an_open_horizon_is_unresolved_not_a_loss() {
        let s = series(1.0);
        let events = json!([{"addr": WHALE, "ts": js_num(day(100.0)), "entryPrice": 1.0, "positionUsd": 30000}]);
        let f = forward_rows(events.as_array().unwrap(), &s, &s, HORIZON_DAYS, day(110.0));

        assert!(f.rows.is_empty());
        assert_eq!(f.unresolved, 1);
        assert_eq!(f.no_price, 0);
    }

    #[test]
    fn a_hit_rate_never_ships_without_its_confidence_interval() {
        let row = |excess: f64, addr: &str| json!({"addr": addr, "ts": 1, "ret": excess, "benchRet": 0.0, "excess": excess});
        let rows = json!([row(10.0, WHALE), row(-5.0, WHALE), row(20.0, "0xbbbb"), row(30.0, "0xcccc")]);
        let a = aggregate(rows.as_array().unwrap(), &[("assetsScored", json!(1))]);

        assert_eq!(a["samples"], json!(4));
        assert_eq!(a["wallets"], json!(3), "the same wallet twice is one wallet");
        assert_eq!(a["hitRate"], json!(75));
        // Wilson on 3/4 is wide — which is the point of showing it.
        assert_eq!(a["hitRateCi95"], json!([30, 95]));
        assert_eq!(a["meanExcess"], json!(13.8));
        assert_eq!(a["medianExcess"], json!(15));
        assert_eq!(a["assetsScored"], json!(1));

        assert_eq!(aggregate(&[], &[]), Value::Null, "no rows → no signal, not a zero");
    }
}
