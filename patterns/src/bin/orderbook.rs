//! ЗБИРАЧ СТАКАНІВ (Фаза 1.1-1.2 + Дні 6-7 плану Oracul-Aladdin).
//! Знімає глибину стакана з біржі (Binance public, без ключа) і рахує РЕАЛЬНУ ліквідність:
//! скільки можна купити/продати в межах ±2% ковзання. Дописує знімок у часовий ряд.
//! Cron-ready: один запуск -> один рядок на символ -> вихід.
//!
//! Запуск: orderbook BTCUSDT ETHUSDT ARBUSDT ...    (дефолт — базовий кошик)
//! Вихід:  dataset/backfill/local-from-laptop/orderbook.csv (append)
//!   ts,symbol,mid,spread_bps,bid_depth2pct_usd,ask_depth2pct_usd,bid_qty,ask_qty
//! Це актив, якого немає в конкурентів: історія реальної ліквідності зростає сама щогодини.
use serde_json::Value;
use std::io::Write;

const OUT: &str = "/opt/wallet-tracker/dataset/backfill/local-from-laptop/orderbook.csv";
const HOSTS: [&str; 3] = ["https://api.binance.com", "https://api1.binance.com", "https://data-api.binance.vision"];

fn depth(symbol: &str) -> Option<Value> {
    for h in HOSTS {
        let url = format!("{h}/api/v3/depth?symbol={symbol}&limit=500");
        if let Ok(r) = ureq::get(&url).timeout(std::time::Duration::from_secs(20)).call() {
            if let Ok(v) = r.into_json::<Value>() { return Some(v) }
        }
    }
    None
}

/// глибина в USD у межах slip% від mid по одному боку
fn side_depth(levels: &[Value], mid: f64, slip: f64, is_bid: bool) -> (f64, f64) {
    let (mut usd, mut qty) = (0.0, 0.0);
    let bound = if is_bid { mid * (1.0 - slip) } else { mid * (1.0 + slip) };
    for lv in levels {
        let arr = lv.as_array().unwrap();
        let p: f64 = arr[0].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        let q: f64 = arr[1].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        if is_bid && p < bound { break }
        if !is_bid && p > bound { break }
        usd += p * q; qty += q;
    }
    (usd, qty)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let symbols: Vec<String> = if args.is_empty() {
        ["BTCUSDT","ETHUSDT","ARBUSDT","MATICUSDT","LINKUSDT","UNIUSDT","AAVEUSDT","ONDOUSDT"]
            .iter().map(|s| s.to_string()).collect()
    } else { args };

    // час (без Date/chrono — з системного годинника через SystemTime)
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);

    // заголовок, якщо файлу нема
    let new = !std::path::Path::new(OUT).exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(OUT)
        .unwrap_or_else(|e| { eprintln!("не відкрити {OUT}: {e}"); std::process::exit(1) });
    if new {
        let _ = writeln!(f, "ts,symbol,mid,spread_bps,bid_depth2pct_usd,ask_depth2pct_usd,bid_qty2pct,ask_qty2pct");
    }

    let mut ok = 0;
    for sym in &symbols {
        let Some(v) = depth(sym) else { eprintln!("{sym}: нема відповіді (гео-блок?)"); continue };
        let bids = v.get("bids").and_then(Value::as_array).cloned().unwrap_or_default();
        let asks = v.get("asks").and_then(Value::as_array).cloned().unwrap_or_default();
        if bids.is_empty() || asks.is_empty() { eprintln!("{sym}: порожній стакан"); continue }
        let best_bid: f64 = bids[0].as_array().unwrap()[0].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        let best_ask: f64 = asks[0].as_array().unwrap()[0].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        let mid = (best_bid + best_ask) / 2.0;
        if mid <= 0.0 { continue }
        let spread_bps = (best_ask - best_bid) / mid * 10_000.0;
        let (bd_usd, bd_qty) = side_depth(&bids, mid, 0.02, true);
        let (ad_usd, ad_qty) = side_depth(&asks, mid, 0.02, false);
        let _ = writeln!(f, "{ts},{sym},{mid:.6},{spread_bps:.2},{bd_usd:.0},{ad_usd:.0},{bd_qty:.4},{ad_qty:.4}");
        println!("{sym}: mid={mid:.4} spread={spread_bps:.1}bps · ліквідність ±2%: bid ${bd_usd:.0} / ask ${ad_usd:.0}");
        ok += 1;
    }
    eprintln!("[orderbook] знято {ok}/{} символів -> {OUT} (ts={ts})", symbols.len());
}
