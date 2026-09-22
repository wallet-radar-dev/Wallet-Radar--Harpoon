//! ПАТЕРН casctrack (фундамент стратегії КАСКАД): прогрес РОЗДАЧІ китів по кожному активу.
//! Читає cascade-curves.json (бін casccurve) — РЕКОНСТРУЙОВАНУ чисту позицію групи в ЧАСІ:
//!   наші 100% = ПІК net-холдингу китів; продано = пік − поточне; distributed% = (пік−поточне)/пік.
//! Це відрізняє РОЗДАЧУ від CHURN (totals не могли). Кити = члени капітал-груп МІНУС INFRA/ММ.
//! ~45-48% distributed = зона розвороту → ВИХІД; <10% = ще набирають → ЦІЛЬ РОТАЦІЇ.
//! ЧЕСНО: distributed% надійний (юніти скорочуються); emission% від кривої НЕ надійний (масштаб amount ≠ supply),
//! тож тут його не показуємо. priceNow — снапшот (стане актуальним із API).
//! Аргументи: exit=47  sort=dist|recent  top=30  min_events=5
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct CascTrack;

fn find(dir: &str, name: &str) -> Option<String> {
    for p in [format!("{dir}/{name}"), format!("{dir}/../{name}"),
              format!("/opt/wallet-tracker/dataset/backfill/{name}")] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

pub struct Curve { pub sym: String, pub token: String, pub dist: f64, pub price: f64,
    pub peak_date: String, pub last_date: String, pub n: u64 }

/// зчитати криві роздачі (спільне для casctrack і стратегії)
pub fn curves(ctx: &Ctx) -> Result<Vec<Curve>, String> {
    let p = find(&ctx.dir, "cascade-curves.json").ok_or("нема cascade-curves.json (спершу бін casccurve)")?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for t in v["tokens"].as_array().cloned().unwrap_or_default() {
        out.push(Curve {
            sym: t["symbol"].as_str().unwrap_or("").to_string(),
            token: t["token"].as_str().unwrap_or("").to_string(),
            dist: t["distributedPct"].as_f64().unwrap_or(0.0),
            price: t["priceNow"].as_f64().unwrap_or(0.0),
            peak_date: t["peakDate"].as_str().unwrap_or("").to_string(),
            last_date: t["lastDate"].as_str().unwrap_or("").to_string(),
            n: t["nEvents"].as_u64().unwrap_or(0),
        });
    }
    Ok(out)
}

pub fn status(dist: f64, exit: f64) -> &'static str {
    if dist < 10.0 { "НАБИРАЮТЬ" }
    else if dist < exit * 0.75 { "тримають" }
    else if dist < exit { "→ ЗОНА ВИХОДУ" }
    else if dist < 70.0 { "РОЗДАЮТЬ" }
    else { "ЗЛИЛИ" }
}

impl Pattern for CascTrack {
    fn name(&self) -> &str { "casctrack" }
    fn about(&self) -> &str { "прогрес роздачі китів у часі (net-крива; фундамент стратегії каскад)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let exit = num(args, "exit", 47.0);
        let top = num(args, "top", 30.0) as usize;
        let min_ev = num(args, "min_events", 5.0) as u64;
        let sort = args.get("sort").map(|s| s.as_str()).unwrap_or("dist");

        let mut rows = curves(ctx)?;
        rows.retain(|r| r.n >= min_ev);
        match sort {
            "recent" => rows.sort_by(|a, b| b.last_date.cmp(&a.last_date)),
            _ => rows.sort_by(|a, b| a.dist.partial_cmp(&b.dist).unwrap()), // від меншого distributed (кандидати нагорі)
        }
        println!("── КАСКАД: ПРОГРЕС РОЗДАЧІ КИТІВ (net-крива, поріг виходу {exit}%) ──");
        println!("активів: {} · кити = капітал-групи мінус INFRA/ММ · distributed = (пік−поточне)/пік", rows.len());
        println!("{:<9} {:>9} {:>13} {:>11} {:>11} {:>6} {}", "актив", "роздано%", "цінаНині", "пік_набору", "остан.рух", "подій", "стан");
        for r in rows.iter().take(top) {
            println!("{:<9} {:>8.1}% {:>13.5} {:>11} {:>11} {:>6} {}",
                r.sym, r.dist, r.price, r.peak_date, r.last_date, r.n, status(r.dist, exit));
        }
        println!("\nЧИТАТИ: «роздано%» = скільки % ПІКУ набору кити вже злили. <10%=ще набирають (ЦІЛЬ РОТАЦІЇ),");
        println!("40-{exit}%=зона розвороту (ВИХІД), >{exit}%=роздають/злили. Наступне — стратегія cascade (ротація).");
        Ok(())
    }
}
