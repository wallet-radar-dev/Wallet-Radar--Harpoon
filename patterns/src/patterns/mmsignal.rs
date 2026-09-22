//! ПАТЕРН mmsignal: ЗВІДНИЙ СИГНАЛ — збіг напрямку ММ в ОБОХ контурах (ончейн + CEX).
//! Ідея: коли група тихо НАБИРАЄ і в ланцюгу, і на біржі одночасно — це сильніший сигнал, ніж окремо.
//!   ончейн-нетто = Σ(in−out) мейкерів групи / їх обіг  (з mmfind);
//!   CEX-нетто     = Σ(taker_buy − sell) / обсяг         (з tradetape);
//!   вага          = частка ончейн-потоку від ММ групи   (mmfind mmSharePct).
//! ЗБІГ: обидва >0 = НАКОПИЧЕННЯ (верх рангу), обидва <0 = РОЗДАЧА (низ), різні знаки = розбіжність.
//! Читає mmfind.json + tradetape.csv. Аргументи: top=25 min_vol=50000
use std::collections::HashMap;
use std::io::Write;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct MmSignal;

/// стабільний код вердикту для CSV/дифів
fn code(v: &str) -> &'static str {
    match v {
        "НАБИРАЄ обидва" => "accum",
        "РОЗДАЄ обидва" => "distrib",
        "розбіжність" => "diverge",
        "лише CEX+" => "cex_buy",
        "лише CEX−" => "cex_sell",
        _ => "none",
    }
}

fn find(dir: &str, name: &str) -> Option<String> {
    for p in [format!("{dir}/{name}"), format!("{dir}/../{name}"),
              format!("/opt/wallet-tracker/dataset/backfill/{name}")] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}
fn strip(sym: &str) -> String { sym.trim_end_matches("USDT").trim_end_matches("USDC").trim_end_matches("USD").to_uppercase() }

impl Pattern for MmSignal {
    fn name(&self) -> &str { "mmsignal" }
    fn about(&self) -> &str { "звідний сигнал: збіг ончейн+CEX напрямку ММ (де група набирає/роздає в обох)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let top = num(args, "top", 25.0) as usize;
        let min_vol = num(args, "min_vol", 50_000.0);

        // ── ОНЧЕЙН з mmfind: по СИМВОЛУ net, turnover, mmSharePct ──
        let p = find(&ctx.dir, "mmfind.json").ok_or("нема mmfind.json (спершу mmfind)")?;
        let d: Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let mut on_net: HashMap<String, f64> = HashMap::new();   // sym -> Σ(in-out) токенів
        let mut on_turn: HashMap<String, f64> = HashMap::new();  // sym -> Σ turnover токенів
        for m in d["mm"].as_array().cloned().unwrap_or_default() {
            let sym = m["symbol"].as_str().unwrap_or("").to_uppercase();
            if sym.is_empty() { continue }
            *on_net.entry(sym.clone()).or_insert(0.0) += m["in"].as_f64().unwrap_or(0.0) - m["out"].as_f64().unwrap_or(0.0);
            *on_turn.entry(sym).or_insert(0.0) += m["turnover"].as_f64().unwrap_or(0.0);
        }
        let mm_share: HashMap<String, f64> = d["share"].as_array().cloned().unwrap_or_default().iter()
            .filter_map(|s| Some((s["symbol"].as_str()?.to_uppercase(), s["mmSharePct"].as_f64()?))).collect();

        // ── CEX з tradetape: по СИМВОЛУ net-flow, vol ──
        let tp = find(&ctx.dir, "tradetape.csv").ok_or("нема tradetape.csv (спершу бін tradetape)")?;
        let txt = std::fs::read_to_string(&tp).map_err(|e| e.to_string())?;
        let mut cex_net: HashMap<String, f64> = HashMap::new();
        let mut cex_vol: HashMap<String, f64> = HashMap::new();
        for line in txt.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 6 { continue }
            let sym = strip(c[1]);
            *cex_net.entry(sym.clone()).or_insert(0.0) += c[4].parse::<f64>().unwrap_or(0.0) - c[5].parse::<f64>().unwrap_or(0.0);
            *cex_vol.entry(sym).or_insert(0.0) += c[3].parse::<f64>().unwrap_or(0.0);
        }

