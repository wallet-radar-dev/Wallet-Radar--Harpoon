//! ПАТЕРН report (Фаза 2.1 + X.4 плану): клієнтський звіт у ФАЙЛ — «продати без платформи».
//! Зводить координацію (coord-*.json) + викуп емісії + РЕАЛЬНУ ліквідність (orderbook.csv)
//! у markdown-звіт. Показує межі впевненості (X.4). Двомовний заголовок (X.4 — англ. основна).
//! Аргументи: chain=42161  top=15  lang=uk|en  out=<файл>
use std::collections::HashMap;
use std::io::Write;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::dict;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Report;

/// найсвіжіша ліквідність ±2% по символу з orderbook.csv (bid+ask usd)
fn liquidity(dir: &str) -> HashMap<String, (f64, f64, u64)> {
    let mut m: HashMap<String, (f64, f64, u64)> = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(format!("{dir}/orderbook.csv")) {
        for line in s.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 6 { continue }
            let ts: u64 = c[0].parse().unwrap_or(0);
            let sym = c[1].trim_end_matches("USDT").to_string();
            let bid: f64 = c[4].parse().unwrap_or(0.0);
            let ask: f64 = c[5].parse().unwrap_or(0.0);
            let e = m.entry(sym).or_insert((0.0, 0.0, 0));
            if ts >= e.2 { *e = (bid, ask, ts); } // найсвіжіший
        }
    }
    m
}

impl Pattern for Report {
    fn name(&self) -> &str { "report" }
    fn about(&self) -> &str { "клієнтський звіт у файл: координація + викуп емісії + реальна ліквідність" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let chain = args.get("chain").map(|s| s.as_str()).unwrap_or("42161");
        let top = num(args, "top", 15.0) as usize;
        let en = args.get("lang").map(|s| s == "en").unwrap_or(false);
        let out = args.get("out").cloned().unwrap_or_else(|| format!("{}/report-{}.md", ctx.dir, chain));

        let coord = std::fs::read_to_string(format!("{}/coord-{}.json", ctx.dir, chain))
            .map_err(|e| format!("нема coord-{chain}.json (спершу synchro): {e}"))?;
        let cl: Vec<Value> = serde_json::from_str(&coord).map_err(|e| e.to_string())?;
        let liq = liquidity(&ctx.dir);

        let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        let (t_title, t_intro, t_hdr, t_note) = if en {
            ("# Coordinated Accumulation Report",
             "Tokens where a synchronized cluster of wallets bought up the bulk of holders within a tight window (emission buy-up), cross-referenced with real on-chain/exchange liquidity.",
             "| token | wallets in cluster | of all holders | buy-up % | window (h) | real liquidity ±2% |",
             "Confidence: coordination is measured directly (synchronized first-acquisition). Liquidity is the latest ±2% order-book depth where a listing exists; absent = no major-exchange pair. Emission % is holder-share within the crawled set, a lower bound.")
        } else {
            ("# Звіт: координований викуп емісії",
             "Токени, де синхронний кластер гаманців викупив основну масу холдерів у вузькому вікні (викуп емісії), зіставлено з реальною ончейн/біржовою ліквідністю.",
             "| токен | у кластері | з усіх холдерів | викуп % | вікно (год) | реальна ліквідність ±2% |",
             "Межі впевненості: координація виміряна прямо (синхронне перше набуття). Ліквідність — найсвіжіша глибина стакана ±2%, де є лістинг; відсутня = нема пари на великій біржі. % емісії — частка холдерів у крауленому наборі, нижня оцінка.")
        };
        writeln!(f, "{t_title}\n\nchain {chain} · dict {} · {t_intro}\n", dict::dict_version()).map_err(|e| e.to_string())?;
        writeln!(f, "{t_hdr}\n|---|--:|--:|--:|--:|---|").map_err(|e| e.to_string())?;

        // топ за викупом емісії
        let mut rows: Vec<&Value> = cl.iter()
            .filter(|c| c.get("grab").and_then(Value::as_f64).unwrap_or(0.0) >= dict::grab_emission()
                     && c.get("cluster").and_then(Value::as_i64).unwrap_or(0) >= 10)
            .collect();
        rows.sort_by(|a, b| b.get("cluster").and_then(Value::as_i64).unwrap_or(0)
            .cmp(&a.get("cluster").and_then(Value::as_i64).unwrap_or(0)));
        for c in rows.iter().take(top) {
            let sym = c.get("sym").and_then(Value::as_str).unwrap_or("");
            let cluster = c.get("cluster").and_then(Value::as_i64).unwrap_or(0);
            let total = c.get("total_holders").and_then(Value::as_i64).unwrap_or(0);
            let grab = c.get("grab").and_then(Value::as_f64).unwrap_or(0.0);
            let span = c.get("span_h").and_then(Value::as_f64).unwrap_or(0.0);
            let l = liq.get(sym).map(|(b, a, _)| format!("${:.0} bid / ${:.0} ask", b, a))
                .unwrap_or_else(|| if en { "no major listing".into() } else { "нема лістингу".into() });
            writeln!(f, "| {sym} | {cluster} | {total} | {}% | {span:.1} | {l} |", (grab * 100.0) as i64)
                .map_err(|e| e.to_string())?;
        }
        writeln!(f, "\n> {t_note}").map_err(|e| e.to_string())?;
        // Фаза 0.3: журнал перерахунку вітрини
        crate::core::journal::record(&ctx.dir, "report", &out,
            &[&format!("{}/coord-{}.json", ctx.dir, chain), &format!("{}/orderbook.csv", ctx.dir)]);
        println!("[report] звіт -> {out} ({} рядків, {})", rows.len().min(top), if en {"en"} else {"uk"});
        Ok(())
    }
}
