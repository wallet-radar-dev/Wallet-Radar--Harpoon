//! ПАТЕРН mm (аналіз МАРКЕТ-МЕЙКЕРІВ): зводить mmfind.json — обсяги, патерни, частка емісії.
//! ММ визначено ОНЧЕЙН (mmfind): balance=|in−out|/(in+out)≈0 (проганяє обсяг, чиста позиція ~0),
//! score=(1−balance)·ln(ops). Відповідає на 3 питання: обсяги (turnoverUsd), патерни (netPct/opsPerDay),
//! частка емісії (обіг/supply + утримання/supply).
//! ФІЛЬТР за замовч.: відсіюємо СТЕЙБЛИ/загорнуті (словник) і КОНТРАКТИ-роутери (wallet-kind) —
//! лишаються чисті альт-ММ. stables=1 / contracts=1 щоб повернути. Рахуємо з ФІЛЬТРОВАНОГО списку.
//! ЧЕСНО: це ОНЧЕЙН-мейкери, НЕ біржові дески; CEX-стакани дають глибину, не обсяг.
//! Читає mmfind.json + wallet-kind.json + tokenpnl/token-supply. Аргументи: top=20 sort=turnover|share|emis min_usd=1e6
use std::collections::{HashMap, HashSet};
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Mm;

fn find(dir: &str, name: &str) -> Option<String> {
    for p in [format!("{dir}/{name}"), format!("{dir}/../{name}"),
              format!("/opt/wallet-tracker/dataset/backfill/{name}"), format!("/opt/wallet-tracker/dataset/{name}")] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

fn load_supply(dir: &str) -> HashMap<String, f64> {
    let mut s = HashMap::new();
    if let Some(p) = find(dir, "tokenpnl.jsonl") {
        if let Ok(f) = std::fs::File::open(&p) {
            use std::io::BufRead;
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(d) = serde_json::from_str::<Value>(&line) {
                    let c = d["contract"].as_str().unwrap_or("").to_lowercase();
                    if let Some(x) = d["supply"].as_f64() { if x > 0.0 && !c.is_empty() { s.entry(c).or_insert(x); } }
                }
            }
        }
    }
    if let Some(v) = find(dir, "token-supply.json").and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if let Some(o) = v.as_object() { for (k, x) in o { if let Some(y) = x["supply"].as_f64() { if y > 0.0 { s.entry(k.to_lowercase()).or_insert(y); } } } }
    }
    s
}

/// контракти (роутери/пули/агрегатори) з wallet-kind.json
fn load_contracts(dir: &str) -> HashSet<String> {
    find(dir, "wallet-kind.json").and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().map(|o| o.iter()
            .filter(|(_, k)| k.as_str() == Some("contract"))
            .map(|(a, _)| a.to_lowercase()).collect()))
        .unwrap_or_default()
}

/// повне членство: wallet -> group (group-membership.json від grouppart)
fn load_membership(dir: &str) -> HashMap<String, String> {
    find(dir, "group-membership.json").and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["wallets"].as_object().map(|o| o.iter()
            .filter_map(|(a, x)| x["group"].as_str().map(|g| (a.to_lowercase(), g.to_string()))).collect()))
        .unwrap_or_default()
}

/// відомий роутер/агрегатор/системна адреса за префіксом (запасний шлях до wallet-kind)
fn is_router(w: &str) -> bool {
    w.starts_with("0x1111111") || w.starts_with("0x0000000000") || w.starts_with("0x00000000")
        || w == wr_store::addrs::zero()
}