        // ── ЗВЕДЕННЯ по символах, що є в CEX ──
        // (sym, on_pct, cex_pct, mmshare, vol, score, verdict)
        let mut rows: Vec<(String, f64, f64, f64, f64, f64, &str)> = vec![];
        for (sym, &vol) in &cex_vol {
            if vol < min_vol { continue }
            // ончейн-символ: пробуємо sym і W+sym (WBTC/WETH для BTC/ETH)
            let key = if on_turn.contains_key(sym) { Some(sym.clone()) }
                      else if on_turn.contains_key(&format!("W{sym}")) { Some(format!("W{sym}")) } else { None };
            let (on_pct, share) = match &key {
                Some(k) => {
                    let t = on_turn.get(k).copied().unwrap_or(0.0);
                    let op = if t > 0.0 { 100.0 * on_net.get(k).copied().unwrap_or(0.0) / t } else { 0.0 };
                    (op, mm_share.get(k).copied().unwrap_or(0.0))
                }
                None => (f64::NAN, -1.0),
            };
            let cex_pct = 100.0 * cex_net.get(sym).copied().unwrap_or(0.0) / vol;
            // score: збіг знаків, зважений на частку ончейн-ММ
            let (score, verdict) = if on_pct.is_nan() {
                (cex_pct * 0.5, if cex_pct > 0.0 { "лише CEX+" } else { "лише CEX−" })
            } else {
                let w = (share.max(0.0) / 100.0).max(0.2); // вага (мін 0.2, щоб не занулити без share)
                if on_pct > 0.0 && cex_pct > 0.0 { ((on_pct + cex_pct) * w, "НАБИРАЄ обидва") }
                else if on_pct < 0.0 && cex_pct < 0.0 { ((on_pct + cex_pct) * w, "РОЗДАЄ обидва") }
                else { (0.0, "розбіжність") }
            };
            rows.push((sym.clone(), on_pct, cex_pct, share, vol, score, verdict));
        }
        rows.sort_by(|a, b| b.5.partial_cmp(&a.5).unwrap());

        // save=1: дописати знімок сигналу в історію (для алертів на зміну вердикту)
        if args.get("save").map(|s| s == "1").unwrap_or(false) {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs()).unwrap_or(0);
            let out = format!("{}/mmsignal-history.csv", ctx.dir);
            let new = !std::path::Path::new(&out).exists();
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&out) {
                if new { let _ = writeln!(f, "ts,symbol,on_pct,cex_pct,mmshare,cex_vol,score,verdict"); }
                for v in &rows {
                    let on = if v.1.is_nan() { String::new() } else { format!("{:.2}", v.1) };
                    let _ = writeln!(f, "{ts},{},{on},{:.2},{:.1},{:.0},{:.2},{}", v.0, v.2, v.3.max(0.0), v.4, v.5, code(v.6));
                }
            }
            crate::core::journal::record(&ctx.dir, "mmsignal", &out,
                &[&format!("{}/mmfind.json", ctx.dir), &format!("{}/tradetape.csv", ctx.dir)]);
            println!("[mmsignal] знімок сигналу дописано -> {out} ({} токенів, ts={ts})", rows.len());
        }

        println!("── ЗВІДНИЙ СИГНАЛ ММ: ОНЧЕЙН + CEX ЗБІГ ──");
        println!("верх = НАБИРАЄ в обох контурах · низ = РОЗДАЄ в обох · нуль = розбіжність/лише один контур\n");
        println!("{:<7} {:>13} {:>11} {:>8} {:>13} {:>8} {}", "токен", "ончейн-нетто%", "CEX-нетто%", "ончММ%", "CEX-обсяг$", "score", "вердикт");
        let show = |v: &(String, f64, f64, f64, f64, f64, &str)| {
            let on = if v.1.is_nan() { "—".into() } else { format!("{:+.1}%", v.1) };
            let sh = if v.3 < 0.0 { "—".into() } else { format!("{:.0}%", v.3) };
            println!("{:<7} {:>13} {:>+10.1}% {:>8} {:>13.0} {:>+8.1} {}", v.0, on, v.2, sh, v.4, v.5, v.6);
        };
        println!("── ТОП НАКОПИЧЕННЯ (набирають в обох) ──");
        for v in rows.iter().filter(|v| v.6 == "НАБИРАЄ обидва").take(top) { show(v); }
        println!("\n── ТОП РОЗДАЧА (роздають в обох) ──");
        for v in rows.iter().rev().filter(|v| v.6 == "РОЗДАЄ обидва").take(top.min(10)) { show(v); }
        let div = rows.iter().filter(|v| v.6 == "розбіжність").count();
        println!("\nрозбіжність (один контур купує, інший продає): {div} токенів (сигнал слабкий/маскування)");
        println!("\nЧИТАТИ: ончейн-нетто% = чисте сальдо мейкерів групи як % їх обігу; CEX-нетто% = чистий taker-потік;");
        println!("score = (ончейн+CEX нетто)·вага(ончММ). ЗАСТЕРЕЖЕННЯ: проксі-потік (не рахунки); ончейн MM churn-ять, тож нетто% малий за модулем — важливий ЗНАК і ЗБІГ.");
        Ok(())
    }
}
