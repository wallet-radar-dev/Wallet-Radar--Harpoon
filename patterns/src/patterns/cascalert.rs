//! ПАТЕРН cascalert: АЛЕРТИ на зміну дії у стратегії Каскад (cascade-history.csv).
//! Ловить перетин порогів: ТРИМАТИ/РОТАЦІЯ → ВИХІД (кит перетнув поріг роздачі — фіксувати!),
//! або РОТАЦІЯ-В (зʼявилась нова ціль). Порівнює два останні знімки. З send=1 шле в Telegram.
//! Аргументи: send=1
use std::collections::HashMap;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::Pattern;

pub struct CascAlert;

fn label(c: &str) -> &'static str {
    match c { "exit" => "🔴 ВИХІД", "rotate_in" => "🟢 РОТАЦІЯ-В", "hold" => "⚪ ТРИМАТИ", _ => c_leak() }
}
fn c_leak() -> &'static str { "?" }
fn sev(from: &str, to: &str) -> i32 {
    match (from, to) {
        (_, "exit") => 3,          // перетнув поріг виходу — найважливіше
        ("exit", _) => 2,          // вийшов зі стану виходу
        (_, "rotate_in") => 2,     // нова ціль ротації
        _ => 1,
    }
}
fn env_val(root: &str, key: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix(&format!("{key}=")).map(|v| v.trim().to_string())))
        .unwrap_or_default()
}

impl Pattern for CascAlert {
    fn name(&self) -> &str { "cascalert" }
    fn about(&self) -> &str { "алерти на зміну дії каскаду (перетин порогу виходу) +Telegram" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let send = args.get("send").map(|s| s == "1").unwrap_or(false);
        let path = format!("{}/cascade-history.csv", ctx.dir);
        let txt = std::fs::read_to_string(&path)
            .map_err(|e| format!("нема cascade-history.csv (спершу cascade save=1): {e}"))?;
        let mut cell: HashMap<(String, u64), String> = HashMap::new();
        let mut tss: Vec<u64> = vec![];
        for line in txt.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 5 { continue }
            let ts: u64 = c[0].parse().unwrap_or(0);
            cell.insert((c[1].to_string(), ts), c[4].to_string());
            if !tss.contains(&ts) { tss.push(ts); }
        }
        tss.sort_unstable();
        if tss.len() < 2 { println!("[cascalert] лише {} знімок — потрібно ≥2 (cron накопичить)", tss.len()); return Ok(()) }
        let (prev, last) = (tss[tss.len() - 2], tss[tss.len() - 1]);

        let mut syms: Vec<String> = cell.keys().map(|(s, _)| s.clone()).collect();
        syms.sort_unstable(); syms.dedup();
        let mut ch: Vec<(String, String, String, i32)> = vec![];
        for s in &syms {
            let (Some(a), Some(b)) = (cell.get(&(s.clone(), prev)), cell.get(&(s.clone(), last))) else { continue };
            if a != b { ch.push((s.clone(), a.clone(), b.clone(), sev(a, b))); }
        }
        ch.sort_by(|a, b| b.3.cmp(&a.3));

        println!("── АЛЕРТИ КАСКАД: зміна дії (ts {prev} → {last}) ──");
        if ch.is_empty() { println!("змін немає."); return Ok(()) }
        let mut msg = String::from("🌊 Wallet Radar — Каскад, зміна дії:\n");
        for (s, a, b, sv) in &ch {
            let mark = if *sv == 3 { " ⚠ ПЕРЕТНУВ ПОРІГ ВИХОДУ" } else { "" };
            let line = format!("{} {} → {}{}", s, label(a), label(b), mark);
            println!("  {line}"); msg.push_str(&line); msg.push('\n');
        }
        println!("\nусього змін: {}", ch.len());
        if send {
            let token = env_val("/opt/wallet-tracker", "TELEGRAM_BOT_TOKEN");
            let chat = env_val("/opt/wallet-tracker", "TELEGRAM_CHAT_ID");
            if token.is_empty() || chat.is_empty() { println!("[cascalert] TG не налаштовано"); }
            else {
                let url = format!("https://api.telegram.org/bot{token}/sendMessage");
                match ureq::post(&url).timeout(std::time::Duration::from_secs(15))
                    .send_json(serde_json::json!({"chat_id": chat, "text": msg})) {
                    Ok(_) => println!("[cascalert] надіслано в Telegram"),
                    Err(e) => println!("[cascalert] TG помилка: {e}"),
                }
            }
        }
        Ok(())
    }
}
