//! ПАТЕРН capexch (Фаза 1.2 плану L4): додає БІРЖОВУ ГЛИБИНУ в оцінку реального капіталу.
//! capreal рахує реалізовне лише через DEX-пули; тут додаємо глибину стаканів бірж (orderbook.csv):
//!   realizable = min(папір, liqFrac·DEX-резерв + біржовий bid ±2%).
//! Позицію можна вийти і в пул, і в стакан — тож для токенів із лістингом оцінка ПОВНІША.
//! НЕ байт-паритет: це нова спроможність, цифра свідомо зростає для біржових токенів.
//! Читає cap-real.json (capreal) + orderbook.csv; нічого не мутує. Аргументи: top=15
use std::collections::HashMap;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct CapExch;

/// символ (BTC/ETH/…) -> найсвіжіша bid-глибина ±2% (USD) зі стаканів.
fn cex_bid(dir: &str) -> HashMap<String, (f64, u64)> {
    let mut m: HashMap<String, (f64, u64)> = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(format!("{dir}/orderbook.csv")) {
        for line in s.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 5 { continue }
            let ts: u64 = c[0].parse().unwrap_or(0);
            let sym = c[1].trim_end_matches("USDT").trim_end_matches("USDC").trim_end_matches("USD").to_uppercase();
            let bid: f64 = c[4].parse().unwrap_or(0.0); // bid_depth2pct_usd
            let e = m.entry(sym).or_insert((0.0, 0));
            if ts >= e.1 { *e = (bid, ts); } // найсвіжіший
        }
    }
    m
}

fn find_capreal(dir: &str) -> Option<String> {
    for p in [format!("{dir}/cap-real.json"), format!("{dir}/../cap-real.json"),
              "/opt/wallet-tracker/dataset/backfill/cap-real.json".to_string()] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

impl Pattern for CapExch {
    fn name(&self) -> &str { "capexch" }
    fn about(&self) -> &str { "додає біржову глибину стаканів у реальний капітал (Фаза 1.2)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let top = num(args, "top", 15.0) as usize;
        let path = find_capreal(&ctx.dir).ok_or("нема cap-real.json (спершу capreal)")?;
        let cr: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let bid = cex_bid(&ctx.dir);
        let groups = cr["groups"].as_array().ok_or("cap-real.json: нема groups")?;

        // сумарний ефект + по групах
        let mut rows: Vec<(String, f64, f64, f64, Vec<(String, f64)>)> = vec![]; // id, old_real, new_real, delta, [(sym,gain)]
        let (mut tot_old, mut tot_new) = (0.0f64, 0.0f64);
        for g in groups {
            let id = g["id"].as_str().unwrap_or("?").to_string();
            let liq_frac = g["liqFrac"].as_f64().unwrap_or(0.10);
            let native = g["nativeEthUsd"].as_f64().unwrap_or(0.0);
            let old_real = g["realTotalUsd"].as_f64().unwrap_or(0.0);
            let mut new_realiz = 0.0f64;
            let mut gains: Vec<(String, f64)> = vec![];
            if let Some(assets) = g["assets"].as_array() {
                for a in assets {
                    let paper = a["paperUsd"].as_f64().unwrap_or(0.0);
                    let reserve = a["reserveUsd"].as_f64().unwrap_or(0.0);
                    let sym = a["symbol"].as_str().unwrap_or("").to_uppercase();
                    let depth = if sym.is_empty() { 0.0 } else { bid.get(&sym).map(|(b, _)| *b).unwrap_or(0.0) };
                    let old_r = wr_dict::realizable_usd(paper, reserve, liq_frac);
                    let new_r = wr_dict::realizable_with_cex(paper, reserve, liq_frac, depth);
                    new_realiz += new_r;
                    if new_r > old_r + 1.0 { gains.push((sym, new_r - old_r)); }
                }
            }
            let new_real = native + new_realiz;
            gains.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap());
            tot_old += old_real; tot_new += new_real;
            rows.push((id, old_real, new_real, new_real - old_real, gains));
        }
        rows.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());

        println!("── КАПІТАЛ + БІРЖОВА ГЛИБИНА (Фаза 1.2, dict {}) ──", wr_dict::dict_version());
        println!("груп: {} · символів зі стаканами: {}", groups.len(), bid.len());
        println!("реальний капітал (усі групи): ${:.0} → ${:.0}  (+${:.0} завдяки біржовій глибині)",
            tot_old, tot_new, tot_new - tot_old);
        let affected = rows.iter().filter(|r| r.3 > 1.0).count();
        println!("груп, де біржова глибина щось додала: {affected} із {}", rows.len());
        println!("\n── топ груп за приростом реалізовного ──");
        for (id, old_r, new_r, delta, gains) in rows.iter().take(top) {
            if *delta <= 1.0 { break }
            let assets: Vec<String> = gains.iter().take(4).map(|(s, g)| format!("{s}+${:.0}", g)).collect();
            println!("  {:<10} ${:>14.0} → ${:>14.0}  (+${:.0})  [{}]", id, old_r, new_r, delta, assets.join(" "));
        }
        if affected == 0 {
            println!("(жодна група не тримає токенів із лістингом у orderbook.csv — розшир список символів у orderbook-cron)");
        }
        Ok(())
    }
}
