//! ПАТЕРН journal (Фаза 0.3): показати ЖУРНАЛ ПЕРЕРАХУНКІВ вітрин.
//! По будь-якій цифрі видно: коли, яким кодом (hash білда), на яких входах побудовано вихід.
//! Читає recompute-journal.jsonl. Аргументи: n=20 (останніх записів)  tool=<фільтр>
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Journal;

fn ymd(ts: i64) -> String {
    // прості UTC дата-час без chrono
    let days = ts / 86400; let rem = ts % 86400;
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let mut y = 1970i64; let mut d = days;
    loop { let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0; let dy = if leap {366} else {365};
        if d >= dy { d -= dy; y += 1; } else { break } }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let ml = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 0; while d >= ml[mo] { d -= ml[mo]; mo += 1; }
    format!("{y}-{:02}-{:02} {:02}:{:02}", mo + 1, d + 1, h, mi)
}

impl Pattern for Journal {
    fn name(&self) -> &str { "journal" }
    fn about(&self) -> &str { "журнал перерахунків вітрин: коли/яким кодом/на яких даних (Фаза 0.3)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let n = num(args, "n", 20.0) as usize;
        let filt = args.get("tool").cloned();
        let path = format!("{}/recompute-journal.jsonl", ctx.dir);
        let txt = std::fs::read_to_string(&path)
            .map_err(|e| format!("нема recompute-journal.jsonl (ще не було перерахунків): {e}"))?;
        let mut rows: Vec<Value> = txt.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect();
        if let Some(t) = &filt { rows.retain(|r| r["tool"].as_str() == Some(t.as_str())); }

        println!("── ЖУРНАЛ ПЕРЕРАХУНКІВ ВІТРИН (Фаза 0.3) ──");
        println!("записів: {} · показано останніх {}\n", rows.len(), n.min(rows.len()));
        println!("{:<17} {:<12} {:<17} {:<22} входи", "коли (UTC)", "інструмент", "code_hash", "вихід");
        for r in rows.iter().rev().take(n).rev() {
            let ts = r["ts"].as_i64().unwrap_or(0);
            let tool = r["tool"].as_str().unwrap_or("?");
            let hash = r["code_hash"].as_str().unwrap_or("?");
            let out = r["out"].as_str().unwrap_or("?");
            let ins: Vec<String> = r["inputs"].as_array().map(|a| a.iter()
                .filter_map(|x| x["f"].as_str().map(String::from)).collect()).unwrap_or_default();
            println!("{:<17} {:<12} {:<17} {:<22} {}", ymd(ts), tool, hash, out, ins.join(", "));
        }
        println!("\ncode_hash — fingerprint білда рушія (FNV-1a виконуваного файлу); dict-версія у кожному рядку jsonl.");
        println!("Так по цифрі з вітрини видно, ЯКИМ кодом і на ЯКИХ входах (з їх mtime/size) її зроблено.");
        Ok(())
    }
}
