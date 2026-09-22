//! ПАТЕРН cascade (СТРАТЕГІЯ КАСКАД): ранжування портфеля за прогресом роздачі китів.
//! Правила:
//!   ВИХІД     — distributed ≥ exit% (кити вже виходять, розворот близько) АБО ціна падає (фіксуємо);
//!   РОТАЦІЯ-В — distributed < accum% (кити ще набирають/тримають) + не падає + МЕНША кап (більше простору);
//!   ТРИМАТИ   — решта.
//! Каскад = переливати з ВИХІД у топ РОТАЦІЯ-В (де кити ще не розвантажились).
//! distributed% — з net-кривої (casccurve, надійний). Тренд — 7-дн зміна з token-prices-hourly (де є), інакше «?».
//! ЧЕСНО: снапшот-ціна + частковий тренд; повний «росте/падає» — з живим ціновим фідом (API-ключі).
//! Читає cascade-curves.json + token-prices-hourly.json. Аргументи: exit=47 accum=12 fall=-8 trend_days=7 top=15
use std::collections::HashMap;
use std::io::Write;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Cascade;

fn act_code(a: &str) -> &'static str {
    match a { "ВИХІД" => "exit", "РОТАЦІЯ-В" => "rotate_in", "ТРИМАТИ" => "hold", _ => "?" }
}

