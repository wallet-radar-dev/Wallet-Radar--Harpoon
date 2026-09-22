//! ПАТЕРН riskdecomp (Фаза 3.4 плану L6): ДЕКОМПОЗИЦІЯ РИЗИКУ АКТИВУ.
//! «З чого саме складається ризик активу»: X% від концентрації в групі, Y% від тонкої
//! ліквідності, Z% публічний флоат. RiskScore високий лише коли є І концентрація, І ілліквідність
//! (концентровано, але глибокий ринок — вийти можна; тонко, але розпорошено — нема кому дампити).
//! Пояснюваність, якої не дає embedding-«чорна скринька» Oracul.
//! Читає cap-real.json + token-supply.json + orderbook.csv. Аргументи: asset=<SYM|all> top=20 min_usd=100000
use std::collections::HashMap;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct RiskDecomp;

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

fn find(dir: &str, name: &str) -> Option<String> {
    for p in [format!("{dir}/{name}"), format!("{dir}/../{name}"),
              format!("/opt/wallet-tracker/dataset/backfill/{name}"), format!("/opt/wallet-tracker/dataset/{name}")] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

struct Asset {
    sym: String, mcap: f64, grp_hold: f64, reserve: f64, depth: f64,
    groups: Vec<(String, f64)>, // (group, holding usd) desc
    conc: f64, conc_known: bool, illiq: f64, score: f64,
}

impl Pattern for RiskDecomp {
    fn name(&self) -> &str { "riskdecomp" }
    fn about(&self) -> &str { "декомпозиція ризику активу: група% / тонка ліквідність% / інше (Фаза 3.4)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let top = num(args, "top", 20.0) as usize;
        let min_usd = num(args, "min_usd", 100_000.0);
        let want = args.get("asset").map(|s| s.to_uppercase());

        let path = find(&ctx.dir, "cap-real.json").ok_or("нема cap-real.json (спершу capreal)")?;
        let cr: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let bid = cex_bid(&ctx.dir);
        // supply: contract-lc -> human supply. Джерела як у net6k: tokenpnl.jsonl (широке покриття)
        // + token-supply.json (добране). token-supply.json покриває лише ~133 токени, тож tokenpnl головне.
        let mut supply: HashMap<String, f64> = HashMap::new();
        if let Some(p) = find(&ctx.dir, "tokenpnl.jsonl") {
            if let Ok(f) = std::fs::File::open(&p) {
                use std::io::BufRead;
                for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                    if let Ok(d) = serde_json::from_str::<Value>(&line) {
                        let c = d["contract"].as_str().unwrap_or("").to_lowercase();
                        if let Some(s) = d["supply"].as_f64() { if s > 0.0 && !c.is_empty() { supply.entry(c).or_insert(s); } }
                    }
                }
            }
        }
        if let Some(v) = find(&ctx.dir, "token-supply.json").and_then(|p| std::fs::read_to_string(&p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            if let Some(o) = v.as_object() {
                for (k, x) in o { if let Some(s) = x["supply"].as_f64() { if s > 0.0 { supply.entry(k.to_lowercase()).or_insert(s); } } }
            }
        }
        let groups = cr["groups"].as_array().ok_or("cap-real.json: нема groups")?;

        // агрегуємо по токену: Σ hold, per-group, reserve, price(з paperUsd/amount)
        struct Agg { sym: String, contract: String, hold: f64, reserve: f64, price: f64, groups: Vec<(String, f64)> }
        let mut toks: HashMap<String, Agg> = HashMap::new();
        for g in groups {
            let gid = g["id"].as_str().unwrap_or("?").to_string();
            if let Some(assets) = g["assets"].as_array() {
                for a in assets {
                    let paper = a["paperUsd"].as_f64().unwrap_or(0.0);
                    if paper < min_usd { continue }
                    let c = a["token"].as_str().unwrap_or("").to_lowercase();
                    if c.is_empty() { continue }
                    let amount = a["amount"].as_f64().unwrap_or(0.0);
                    let price = if amount > 0.0 { paper / amount } else { 0.0 };
                    let e = toks.entry(c.clone()).or_insert_with(|| Agg {
                        sym: a["symbol"].as_str().unwrap_or("").to_uppercase(), contract: c,
                        hold: 0.0, reserve: a["reserveUsd"].as_f64().unwrap_or(0.0), price, groups: vec![] });
                    e.hold += paper;
                    e.reserve = e.reserve.max(a["reserveUsd"].as_f64().unwrap_or(0.0));
                    if e.price == 0.0 { e.price = price; }
                    e.groups.push((gid.clone(), paper));
                }
            }
        }

        let mut rows: Vec<Asset> = vec![];
        for (c, ag) in &toks {
            let mcap = supply.get(c).copied().unwrap_or(0.0) * ag.price;
            let depth = wr_dict::dex_depth_at_ref(ag.reserve) + if ag.sym.is_empty() { 0.0 } else { bid.get(&ag.sym).copied().unwrap_or(0.0) };
            // концентрація: частка mcap у координованих групах (0..1). Без supply — невідома (не вигадуємо).
            let conc_known = mcap > 0.0;
            let conc = if conc_known { (ag.hold / mcap).min(1.0) } else { 1.0 }; // для рангу невідому трактуємо консервативно
            // ілліквідність: яка частка концентрованого капіталу НЕ поглинається глибиною ±2%.
            let illiq = (1.0 - (depth / ag.hold).min(1.0)).max(0.0);
            let score = 100.0 * conc * illiq; // потрібні ОБИДВІ
            let mut gs = ag.groups.clone();
            gs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            rows.push(Asset { sym: if ag.sym.is_empty() { ag.contract[..8.min(ag.contract.len())].to_string() } else { ag.sym.clone() },
                mcap, grp_hold: ag.hold, reserve: ag.reserve, depth, groups: gs, conc, conc_known, illiq, score });
        }

        println!("── ДЕКОМПОЗИЦІЯ РИЗИКУ АКТИВУ (Фаза 3.4, dict {}) ──", wr_dict::dict_version());
        println!("RiskScore = концентрація × ілліквідність (0-100): високий лише коли є ОБИДВІ.\n");

        if let Some(w) = &want {
            let Some(a) = rows.iter().find(|a| &a.sym == w) else { return Err(format!("актив {w} не знайдено серед позицій груп")) };
            self.detail(a);
        } else {
            rows.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
            println!("{:<10} {:>6} {:>12} {:>7} {:>7} {}", "актив", "RISK", "капітал груп$", "конц%", "ілліквід%", "гол. джерело");
            for a in rows.iter().take(top) {
                let (tg, tv) = a.groups.first().cloned().unwrap_or(("?".into(), 0.0));
                let dom = if a.grp_hold > 0.0 { 100.0 * tv / a.grp_hold } else { 0.0 };
                let liqf = if a.depth < a.grp_hold * 0.1 { "ТОНКА" } else { "" };
                let concs = if a.conc_known { format!("{:.0}%", a.conc * 100.0) } else { "?".into() };
                println!("{:<10} {:>6.0} {:>12.0} {:>6} {:>7.0}% {} {:.0}% {}",
                    a.sym, a.score, a.grp_hold, concs, a.illiq * 100.0, tg, dom, liqf);
            }
            println!("\nчитати: конц% — скільки емісії в координованих групах; ілліквід% — частка холдингів,");
            println!("що НЕ поглинається глибиною ±2%. RISK високий = концентровано І нема куди продати.");
        }
        Ok(())
    }
}

impl RiskDecomp {
    fn detail(&self, a: &Asset) {
        println!("── {} · RiskScore {:.0}/100 ──", a.sym, a.score);
        println!("капіталізація: ${:.0} · капітал груп: ${:.0} · глибина±2%: ${:.0} (DEX-резерв ${:.0})",
            a.mcap, a.grp_hold, a.depth, a.reserve);
        println!("\nЗ ЧОГО СКЛАДАЄТЬСЯ РИЗИК:");
        // 1) концентрація по групах (частка mcap)
        if a.conc_known {
            let float = (a.mcap - a.grp_hold).max(0.0);
            println!("  КОНЦЕНТРАЦІЯ: {:.0}% емісії в координованих групах", a.conc * 100.0);
            for (g, v) in a.groups.iter().take(6) {
                println!("     ├ {:<10} ${:>13.0}  ({:.1}% емісії)", g, v, 100.0 * v / a.mcap);
            }
            println!("     └ публічний флоат ${:>13.0}  ({:.1}% емісії)", float, 100.0 * float / a.mcap);
        } else {
            println!("  КОНЦЕНТРАЦІЯ: невідома (нема supply у tokenpnl/token-supply) — % емісії не порахувати");
            for (g, v) in a.groups.iter().take(6) {
                println!("     ├ {:<10} ${:>13.0}", g, v);
            }
            println!("     (капітал груп ${:.0}; частку від емісії дасть tokprices/toksupply на цей токен)", a.grp_hold);
        }
        // 2) ліквідність
        let cover = if a.grp_hold > 0.0 { 100.0 * (a.depth / a.grp_hold).min(1.0) } else { 0.0 };
        println!("  ЛІКВІДНІСТЬ: глибина±2% покриває лише {:.1}% капіталу груп → ілліквідність {:.0}%", cover, a.illiq * 100.0);
        // 3) вердикт
        let (dg, dv) = a.groups.first().cloned().unwrap_or(("?".into(), 0.0));
        if a.conc_known {
            println!("\nВЕРДИКТ: головний ризик — група {dg} ({:.0}% емісії, ${:.0})", 100.0 * dv / a.mcap, dv);
        } else {
            println!("\nВЕРДИКТ: головний ризик — група {dg} (${:.0}; частка емісії невідома)", dv);
        }
        if a.illiq > 0.5 { println!("         + ТОНКА ЛІКВІДНІСТЬ (вийти майже нікуди): підсилює будь-який продаж"); }
        if a.conc < 0.1 { println!("         (концентрація низька — навіть при тонкій ліквідності системного дампу нема кому робити)"); }
    }
}
