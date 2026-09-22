//! ПАТЕРН scenario (Фаза 3.1 плану L6): СЦЕНАРНИЙ АНАЛІЗ розпродажу.
//! «Група роздає N% позицій за M днів при ПОТОЧНІЙ ліквідності → яка просадка?»
//! Глибина = DEX-резерв (приведений до ±2%) + біржовий bid ±2% (стакани). Модель лінійна
//! (нижня межа — реальний стакан тоншає глибше), просадка показана з інтервалом впевненості.
//! Читає cap-real.json + orderbook.csv; нічого не мутує.
//! Аргументи: group=<id|all>  pct=30  days=7  top=15  min_usd=100000
use std::collections::HashMap;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Scenario;

fn cex_bid(dir: &str) -> HashMap<String, f64> {
    let mut m: HashMap<String, (f64, u64)> = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(format!("{dir}/orderbook.csv")) {
        for line in s.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 5 { continue }
            let ts: u64 = c[0].parse().unwrap_or(0);
            let sym = c[1].trim_end_matches("USDT").trim_end_matches("USDC").trim_end_matches("USD").to_uppercase();
            let bid: f64 = c[4].parse().unwrap_or(0.0);
            let e = m.entry(sym).or_insert((0.0, 0));
            if ts >= e.1 { *e = (bid, ts); }
        }
    }
    m.into_iter().map(|(k, (b, _))| (k, b)).collect()
}

fn find_capreal(dir: &str) -> Option<String> {
    for p in [format!("{dir}/cap-real.json"), format!("{dir}/../cap-real.json"),
              "/opt/wallet-tracker/dataset/backfill/cap-real.json".to_string()] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

/// ризик групи у сценарії: сумарний продаж, зважена денна просадка, застрягле (без ліквідності)
struct GroupRisk { id: String, sell: f64, dd_daily_w: f64, stuck: f64, worst: Vec<Asset> }
/// (символ, продати$, глибина±2%$, просадка/день %)
struct Asset { sym: String, sell: f64, depth: f64, dd: f64 }

fn assess(g: &Value, bid: &HashMap<String, f64>, pct: f64, days: f64, min_usd: f64) -> GroupRisk {
    let id = g["id"].as_str().unwrap_or("?").to_string();
    let (mut sell_tot, mut dd_wsum, mut stuck) = (0.0f64, 0.0f64, 0.0f64);
    let mut worst: Vec<Asset> = vec![];
    if let Some(assets) = g["assets"].as_array() {
        for a in assets {
            let paper = a["paperUsd"].as_f64().unwrap_or(0.0);
            if paper < min_usd { continue }
            let reserve = a["reserveUsd"].as_f64().unwrap_or(0.0);
            let sym = a["symbol"].as_str().unwrap_or("").to_uppercase();
            let sell = pct / 100.0 * paper;
            let depth = wr_dict::dex_depth_at_ref(reserve)
                + if sym.is_empty() { 0.0 } else { bid.get(&sym).copied().unwrap_or(0.0) };
            let daily = sell / days;
            let dd = wr_dict::price_impact_pct(daily, depth);
            sell_tot += sell;
            if dd.is_infinite() { stuck += sell; }
            else { dd_wsum += dd * sell; }
            worst.push(Asset { sym, sell, depth, dd });
        }
    }
    worst.sort_by(|x, y| y.dd.partial_cmp(&x.dd).unwrap());
    let liquid = sell_tot - stuck;
    GroupRisk { id, sell: sell_tot, dd_daily_w: if liquid > 0.0 { dd_wsum / liquid } else { 0.0 }, stuck, worst }
}

impl Pattern for Scenario {
    fn name(&self) -> &str { "scenario" }
    fn about(&self) -> &str { "сценарій розпродажу: група роздає N% за M днів → просадка (Фаза 3.1)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let pct = num(args, "pct", 30.0);
        let days = num(args, "days", 7.0).max(1.0);
        let top = num(args, "top", 15.0) as usize;
        let min_usd = num(args, "min_usd", 100_000.0);
        let want = args.get("group").map(|s| s.as_str()).unwrap_or("all");

        let path = find_capreal(&ctx.dir).ok_or("нема cap-real.json (спершу capreal)")?;
        let cr: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let bid = cex_bid(&ctx.dir);
        let groups = cr["groups"].as_array().ok_or("cap-real.json: нема groups")?;

        println!("── СЦЕНАРІЙ РОЗПРОДАЖУ (Фаза 3.1, dict {}) ──", wr_dict::dict_version());
        println!("параметри: роздати {pct}% позицій за {days} днів · поріг позиції ${min_usd:.0}");
        println!("модель: просадка = 2%·(денний продаж / глибина±2%), лінійна НИЖНЯ МЕЖА; діапазон [оцінка … 2×]");

        if want == "all" {
            let mut risks: Vec<GroupRisk> = groups.iter().map(|g| assess(g, &bid, pct, days, min_usd)).collect();
            // ранг за денною просадкою ліквідної частини, тоді за застряглим
            risks.sort_by(|a, b| b.dd_daily_w.partial_cmp(&a.dd_daily_w).unwrap()
                .then(b.stuck.partial_cmp(&a.stuck).unwrap()));
            println!("\n{:<10} {:>13} {:>13} {:>16} {}", "група", "продати$", "просадка/день", "застрягло$(немає ліквідн.)", "гол.токени");
            for r in risks.iter().filter(|r| r.sell > 0.0).take(top) {
                let toks: Vec<String> = r.worst.iter().filter(|a| !a.sym.is_empty() && a.dd.is_finite())
                    .take(3).map(|a| format!("{}~{:.1}%", a.sym, a.dd)).collect();
                println!("{:<10} {:>13.0} {:>11.2}% {:>16.0} {}", r.id, r.sell, r.dd_daily_w, r.stuck, toks.join(" "));
            }
            println!("\nчитати: «просадка/день» — скільки впаде ціна за день такого темпу (≥, нижня межа).");
            println!("«застрягло» — позиції без ліквідності: вийти НЕМОЖЛИВО за поточних пулів/стаканів.");
        } else {
            let Some(g) = groups.iter().find(|g| g["id"].as_str() == Some(want)) else {
                return Err(format!("групу {want} не знайдено"));
            };
            let r = assess(g, &bid, pct, days, min_usd);
            println!("\n── ГРУПА {want}: роздає {pct}% за {days} днів ──");
            println!("сумарно продати: ${:.0} · денний тиск ${:.0}/день", r.sell, r.sell / days);
            println!("\n{:<10} {:>14} {:>14} {:>22}", "токен", "продати$", "глибина±2%$", "просадка/день [оцінка…2×]");
            for a in r.worst.iter().take(top) {
                let sym = if a.sym.is_empty() { "?" } else { &a.sym };
                if a.dd.is_infinite() {
                    println!("{:<10} {:>14.0} {:>14.0} {:>18}", sym, a.sell, a.depth, "НЕ ВИЙТИ (0 ліквідн.)");
                } else {
                    println!("{:<10} {:>14.0} {:>14.0} {:>12.1}% … {:.1}%", sym, a.sell, a.depth, a.dd, a.dd * 2.0);
                }
            }
            println!("\nСЕРЕДНЬОЗВАЖЕНА просадка/день (ліквідна частина): {:.2}% (≥)", r.dd_daily_w);
            if r.stuck > 0.0 {
                println!("⚠ ${:.0} позицій БЕЗ ЛІКВІДНОСТІ — вийти неможливо (нема пулів/стаканів)", r.stuck);
            }
        }
        Ok(())
    }
}
