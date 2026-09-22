//! ПАТЕРН mmalert: АЛЕРТИ на ЗМІНУ ВЕРДИКТУ токена у звідному сигналі (mmsignal-history.csv).
//! Порівнює два останні знімки й ловить перехід: accum↔distrib (розворот — найсильніше),
//! вхід/вихід із накопичення/роздачі, поява/зникнення сигналу. З send=1 шле в Telegram.
//! Читає mmsignal-history.csv (+ .env для TG). Аргументи: send=1  min_delta=0 (поріг |Δscore|)
use std::collections::HashMap;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct MmAlert;

fn label(code: &str) -> &'static str {
    match code {
        "accum" => "НАБИРАЄ обидва", "distrib" => "РОЗДАЄ обидва", "diverge" => "розбіжність",
        "cex_buy" => "лише CEX+", "cex_sell" => "лише CEX−", _ => code_leak(code),
    }
}
fn code_leak(_c: &str) -> &'static str { "?" }

/// пріоритет зміни: розворот accum↔distrib найвищий
fn severity(from: &str, to: &str) -> i32 {
    match (from, to) {
        ("accum", "distrib") | ("distrib", "accum") => 3, // повний розворот
        (_, "accum") | ("distrib", _) => 2,               // став набирати / перестав роздавати
        ("accum", _) | (_, "distrib") => 2,               // перестав набирати / став роздавати
        _ => 1,
    }
}

fn env_val(root: &str, key: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix(&format!("{key}=")).map(|v| v.trim().to_string())))
        .unwrap_or_default()
}

impl Pattern for MmAlert {
    fn name(&self) -> &str { "mmalert" }
    fn about(&self) -> &str { "алерти на зміну вердикту токена у звідному ММ-сигналі (+Telegram)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let send = args.get("send").map(|s| s == "1").unwrap_or(false);
        let min_delta = num(args, "min_delta", 0.0);

        let path = format!("{}/mmsignal-history.csv", ctx.dir);
        let txt = std::fs::read_to_string(&path)
            .map_err(|e| format!("нема mmsignal-history.csv (спершу mmsignal save=1): {e}"))?;

        // (symbol,ts) -> (code, score)
        let mut cell: HashMap<(String, u64), (String, f64)> = HashMap::new();
        let mut tss: Vec<u64> = vec![];
        for line in txt.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 8 { continue }
            let ts: u64 = c[0].parse().unwrap_or(0);
            let sym = c[1].to_string();
            let score: f64 = c[6].parse().unwrap_or(0.0);
            let verdict = c[7].to_string();
            cell.insert((sym, ts), (verdict, score));
            if !tss.contains(&ts) { tss.push(ts); }
        }
        tss.sort_unstable();
        if tss.len() < 2 {
            println!("[mmalert] лише {} знімок — потрібно ≥2 для порівняння (snapshot накопичить щодня)", tss.len());
            return Ok(());
        }
        let (prev, last) = (tss[tss.len() - 2], tss[tss.len() - 1]);

        // зміни вердикту між prev і last
        let mut syms: Vec<String> = cell.keys().map(|(s, _)| s.clone()).collect();
        syms.sort_unstable(); syms.dedup();
        let mut changes: Vec<(String, String, String, f64, f64, i32)> = vec![]; // sym, from, to, dscore, score_now, sev
        for s in &syms {
            let (Some((cf, sf)), Some((ct, st))) = (cell.get(&(s.clone(), prev)), cell.get(&(s.clone(), last))) else { continue };
            if cf == ct { continue }
            let dscore = st - sf;
            if dscore.abs() < min_delta { continue }
            changes.push((s.clone(), cf.clone(), ct.clone(), dscore, *st, severity(cf, ct)));
        }
        changes.sort_by(|a, b| b.5.cmp(&a.5).then(b.3.abs().partial_cmp(&a.3.abs()).unwrap()));

        let when = format!("ts {prev} → {last}");
        println!("── АЛЕРТИ: ЗМІНА ВЕРДИКТУ ММ-СИГНАЛУ ({when}) ──");
        if changes.is_empty() { println!("змін вердикту немає."); return Ok(()) }
        let mut msg = String::from("🔔 Wallet Radar — зміна ММ-сигналу:\n");
        for (s, cf, ct, ds, now, sev) in &changes {
            let arrow = if *sev == 3 { "🔄 РОЗВОРОТ" } else { "→" };
            let em = match ct.as_str() { "accum" => "🟢", "distrib" => "🔴", _ => "⚪" };
            let line = format!("{em} {} {arrow} {} → {} (score {:+.1}, Δ{:+.1})", s, label(cf), label(ct), now, ds);
            println!("  {line}");
            msg.push_str(&line); msg.push('\n');
        }
        println!("\nусього змін: {}", changes.len());

        if send {
            let root = "/opt/wallet-tracker";
            let token = env_val(root, "TELEGRAM_BOT_TOKEN");
            let chat = { let c = env_val(root, "TELEGRAM_CHAT_ID"); if c.is_empty() { env_val(root, "TG_CHAT_ID") } else { c } };
            if token.is_empty() || chat.is_empty() {
                println!("[mmalert] TG не налаштовано (.env TELEGRAM_BOT_TOKEN/TELEGRAM_CHAT_ID) — не надіслано");
            } else {
                let url = format!("https://api.telegram.org/bot{token}/sendMessage");
                match ureq::post(&url).timeout(std::time::Duration::from_secs(15))
                    .send_json(serde_json::json!({"chat_id": chat, "text": msg})) {
                    Ok(_) => println!("[mmalert] надіслано в Telegram ({} змін)", changes.len()),
                    Err(e) => println!("[mmalert] TG помилка: {e}"),
                }
            }
        }
        Ok(())
    }
}
