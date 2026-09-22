//! ЗБИРАЧ БІРЖОВОЇ СТРІЧКИ УГОД (доповнює orderbook: глибина → ОБСЯГ+ПОТІК).
//! Binance klines 1h (public, без ключа): quoteVolume (USD-обсяг), takerBuyQuoteVolume
//! (агресивні КУПІВЛІ), numberOfTrades. З цього — обсяг, потік taker buy/sell, середній розмір угоди.
//! Це реальний CEX-обсяг (не глибина) — щоб міряти й біржовий ММ/потік, а не лише ончейн-мейкерів.
//! Дедуп за (ts,symbol): дописуємо лише свічки, новіші за останню записану → без прогалин і дублів.
//!
//! Запуск: tradetape BTCUSDT ETHUSDT ...  (дефолт — базовий кошик)
//! Вихід:  dataset/backfill/local-from-laptop/tradetape.csv (append)
//!   ts,symbol,close,quote_vol_usd,taker_buy_usd,taker_sell_usd,n_trades,avg_trade_usd,buy_pct
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};

const OUT: &str = "/opt/wallet-tracker/dataset/backfill/local-from-laptop/tradetape.csv";
const HOSTS: [&str; 3] = ["https://api.binance.com", "https://api1.binance.com", "https://data-api.binance.vision"];

fn klines(symbol: &str, limit: u32) -> Option<Value> {
    for h in HOSTS {
        let url = format!("{h}/api/v3/klines?symbol={symbol}&interval=1h&limit={limit}");
        if let Ok(r) = ureq::get(&url).timeout(std::time::Duration::from_secs(20)).call() {
            if let Ok(v) = r.into_json::<Value>() { return Some(v) }
        }
    }
    None
}

/// остання записана ts на символ (щоб дописувати лише новіші свічки)
fn last_ts() -> HashMap<String, i64> {
    let mut m = HashMap::new();
    if let Ok(f) = std::fs::File::open(OUT) {
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok).skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 2 { continue }
            if let Ok(ts) = c[0].parse::<i64>() {
                let e = m.entry(c[1].to_string()).or_insert(0);
                if ts > *e { *e = ts; }
            }
        }
    }
    m
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let symbols: Vec<String> = if args.is_empty() {
        ["BTCUSDT","ETHUSDT","LINKUSDT","UNIUSDT","AAVEUSDT","ONDOUSDT","PEPEUSDT","SHIBUSDT"]
            .iter().map(|s| s.to_string()).collect()
    } else { args };

    let last = last_ts();
    let new = !std::path::Path::new(OUT).exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(OUT)
        .unwrap_or_else(|e| { eprintln!("не відкрити {OUT}: {e}"); std::process::exit(1) });
    if new {
        let _ = writeln!(f, "ts,symbol,close,quote_vol_usd,taker_buy_usd,taker_sell_usd,n_trades,avg_trade_usd,buy_pct");
    }

    let mut rows = 0;
    for sym in &symbols {
        let Some(v) = klines(sym, 48) else { eprintln!("{sym}: нема відповіді (гео-блок?)"); continue };
        let Some(arr) = v.as_array() else { continue };
        let prev = last.get(sym).copied().unwrap_or(0);
        let mut wrote = 0;
        for k in arr {
            let Some(a) = k.as_array() else { continue };
            if a.len() < 11 { continue }
            let ts = a[0].as_i64().unwrap_or(0) / 1000; // openTime ms -> s
            if ts <= prev { continue }                  // лише новіші за записане
            let close: f64 = a[4].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            let qv: f64 = a[7].as_str().unwrap_or("0").parse().unwrap_or(0.0);   // quoteAssetVolume (USD)
            let n: i64 = a[8].as_i64().unwrap_or(0);                              // numberOfTrades
            let tbuy: f64 = a[10].as_str().unwrap_or("0").parse().unwrap_or(0.0); // takerBuyQuoteVolume (USD)
            let tsell = (qv - tbuy).max(0.0);
            let avg = if n > 0 { qv / n as f64 } else { 0.0 };
            let buy_pct = if qv > 0.0 { 100.0 * tbuy / qv } else { 0.0 };
            let _ = writeln!(f, "{ts},{sym},{close:.6},{qv:.0},{tbuy:.0},{tsell:.0},{n},{avg:.0},{buy_pct:.1}");
            wrote += 1;
        }
        if wrote > 0 { rows += wrote; println!("{sym}: +{wrote} год свічок"); }
    }
    eprintln!("[tradetape] дописано {rows} рядків -> {OUT}");
}
