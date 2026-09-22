//! ПАТЕРН cexflow: БІРЖОВА ММ-СИГНАТУРА зі стрічки угод (tradetape.csv).
//! ММ на біржі не видно поіменно, але його ПОЧЕРК видно у потоці:
//!   • buy% (taker) стабільно ≈50% + мала σ  → двостороннє котирування (мейкер працює);
//!   • дрібна середня угода                   → гранулярність мейкера/роздробу (не кит);
//!   • обсяг/глибина (velocity)               → скільки разів прокручено видиму книгу ±2%;
//!   • нетто-потік (Σ taker_buy−sell)         → напрямний тиск (накопичення/роздача на біржі).
//! Звʼязок з ОНЧЕЙН-ММ (mmfind): де токен має і біржовий мейкінг, і ончейн-мейкінг групи.
//! Читає tradetape.csv + orderbook.csv + mmfind.json. Аргументи: top=25 sort=vol|net|velocity min_hrs=3
use std::collections::HashMap;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct CexFlow;

fn find(dir: &str, name: &str) -> Option<String> {
    for p in [format!("{dir}/{name}"), format!("{dir}/../{name}"),
              format!("/opt/wallet-tracker/dataset/backfill/{name}")] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

fn strip(sym: &str) -> String {
    sym.trim_end_matches("USDT").trim_end_matches("USDC").trim_end_matches("USD").to_uppercase()
}

/// глибина ±2% (bid+ask, найсвіжіша) на символ з orderbook.csv
fn depth(dir: &str) -> HashMap<String, f64> {
    let mut m: HashMap<String, (f64, u64)> = HashMap::new();
    if let Some(p) = find(dir, "orderbook.csv") {
        if let Ok(s) = std::fs::read_to_string(&p) {
            for line in s.lines().skip(1) {
                let c: Vec<&str> = line.split(',').collect();
                if c.len() < 6 { continue }
                let ts: u64 = c[0].parse().unwrap_or(0);
                let sym = strip(c[1]);
                let d = c[4].parse::<f64>().unwrap_or(0.0) + c[5].parse::<f64>().unwrap_or(0.0);
                let e = m.entry(sym).or_insert((0.0, 0));
                if ts >= e.1 { *e = (d, ts); }
            }
        }
    }
    m.into_iter().map(|(k, (d, _))| (k, d)).collect()
}

/// ончейн-ММ частка потоку групи (mmfind share) на символ
fn onchain_mm(dir: &str) -> HashMap<String, f64> {
    find(dir, "mmfind.json").and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["share"].as_array().map(|a| a.iter()
            .filter_map(|s| Some((s["symbol"].as_str()?.to_uppercase(), s["mmSharePct"].as_f64()?))).collect()))
        .unwrap_or_default()
}

struct Agg { vol: f64, tbuy: f64, tsell: f64, trades: f64, hrs: usize, bps: Vec<f64> }

impl Pattern for CexFlow {
    fn name(&self) -> &str { "cexflow" }
    fn about(&self) -> &str { "біржова ММ-сигнатура зі стрічки угод: потік, розмір, velocity, звʼязок з ончейн-ММ" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let top = num(args, "top", 25.0) as usize;
        let min_hrs = num(args, "min_hrs", 3.0) as usize;
        let sort = args.get("sort").map(|s| s.as_str()).unwrap_or("vol");

        let path = find(&ctx.dir, "tradetape.csv").ok_or("нема tradetape.csv (спершу бін tradetape)")?;
        let txt = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let dep = depth(&ctx.dir);
        let onc = onchain_mm(&ctx.dir);

        // агрегуємо стрічку по символу
        let mut by: HashMap<String, Agg> = HashMap::new();
        for line in txt.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 9 { continue }
            let sym = strip(c[1]);
            let vol: f64 = c[3].parse().unwrap_or(0.0);
            let tbuy: f64 = c[4].parse().unwrap_or(0.0);
            let tsell: f64 = c[5].parse().unwrap_or(0.0);
            let n: f64 = c[6].parse().unwrap_or(0.0);
            let bp: f64 = c[8].parse().unwrap_or(0.0);
            let e = by.entry(sym).or_insert(Agg { vol: 0.0, tbuy: 0.0, tsell: 0.0, trades: 0.0, hrs: 0, bps: vec![] });
            e.vol += vol; e.tbuy += tbuy; e.tsell += tsell; e.trades += n; e.hrs += 1; e.bps.push(bp);
        }