impl Pattern for Mm {
    fn name(&self) -> &str { "mm" }
    fn about(&self) -> &str { "аналіз маркет-мейкерів: обсяги/патерни/частка емісії (альт-ММ, без стейблів/роутерів)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let top = num(args, "top", 20.0) as usize;
        let min_usd = num(args, "min_usd", 1_000_000.0);
        let sort = args.get("sort").map(|s| s.as_str()).unwrap_or("turnover");
        let keep_stables = args.get("stables").map(|s| s == "1").unwrap_or(false);
        let keep_contracts = args.get("contracts").map(|s| s == "1").unwrap_or(false);

        let p = find(&ctx.dir, "mmfind.json").ok_or("нема mmfind.json (спершу backfill mmfind)")?;
        let d: Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let mm_all = d["mm"].as_array().cloned().unwrap_or_default();
        let share = d["share"].as_array().cloned().unwrap_or_default();
        let supply = load_supply(&ctx.dir);
        let contracts = load_contracts(&ctx.dir);

        // mmSharePct з mmfind по токену (для контексту «% потоку групи»)
        let sharepct: HashMap<String, f64> = share.iter()
            .filter_map(|s| Some((s["token"].as_str()?.to_lowercase(), s["mmSharePct"].as_f64()?))).collect();

        // ФІЛЬТР: альти (не стейбл/загорнутий) + EOA (не контракт/роутер)
        let (mut drop_stable, mut drop_contract) = (0usize, 0usize);
        let mm: Vec<&Value> = mm_all.iter().filter(|m| {
            let sym = m["symbol"].as_str().unwrap_or("");
            let w = m["wallet"].as_str().unwrap_or("").to_lowercase();
            if !keep_stables && wr_dict::is_stable_or_wrapped(sym) { drop_stable += 1; return false }
            if !keep_contracts && (contracts.contains(&w) || is_router(&w)) { drop_contract += 1; return false }
            true
        }).collect();

        // РЕЖИМ groups=1: звʼязок ММ ↔ капітал-групи (хто тримає своїх мейкерів)
        if args.get("groups").map(|s| s == "1").unwrap_or(false) {
            let mem = load_membership(&ctx.dir);
            if mem.is_empty() { return Err("нема group-membership.json (спершу grouppart)".into()) }
            struct G { w: HashSet<String>, usd: f64, toks: HashSet<String>, nets: Vec<f64> }
            let mut g: HashMap<String, G> = HashMap::new();
            let (mut matched, mut unmatched) = (0usize, 0usize);
            for m in &mm {
                let w = m["wallet"].as_str().unwrap_or("").to_lowercase();
                let Some(grp) = mem.get(&w) else { unmatched += 1; continue };
                matched += 1;
                let e = g.entry(grp.clone()).or_insert_with(|| G { w: HashSet::new(), usd: 0.0, toks: HashSet::new(), nets: vec![] });
                e.w.insert(w);
                e.usd += m["turnoverUsd"].as_f64().unwrap_or(0.0);
                e.toks.insert(m["symbol"].as_str().unwrap_or("").to_string());
                e.nets.push(m["netPct"].as_f64().unwrap_or(0.0));
            }
            let umm: HashSet<&str> = mm.iter().filter_map(|m| m["wallet"].as_str()).collect();
            println!("── ММ ↔ КАПІТАЛ-ГРУПИ (хто тримає своїх мейкерів) ──");
            println!("альт-ММ гаманців: {} · зіставлено з групою: {} · поза членством: {}", umm.len(), matched, unmatched);
            let mut rows: Vec<(&String, &G)> = g.iter().collect();
            rows.sort_by(|a, b| b.1.usd.partial_cmp(&a.1.usd).unwrap());
            println!("\n{:<12} {:>7} {:>15} {:>8} {:>10} {}", "група", "ММ-гам", "обіг ММ$", "токенів", "мед.netPct", "топ-токени");
            for (grp, gg) in rows.iter().take(top) {
                let mut nets = gg.nets.clone(); nets.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let med = if nets.is_empty() { 0.0 } else { nets[nets.len() / 2] };
                let mut toks: Vec<&String> = gg.toks.iter().collect(); toks.sort();
                let tt: Vec<String> = toks.iter().take(5).map(|s| s.to_string()).collect();
                println!("{:<12} {:>7} {:>15.0} {:>8} {:>9.1}% {}", grp, gg.w.len(), gg.usd, gg.toks.len(), med, tt.join(","));
            }
            println!("\nЧИТАТИ: група з багатьма ММ-гаманцями й великим обігом ТРИМАЄ ВЛАСНИЙ ФЛОТ МЕЙКЕРІВ —");
            println!("тобто сама робить ліквідність/обсяг у своїх токенах. мед.netPct≈0 = чистий churn (мейкінг).");
            println!("«поза членством» — ММ поза капітал-групами (незалежні дески/біржові мейкери).");
            return Ok(());
        }

        let total_turn: f64 = mm.iter().map(|m| m["turnoverUsd"].as_f64().unwrap_or(0.0)).sum();
        let wallets: HashSet<&str> = mm.iter().filter_map(|m| m["wallet"].as_str()).collect();
        let mut nets: Vec<f64> = mm.iter().filter_map(|m| m["netPct"].as_f64()).collect();
        let mut opd: Vec<f64> = mm.iter().filter_map(|m| m["opsPerDay"].as_f64()).collect();
        nets.sort_by(|a, b| a.partial_cmp(b).unwrap()); opd.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = |v: &[f64]| if v.is_empty() { 0.0 } else { v[v.len() / 2] };

        println!("── АНАЛІЗ МАРКЕТ-МЕЙКЕРІВ (ончейн, альт-ММ) ──");
        println!("фільтр: відсіяно {drop_stable} стейбл/загорнутих + {drop_contract} контракт/роутер записів (stables=1/contracts=1 повернути)");
        println!("альт-ММ гаманців: {} · пар: {} · сукупний ончейн-обіг: ${:.0}", wallets.len(), mm.len(), total_turn);
        println!("медіанний патерн: netPct {:.1}% (≈0 = чистий churn-мейкер) · {:.1} оп/день", med(&nets), med(&opd));

        // 1) ОБСЯГИ
        println!("\n── 1. ОБСЯГИ: топ альт-ММ гаманці за ончейн-обігом ──");
        let mut byturn = mm.clone();
        byturn.retain(|m| m["turnoverUsd"].as_f64().unwrap_or(0.0) >= min_usd);
        byturn.sort_by(|a, b| b["turnoverUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["turnoverUsd"].as_f64().unwrap_or(0.0)).unwrap());
        println!("{:<12} {:<8} {:>15} {:>8} {:>9} {:>7}", "гаманець", "токен", "обіг$", "netPct", "оп/день", "днів");
        for m in byturn.iter().take(top) {
            let w = m["wallet"].as_str().unwrap_or("");
            println!("{:<12} {:<8} {:>15.0} {:>7.1}% {:>9.1} {:>7}",
                &w[..12.min(w.len())], m["symbol"].as_str().unwrap_or(""), m["turnoverUsd"].as_f64().unwrap_or(0.0),
                m["netPct"].as_f64().unwrap_or(0.0), m["opsPerDay"].as_f64().unwrap_or(0.0), m["days"].as_i64().unwrap_or(0));
        }

        // 2) ПО ТОКЕНУ (рахуємо з фільтрованого списку)
        struct T { sym: String, w: HashSet<String>, usd: f64, turn_tok: f64, net_tok: f64, tok: String }
        let mut per: HashMap<String, T> = HashMap::new();
        for m in &mm {
            let tok = m["token"].as_str().unwrap_or("").to_lowercase();
            let e = per.entry(tok.clone()).or_insert_with(|| T { sym: m["symbol"].as_str().unwrap_or("").to_string(),
                w: HashSet::new(), usd: 0.0, turn_tok: 0.0, net_tok: 0.0, tok });
            e.w.insert(m["wallet"].as_str().unwrap_or("").to_string());
            e.usd += m["turnoverUsd"].as_f64().unwrap_or(0.0);
            e.turn_tok += m["turnover"].as_f64().unwrap_or(0.0);
            e.net_tok += m["in"].as_f64().unwrap_or(0.0) - m["out"].as_f64().unwrap_or(0.0);
        }
        let mut rows: Vec<(&T, f64, f64, f64)> = per.values().map(|t| {
            let sup = supply.get(&t.tok).copied().unwrap_or(0.0);
            let churn = if sup > 0.0 { 100.0 * t.turn_tok / sup } else { -1.0 };
            let hold = if sup > 0.0 { 100.0 * t.net_tok / sup } else { -1.0 };
            let flow = sharepct.get(&t.tok).copied().unwrap_or(-1.0);
            (t, churn, hold, flow)
        }).collect();
        match sort {
            "share" => rows.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap()),
            "emis" => rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap()),
            _ => rows.sort_by(|a, b| b.0.usd.partial_cmp(&a.0.usd).unwrap()),
        }
        println!("\n── 2. ЧАСТКА ЕМІСІЇ ПІД АЛЬТ-ММ (по токену) ──");
        println!("{:<10} {:>6} {:>14} {:>13} {:>13} {:>13}", "токен", "ММ-гам", "обіг ММ$", "% потоку", "обіг/емісія", "утрим./емісія");
        for (t, churn, hold, flow) in rows.iter().take(top) {
            let ch = if *churn < 0.0 { "?".into() } else { format!("{:.0}%", churn) };
            let ho = if *hold < 0.0 { "?".into() } else { format!("{:.2}%", hold) };
            let fl = if *flow < 0.0 { "?".into() } else { format!("{:.0}%", flow) };
            println!("{:<10} {:>6} {:>14.0} {:>13} {:>13} {:>13}", t.sym, t.w.len(), t.usd, fl, ch, ho);
        }

        println!("\nЧИТАТИ: «обіг ММ$» — USD прогнані через ММ (in+out); «% потоку» — частка активності групи від ММ (mmfind);");
        println!("«обіг/емісія» — скільки разів прокручено supply (>100% = churn); «утрим./емісія» — чисте ММ-сальдо як % емісії.");
        println!("ЗАСТЕРЕЖЕННЯ: ончейн-мейкери (не біржові дески); стейбли/загорнуті/контракти-роутери відсіяно; «емісія» лише де відома supply.");
        Ok(())
    }
}