fn find(dir: &str, name: &str) -> Option<String> {
    for p in [format!("{dir}/{name}"), format!("{dir}/../{name}"),
              format!("/opt/wallet-tracker/dataset/backfill/{name}")] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

/// token(lc) -> N-дн % зміна ціни (DefiLlama hourly + GeckoTerminal ончейн для дрібняку)
fn trends(dir: &str, days: f64) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for file in ["token-prices-hourly.json", "token-prices-onchain.json"] {
        if let Some(v) = find(dir, file).and_then(|p| std::fs::read_to_string(&p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            if let Some(tk) = v["tokens"].as_object() {
                for (c, x) in tk {
                    let key = c.to_lowercase();
                    if m.contains_key(&key) { continue } // DefiLlama пріоритет для мажорів
                    let ps: Vec<(i64, f64)> = x["prices"].as_array().map(|a| a.iter()
                        .filter_map(|p| { let p = p.as_array()?; Some((p[0].as_i64()?, p[1].as_f64()?)) }).collect()).unwrap_or_default();
                    if ps.len() < 2 { continue }
                    let last = ps.iter().max_by_key(|(t, _)| *t).unwrap(); // newest (незалежно від порядку)
                    let target = last.0 - (days * 86400.0) as i64;
                    let base = ps.iter().min_by_key(|(t, _)| (t - target).abs()).unwrap();
                    if base.1 > 0.0 { m.insert(key, 100.0 * (last.1 / base.1 - 1.0)); }
                }
            }
        }
    }
    m
}

impl Pattern for Cascade {
    fn name(&self) -> &str { "cascade" }
    fn about(&self) -> &str { "стратегія каскад: ВИХІД / РОТАЦІЯ-В / ТРИМАТИ за прогресом роздачі китів" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let exit = num(args, "exit", 47.0);
        let accum = num(args, "accum", 12.0);
        let fall = num(args, "fall", -8.0);   // поріг падіння для фіксації
        let days = num(args, "trend_days", 7.0);
        let top = num(args, "top", 15.0) as usize;

        let p = find(&ctx.dir, "cascade-curves.json").ok_or("нема cascade-curves.json (спершу casccurve)")?;
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let tr = trends(&ctx.dir, days);

        // (sym, token, dist, trend(opt), mcap, action, score)
        struct A { sym: String, dist: f64, trend: Option<f64>, mcap: f64, action: &'static str, score: f64, reason: String }
        let mut items: Vec<A> = vec![];
        for t in v["tokens"].as_array().cloned().unwrap_or_default() {
            let dist = t["distributedPct"].as_f64().unwrap_or(0.0);
            let sym = t["symbol"].as_str().unwrap_or("").to_string();
            let tok = t["token"].as_str().unwrap_or("").to_lowercase();
            let price = t["priceNow"].as_f64().unwrap_or(0.0);
            let supply = t["supply"].as_f64().unwrap_or(0.0);
            let mcap = price * supply;
            let n = t["nEvents"].as_u64().unwrap_or(0);
            if n < 10 { continue }
            let trend = tr.get(&tok).copied();
            let falling = trend.map(|x| x <= fall).unwrap_or(false);
            // рішення
            let (action, reason, score): (&str, String, f64) = if dist >= exit {
                ("ВИХІД", format!("роздано {:.0}% ≥ {exit}% (кити виходять)", dist), dist)
            } else if falling {
                ("ВИХІД", format!("падає {:.0}% і роздача почалась ({:.0}%)", trend.unwrap(), dist), 100.0 + trend.unwrap().abs())
            } else if dist < accum {
                // РОТАЦІЯ-В: чим менше роздано + чим сильніше росте + чим менша кап
                let tfac = trend.unwrap_or(0.0).max(-5.0);
                let capbon = if mcap > 0.0 { (12.0 - mcap.log10()).max(0.0) } else { 3.0 }; // менша кап = більше балів
                let sc = (accum - dist) + tfac * 0.4 + capbon * 2.0;
                ("РОТАЦІЯ-В", format!("роздано лише {:.0}%, кити тримають", dist), sc)
            } else {
                ("ТРИМАТИ", format!("роздано {:.0}%", dist), dist)
            };
            items.push(A { sym, dist, trend, mcap, action, score, reason });
        }

        // save=1: знімок стратегії в історію (для алертів на перетин порогу)
        if args.get("save").map(|s| s == "1").unwrap_or(false) {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let out = format!("{}/cascade-history.csv", ctx.dir);
            let new = !std::path::Path::new(&out).exists();
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&out) {
                if new { let _ = writeln!(f, "ts,symbol,dist,trend,action"); }
                for a in &items {
                    let tr = a.trend.map(|x| format!("{:.1}", x)).unwrap_or_default();
                    let _ = writeln!(f, "{ts},{},{:.2},{tr},{}", a.sym, a.dist, act_code(a.action));
                }
            }
            crate::core::journal::record(&ctx.dir, "cascade", &out, &[&format!("{}/cascade-curves.json", ctx.dir)]);
            println!("[cascade] знімок дописано -> {out} ({} активів)", items.len());
        }

        let show = |a: &A| {
            let tr = a.trend.map(|x| format!("{:+.0}%", x)).unwrap_or_else(|| "?".into());
            let mc = if a.mcap > 0.0 { format!("${:.0}", a.mcap) } else { "?".into() };
            println!("  {:<9} роздано {:>5.1}% · тренд7д {:>5} · кап {:>12} · {}", a.sym, a.dist, tr, mc, a.reason);
        };

        println!("── СТРАТЕГІЯ КАСКАД (поріг виходу {exit}%, набору {accum}%, тренд {days}дн) ──");
        println!("активів у покритті: {} · тренд є для {} (hourly)\n", items.len(), tr.len());

        println!("🔴 ВИХІД (фіксувати / не входити):");
        let mut ex: Vec<&A> = items.iter().filter(|a| a.action == "ВИХІД").collect();
        ex.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        for a in ex.iter().take(top) { show(a); }

        println!("\n🟢 РОТАЦІЯ-В (кандидати переливу, ранг за привабливістю):");
        let mut rin: Vec<&A> = items.iter().filter(|a| a.action == "РОТАЦІЯ-В").collect();
        rin.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        for a in rin.iter().take(top) { show(a); }

        println!("\n⚪ ТРИМАТИ (у процесі роздачі, стежити):");
        let mut hold: Vec<&A> = items.iter().filter(|a| a.action == "ТРИМАТИ").collect();
        hold.sort_by(|a, b| b.dist.partial_cmp(&a.dist).unwrap());
        for a in hold.iter().take(top.min(8)) { show(a); }

        println!("\nКАСКАД: переливати ліквідність з 🔴 у топ 🟢 (кити ще не розвантажились, менша кап = простір).");
        println!("ЧЕСНО: тренд «?» = нема hourly-ціни (дрібняк); з живим фідом «росте/падає» стане повним для всіх.");
        Ok(())
    }
}