        // (sym, vol_hr, tph, avg_trade, buy_mean, buy_std, velocity, net_flow, onchain%, signature)
        let mut rows: Vec<(String, f64, f64, f64, f64, f64, f64, f64, f64, String)> = vec![];
        for (sym, a) in &by {
            if a.hrs < min_hrs { continue }
            let vol_hr = a.vol / a.hrs as f64;
            let tph = a.trades / a.hrs as f64;
            let avg = if a.trades > 0.0 { a.vol / a.trades } else { 0.0 };
            let bm = a.bps.iter().sum::<f64>() / a.bps.len() as f64;
            let bstd = (a.bps.iter().map(|x| (x - bm).powi(2)).sum::<f64>() / a.bps.len() as f64).sqrt();
            let d = dep.get(sym).copied().unwrap_or(0.0);
            let vel = if d > 0.0 { vol_hr / d } else { -1.0 };
            let net = a.tbuy - a.tsell;
            let onchain = onc.get(sym).or_else(|| onc.get(&format!("W{sym}"))).copied().unwrap_or(-1.0);
            // сигнатура
            let sig = if (bm - 50.0).abs() < 4.0 && bstd < 8.0 { "ММ-щільно" }
                      else if bstd > 12.0 { "волатильно" }
                      else if bm >= 54.0 { "тиск-КУПІВЛЯ" }
                      else if bm <= 46.0 { "тиск-ПРОДАЖ" }
                      else { "змішано" };
            rows.push((sym.clone(), vol_hr, tph, avg, bm, bstd, vel, net, onchain, sig.into()));
        }
        match sort {
            "net" => rows.sort_by(|a, b| b.7.abs().partial_cmp(&a.7.abs()).unwrap()),
            "velocity" => rows.sort_by(|a, b| b.6.partial_cmp(&a.6).unwrap()),
            _ => rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap()),
        }

        println!("── CEX-ПОТІК / ММ-СИГНАТУРА (tradetape) ──");
        println!("символів: {} · вікно годин (макс): {}", rows.len(), by.values().map(|a| a.hrs).max().unwrap_or(0));
        println!("{:<7} {:>11} {:>7} {:>9} {:>10} {:>9} {:>11} {:>8} {}",
            "токен", "обсяг/год$", "уг/год", "сер.уг$", "buy%±σ", "vol/深", "нетто$", "ончММ%", "сигнатура");
        for (s, vh, tph, avg, bm, bstd, vel, net, onc, sig) in rows.iter().take(top) {
            let vels = if *vel < 0.0 { "?".into() } else { format!("{:.2}", vel) };
            let oncs = if *onc < 0.0 { "—".into() } else { format!("{:.0}%", onc) };
            println!("{:<7} {:>11.0} {:>7.0} {:>9.0} {:>6.0}±{:>2.0} {:>9} {:>+11.0} {:>8} {}",
                s, vh, tph, avg, bm, bstd, vels, net, oncs, sig);
        }
        println!("\nЧИТАТИ:");
        println!("• «buy%±σ» ≈50 з малою σ = двостороннє котирування (мейкер тримає ринок); скіс = напрямний тиск;");
        println!("• «vol/深» — обсяг/год ÷ глибина±2%: скільки разів прокручено видиму книгу (velocity, >1 = тонка книга під обсяг);");
        println!("• «нетто$» — Σ(taker_buy−sell): + = агресивне НАКОПИЧЕННЯ на біржі, − = роздача;");
        println!("• «ончММ%» — частка ончейн-потоку від мейкерів групи (mmfind): є і там, і там = група робить ринок в обох.");
        println!("ЗАСТЕРЕЖЕННЯ: Binance spot; особу ММ поіменно не видно — це ПОЧЕРК потоку (проксі), не рахунки.");
        Ok(())
    }
}
