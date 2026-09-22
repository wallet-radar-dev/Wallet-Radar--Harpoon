//! ПАТЕРН baltrack: чи виріс native-баланс за весь період (РЕКОНСТРУКЦІЯ з історії tx).
//! Історичний eth_getBalance на free не працює, тож будуємо біжучий баланс:
//!   баланс(t) = cumsum(вхідні - вихідні - газ) по всіх tx до t.
//! Пагінація по вікнах блоків (щоб дістати всі tx навіть у бота з 100k+ tx).
//! Аргументи: seeds=0x..  points=12  now_ts=<unix>  eth=3000  windows=40
use crate::core::ctx::Ctx;
use crate::core::fetch::Es;
use crate::core::PResult;
use super::{Pattern, csv, num};
use serde_json::Value;

pub struct BalTrack;

impl Pattern for BalTrack {
    fn name(&self) -> &str { "baltrack" }
    fn about(&self) -> &str { "траєкторія native-балансу за весь період (реконструкція з tx)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let points = num(args, "points", 12.0) as usize;
        let peth = num(args, "eth", 3000.0);
        let now_ts = num(args, "now_ts", 1_786_147_200.0) as u64;
        let windows = num(args, "windows", 40.0) as u64;
        let es = Es::new(&ctx.keys);
        let chain = 1u64;

        // members=<файл> -> СУКУПНА крива групи; інакше seeds=<список> -> покриво на гаманець
        let mut seeds = csv(args, "seeds", "");
        let mut aggregate = false;
        if let Some(p) = args.get("members") {
            seeds = std::fs::read_to_string(p).map_err(|e| e.to_string())?.lines()
                .filter_map(|l| l.split_whitespace().next().map(|s| s.to_lowercase()))
                .filter(|s| s.starts_with("0x")).collect();
            aggregate = true;
        }
        if seeds.is_empty() { return Err("вкажи seeds=0x..,0x.. або members=<файл>".into()) }

        // === СУКУПНИЙ режим: злити події всіх у одну криву ===
        if aggregate {
            let mut all: Vec<(u64, f64)> = Vec::new();
            let mut min_start = now_ts;
            eprintln!("[baltrack] сукупно по {} гаманцях...", seeds.len());
            for (idx, a) in seeds.iter().enumerate() {
                let Some(st) = es.first_tx_ts(chain, a) else { continue };
                min_start = min_start.min(st);
                let Some(fb) = es.block_by_time(chain, st) else { continue };
                let Some(lb) = es.block_by_time(chain, now_ts) else { continue };
                if lb <= fb { continue }
                collect(&es, chain, a, fb, lb, windows.min(8), &mut all);
                if (idx + 1) % 25 == 0 { eprintln!("  {}/{}", idx + 1, seeds.len()); }
            }
            all.sort_by_key(|e| e.0);
            println!("=== СУКУПНА крива кола ({} гам, подій {}):", seeds.len(), all.len());
            let mut bal = 0.0; let mut ei = 0; let mut peak = 0.0; let mut prev = 0.0;
            for i in 0..=points {
                let ts = min_start + (now_ts - min_start) * (i as u64) / (points as u64);
                while ei < all.len() && all[ei].0 <= ts { bal += all[ei].1; ei += 1; }
                if bal > peak { peak = bal }
                let arrow = if bal > prev + 1.0 { "↑" } else if bal < prev - 1.0 { "↓" } else { "·" };
                println!("  {}  {:>11.1} ETH (~${:>11.0}) {arrow}", ymd(ts), bal.max(0.0), bal.max(0.0) * peth);
                prev = bal;
            }
            println!("  ПІДСУМОК групи: пік {:.0} ETH · зараз {:.0} ETH · {}", peak, prev.max(0.0),
                if prev > 1.0 { "ВИРОСЛА" } else { "не виросла" });
            return Ok(());
        }

        for a in &seeds {
            let start_ts = match es.first_tx_ts(chain, a) { Some(t) => t, None => { println!("{a}: нема історії"); continue } };
            let Some(first_blk) = es.block_by_time(chain, start_ts) else { continue };
            let Some(last_blk) = es.block_by_time(chain, now_ts) else { continue };
            if last_blk <= first_blk { println!("{a}: короткий період"); continue }

            // збираємо (ts, delta ETH) по вікнах блоків
            let mut events: Vec<(u64, f64)> = Vec::new();
            let mut truncated = 0u32;
            let span = last_blk - first_blk;
            for wi in 0..windows {
                let sb = first_blk + span * wi / windows;
                let eb = first_blk + span * (wi + 1) / windows - 1;
                for act in ["txlist", "txlistinternal"] {
                    let r = es.call(chain, &format!("module=account&action={act}"),
                        &format!("&address={a}&startblock={sb}&endblock={eb}&page=1&offset=10000&sort=asc"));
                    if let Some(arr) = r.as_ref().and_then(Value::as_array) {
                        if arr.len() >= 10000 { truncated += 1; }
                        for t in arr {
                            let ts: u64 = t.get("timeStamp").and_then(|x| x.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0);
                            let fr = t.get("from").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                            let to = t.get("to").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                            // невдала tx (isError=1) ETH НЕ рухає (лише газ спалює)
                            let failed = t.get("isError").and_then(|x| x.as_str()).unwrap_or("0") == "1";
                            let val = if failed { 0.0 } else {
                                t.get("value").and_then(|x| x.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0) / 1e18 };
                            let mut d = 0.0;
                            if to == *a { d += val; }
                            if fr == *a {
                                d -= val;
                                // газ лише для нормальних tx (txlist), де гаманець платник
                                if act == "txlist" {
                                    let gu = t.get("gasUsed").and_then(|x| x.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                    let gp = t.get("gasPrice").and_then(|x| x.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                    d -= gu * gp / 1e18;
                                }
                            }
                            if d != 0.0 && ts > 0 { events.push((ts, d)); }
                        }
                    }
                }
            }
            events.sort_by_key(|e| e.0);
            // cumsum -> баланс(t); семпл у points точок
            println!("=== {a}  (актив з {}, подій {}{})", ymd(start_ts), events.len(),
                if truncated > 0 { format!(", {truncated} вікон обрізано!") } else { String::new() });
            let mut bal = 0.0f64; let mut ei = 0usize; let mut peak = 0.0f64; let mut prev = 0.0f64;
            for i in 0..=points {
                let ts = start_ts + (now_ts - start_ts) * (i as u64) / (points as u64);
                while ei < events.len() && events[ei].0 <= ts { bal += events[ei].1; ei += 1; }
                if bal > peak { peak = bal }
                let arrow = if bal > prev + 0.05 { "↑" } else if bal < prev - 0.05 { "↓" } else { "·" };
                println!("  {}  {:>11.2} ETH (~${:>10.0}) {arrow}", ymd(ts), bal.max(0.0), bal.max(0.0) * peth);
                prev = bal;
            }
            let grew = prev > 0.5;
            println!("  ПІДСУМОК: старт ~0 -> зараз {:.1} ETH · пік {:.1} ETH · {}",
                prev.max(0.0), peak, if grew { "БАЛАНС ВИРІС" } else { "баланс НЕ виріс" });
        }
        Ok(())
    }
}

/// Збирає (ts, native-delta ETH) для адреси по вікнах блоків у events.
fn collect(es: &Es, chain: u64, a: &str, first_blk: u64, last_blk: u64, windows: u64, events: &mut Vec<(u64, f64)>) {
    let span = last_blk - first_blk;
    for wi in 0..windows {
        let sb = first_blk + span * wi / windows;
        let eb = first_blk + span * (wi + 1) / windows - 1;
        for act in ["txlist", "txlistinternal"] {
            let r = es.call(chain, &format!("module=account&action={act}"),
                &format!("&address={a}&startblock={sb}&endblock={eb}&page=1&offset=10000&sort=asc"));
            if let Some(arr) = r.as_ref().and_then(Value::as_array) {
                for t in arr {
                    let ts: u64 = t.get("timeStamp").and_then(|x| x.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let fr = t.get("from").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                    let to = t.get("to").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                    let failed = t.get("isError").and_then(|x| x.as_str()).unwrap_or("0") == "1";
                    let val = if failed { 0.0 } else {
                        t.get("value").and_then(|x| x.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0) / 1e18 };
                    let mut d = 0.0;
                    if to == *a { d += val; }
                    if fr == *a {
                        d -= val;
                        if act == "txlist" {
                            let gu = t.get("gasUsed").and_then(|x| x.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                            let gp = t.get("gasPrice").and_then(|x| x.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                            d -= gu * gp / 1e18;
                        }
                    }
                    if d != 0.0 && ts > 0 { events.push((ts, d)); }
                }
            }
        }
    }
}

fn ymd(ts: u64) -> String {
    let days = ts / 86400; let mut y = 1970u64; let mut d = days;
    loop { let leap = (y%4==0 && y%100!=0)||y%400==0; let yd = if leap {366} else {365};
        if d < yd { break } d -= yd; y += 1; }
    let leap = (y%4==0 && y%100!=0)||y%400==0;
    let ml = [31, if leap {29} else {28}, 31,30,31,30,31,31,30,31,30,31];
    let mut m = 0usize; while m < 12 && d >= ml[m] { d -= ml[m]; m += 1; }
    format!("{y}-{:02}", m + 1)
}
