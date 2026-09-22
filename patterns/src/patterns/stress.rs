//! ПАТЕРН stress (Фаза 3.2 плану L6): СТРЕС-ТЕСТ ринку.
//! «Ринок падає на shock% → тримачі активу разом кидаються продавати panic% → який КАСКАД?»
//! Під шоком ліквідність тоншає (пули/стакани стискаються з ціною), а концентрований
//! груповий продаж додає другий удар: total_drop = shock + price_impact(груповий_продаж, тонша_глибина).
//! Показує (а) які активи постраждають найбільше, (б) які групи найвразливіші (капітал у каскадних активах).
//! Читає cap-real.json + orderbook.csv; нічого не мутує. Аргументи: shock=40 panic=50 top=15 min_usd=100000
use std::collections::{HashMap, HashSet};
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Stress;

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

struct Tok { sym: String, reserve: f64, hold: f64, groups: HashSet<String> }

impl Pattern for Stress {
    fn name(&self) -> &str { "stress" }
    fn about(&self) -> &str { "стрес-тест ринку: шок −X% → каскад продажів → хто постраждає (Фаза 3.2)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let shock = num(args, "shock", 40.0) / 100.0;    // падіння ринку
        let panic = num(args, "panic", 50.0) / 100.0;    // частка холдингів, яку кидаються продавати
        let top = num(args, "top", 15.0) as usize;
        let min_usd = num(args, "min_usd", 100_000.0);
        let pf = 1.0 - shock;                            // ціновий множник після шоку

        let path = find_capreal(&ctx.dir).ok_or("нема cap-real.json (спершу capreal)")?;
        let cr: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let bid = cex_bid(&ctx.dir);
        let groups = cr["groups"].as_array().ok_or("cap-real.json: нема groups")?;

        // агрегуємо холдинги ПО АКТИВУ через усі групи (реальний резерв однаковий на токен)
        let mut toks: HashMap<String, Tok> = HashMap::new();
        for g in groups {
            let gid = g["id"].as_str().unwrap_or("?").to_string();
            if let Some(assets) = g["assets"].as_array() {
                for a in assets {
                    let paper = a["paperUsd"].as_f64().unwrap_or(0.0);
                    if paper < min_usd { continue }
                    let c = a["token"].as_str().unwrap_or("").to_string();
                    if c.is_empty() { continue }
                    let e = toks.entry(c).or_insert_with(|| Tok {
                        sym: a["symbol"].as_str().unwrap_or("").to_uppercase(),
                        reserve: a["reserveUsd"].as_f64().unwrap_or(0.0), hold: 0.0, groups: HashSet::new() });
                    e.hold += paper;
                    e.groups.insert(gid.clone());
                }
            }
        }

        // каскад по кожному активу: total_drop = shock + вплив групового продажу в тоншу глибину
        // (sym, total_drop%, cascade%, hold_post, n_groups, depth_post)
        let mut rows: Vec<(String, f64, f64, f64, usize, f64)> = vec![];
        let mut asset_cascade: HashMap<String, f64> = HashMap::new(); // contract -> cascade%
        for (c, t) in &toks {
            let hold_post = t.hold * pf;
            let sell = panic * hold_post;
            let depth0 = wr_dict::dex_depth_at_ref(t.reserve)
                + if t.sym.is_empty() { 0.0 } else { bid.get(&t.sym).copied().unwrap_or(0.0) };
            let depth_post = depth0 * pf;                      // ліквідність тоншає з ціною
            let cascade = wr_dict::price_impact_pct(sell, depth_post);
            let total_drop = shock * 100.0 + cascade;          // % (початковий шок + каскад)
            asset_cascade.insert(c.clone(), cascade);
            rows.push((if t.sym.is_empty() { c[..8.min(c.len())].to_string() } else { t.sym.clone() },
                       total_drop, cascade, hold_post, t.groups.len(), depth_post));
        }
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        println!("── СТРЕС-ТЕСТ РИНКУ (Фаза 3.2, dict {}) ──", wr_dict::dict_version());
        println!("шок ринку: −{:.0}% · тримачі кидаються продати {:.0}% холдингів · ліквідність тоншає з ціною",
            shock * 100.0, panic * 100.0);
        println!("total_drop = шок + каскад(груповий продаж у тоншу глибину), нижня межа\n");
        println!("{:<10} {:>10} {:>10} {:>14} {:>7} {}", "актив", "падіння%", "каскад%", "холдинг$пост", "груп", "глибина$пост");
        for (sym, drop, casc, hold, ng, depth) in rows.iter().take(top) {
            let d = if casc.is_infinite() { ">100 (не вийти)".to_string() } else { format!("{:.1}", drop.min(99.9)) };
            let cc = if casc.is_infinite() { "∞".to_string() } else { format!("{:.1}", casc) };
            println!("{:<10} {:>10} {:>10} {:>14.0} {:>7} {:>12.0}", sym, d, cc, hold, ng, depth);
        }

        // найвразливіші ГРУПИ: скільки капіталу сидить у каскадних активах (drop, окрім самого шоку)
        let mut gexp: Vec<(String, f64, f64)> = vec![]; // id, exposed_usd (у каскадних), total_post
        for g in groups {
            let gid = g["id"].as_str().unwrap_or("?").to_string();
            let (mut exp, mut tot) = (0.0f64, 0.0f64);
            if let Some(assets) = g["assets"].as_array() {
                for a in assets {
                    let paper = a["paperUsd"].as_f64().unwrap_or(0.0) * pf;
                    if paper < min_usd * pf { continue }
                    tot += paper;
                    let c = a["token"].as_str().unwrap_or("");
                    let casc = asset_cascade.get(c).copied().unwrap_or(0.0);
                    if casc.is_infinite() || casc > 10.0 { exp += paper; } // каскад >10% або взагалі не вийти
                }
            }
            if tot > 0.0 { gexp.push((gid, exp, tot)); }
        }
        gexp.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        println!("\n── найвразливіші групи (капітал у каскадних/неліквідних активах) ──");
        println!("{:<10} {:>16} {:>14} {:>8}", "група", "вразливо$(>10%каск)", "усього$пост", "частка");
        for (id, exp, tot) in gexp.iter().filter(|x| x.1 > 0.0).take(top) {
            println!("{:<10} {:>16.0} {:>14.0} {:>7.0}%", id, exp, tot, 100.0 * exp / tot);
        }
        Ok(())
    }
}
