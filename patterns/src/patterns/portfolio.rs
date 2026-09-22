//! ПАТЕРН portfolio (Фаза 3.3 плану L6): РИЗИК ПОРТФЕЛЯ КЛІЄНТА.
//! Клієнт вводить свій склад → бачить приховану залежність: які НЕПУБЛІЧНІ групи капіталу
//! сидять у тих самих активах, наскільки концентровано, і що станеться з його позиціями,
//! якщо ці групи почнуть роздачу. Перехід від «дивись на китів» до «ось ризик у ТВОЄМУ портфелі».
//! Читає cap-real.json + orderbook.csv; нічого не мутує.
//! Вхід: holds=BLUR:100000,LINK:50000,ONDO:80000   АБО file=<шлях> (рядки `SYMBOL usd`).
use std::collections::HashMap;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Portfolio;

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

/// портфель клієнта: SYMBOL(uppercase) -> USD
fn parse_holdings(args: &std::collections::BTreeMap<String, String>) -> HashMap<String, f64> {
    let mut h = HashMap::new();
    if let Some(f) = args.get("file") {
        if let Ok(s) = std::fs::read_to_string(f) {
            for line in s.lines() {
                let mut it = line.split_whitespace();
                if let (Some(sym), Some(usd)) = (it.next(), it.next()) {
                    if let Ok(u) = usd.replace(['$', ','], "").parse::<f64>() { h.insert(sym.to_uppercase(), u); }
                }
            }
        }
    }
    if let Some(inl) = args.get("holds") {
        for part in inl.split(',') {
            let mut kv = part.split(':');
            if let (Some(sym), Some(usd)) = (kv.next(), kv.next()) {
                if let Ok(u) = usd.parse::<f64>() { h.insert(sym.trim().to_uppercase(), u); }
            }
        }
    }
    h
}

impl Pattern for Portfolio {
    fn name(&self) -> &str { "portfolio" }
    fn about(&self) -> &str { "ризик портфеля клієнта: приховані групи у ваших активах (Фаза 3.3)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let pct = num(args, "pct", 30.0);   // сценарій: групи роздають pct% при виході
        let mut holds = parse_holdings(args);
        if holds.is_empty() {
            // демо-портфель із токенів, де є групи (щоб результат був змістовний)
            for (s, u) in [("BLUR", 100_000.0), ("LINK", 80_000.0), ("ONDO", 120_000.0), ("MKR", 50_000.0), ("SHIB", 60_000.0)] {
                holds.insert(s.into(), u);
            }
            println!("(портфель не задано — демо; використай holds=SYM:usd,... або file=<шлях>)");
        }

        let path = find_capreal(&ctx.dir).ok_or("нема cap-real.json (спершу capreal)")?;
        let cr: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let bid = cex_bid(&ctx.dir);
        let groups = cr["groups"].as_array().ok_or("cap-real.json: нема groups")?;

        // по кожному символу: групи, що тримають (group -> usd), резерв
        let mut per_sym: HashMap<String, (f64, Vec<(String, f64)>)> = HashMap::new(); // sym -> (reserve, [(group,usd)])
        for g in groups {
            let gid = g["id"].as_str().unwrap_or("?").to_string();
            if let Some(assets) = g["assets"].as_array() {
                for a in assets {
                    let sym = a["symbol"].as_str().unwrap_or("").to_uppercase();
                    if sym.is_empty() || !holds.contains_key(&sym) { continue }
                    let paper = a["paperUsd"].as_f64().unwrap_or(0.0);
                    let reserve = a["reserveUsd"].as_f64().unwrap_or(0.0);
                    let e = per_sym.entry(sym).or_insert((reserve, vec![]));
                    e.0 = e.0.max(reserve);
                    e.1.push((gid.clone(), paper));
                }
            }
        }

        let total: f64 = holds.values().sum();
        println!("── РИЗИК ПОРТФЕЛЯ (Фаза 3.3, dict {}) ──", wr_dict::dict_version());
        println!("портфель: ${:.0} у {} активах\n", total, holds.len());

        // 1) по позиціях: хто ще сидить + ваш ризик виходу
        println!("{:<8} {:>11} {:>10} {:>26} {}", "актив", "ваше$", "вихід/день", "гол. непублічна група $", "груп");
        let mut group_exposure: HashMap<String, (f64, f64, u32)> = HashMap::new(); // group -> (their_usd_in_your_assets, your_usd_touched, n_positions)
        let mut exposed_val = 0.0;
        let mut holds_sorted: Vec<(&String, &f64)> = holds.iter().collect();
        holds_sorted.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
        for (sym, &you) in holds_sorted {
            match per_sym.get(sym) {
                Some((reserve, gv)) => {
                    let mut gs = gv.clone();
                    gs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                    let depth = wr_dict::dex_depth_at_ref(*reserve) + bid.get(sym).copied().unwrap_or(0.0);
                    let dd = wr_dict::price_impact_pct(you, depth);
                    let dds = if dd.is_infinite() { "НЕ ВИЙТИ".into() } else { format!("{:.1}%", dd) };
                    let (tg, tv) = &gs[0];
                    println!("{:<8} {:>11.0} {:>10} {:>18} ${:>6.0} {:>4}", sym, you, dds, tg, tv, gs.len());
                    exposed_val += you;
                    for (g, v) in &gs {
                        let e = group_exposure.entry(g.clone()).or_insert((0.0, 0.0, 0));
                        e.0 += v; e.1 += you; e.2 += 1;
                    }
                }
                None => println!("{:<8} {:>11.0} {:>10} {:>26} {}", sym, you, "—", "(жодна група не тримає)", 0),
            }
        }

        // 2) ПРИХОВАНА ЗАЛЕЖНІСТЬ: групи, від яких залежить найбільша частина портфеля
        let mut exp: Vec<(String, f64, f64, u32)> = group_exposure.into_iter()
            .map(|(g, (their, yours, n))| (g, yours, their, n)).collect();
        exp.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        println!("\n── ПРИХОВАНА ЗАЛЕЖНІСТЬ ВІД ГРУП ──");
        println!("(якщо ця група почне роздачу — падають ОДРАЗУ всі ваші позиції в спільних активах)");
        println!("{:<10} {:>16} {:>10} {:>16}", "група", "ваш$ під ризиком", "частка", "їхній$ у ваших активах");
        for (g, yours, their, n) in exp.iter().take(10) {
            println!("{:<10} {:>16.0} {:>9.0}% {:>16.0}  ({} поз.)", g, yours, 100.0 * yours / total, their, n);
        }

        // 3) підсумок концентрації
        let covered = 100.0 * exposed_val / total;
        println!("\nПІДСУМОК: {:.0}% портфеля (${:.0}) — в активах, де сидять непублічні групи капіталу.", covered, exposed_val);
        if let Some((g, yours, _, _)) = exp.first() {
            println!("Найбільша прихована залежність: група {g} стоїть за {:.0}% вашого портфеля.", 100.0 * yours / total);
        }
        println!("(сценарій виходу рахований на поточній ліквідності; «вихід/день» — нижня межа просадки при продажу всієї позиції за день)");
        let _ = pct;
        Ok(())
    }
}
