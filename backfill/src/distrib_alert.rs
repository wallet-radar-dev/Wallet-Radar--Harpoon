//! distrib_alert (Rust) — Telegram-алерт про зміну фази китової мережі по кожному активу.
//! Читає capital-groups.json (realPhase/sellPct/toCexPct/rotatePct на CAP-групах), порівнює з попереднім
//! станом і шле, коли актив ПОЧИНАЄ роздавати (біржа/DEX) або переходить у ротацію. Перший запуск =
//! базовий знімок + зведення. Стан: deploy/distrib-alert.state.json. Reqwest (лише Telegram).
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::BufReader;

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
async fn tg(client: &reqwest::Client, token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() { eprintln!("[distrib-alert] нема TELEGRAM креденшлів"); return false; }
    matches!(client.post(format!("https://api.telegram.org/bot{token}/sendMessage"))
        .form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"), ("disable_web_page_preview", "true")]).send().await,
        Ok(r) if r.status().is_success())
}
fn usd(v: f64) -> String { if v >= 1e9 { format!("${:.2}млрд", v / 1e9) } else { format!("${:.0}M", v / 1e6) } }

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let (token, chat) = (envv(&root, "TELEGRAM_BOT_TOKEN"), envv(&root, "TELEGRAM_CHAT_ID"));
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().unwrap();
    let cg: Value = serde_json::from_reader(BufReader::new(fs::File::open(format!("{root}/dataset/capital-groups.json")).unwrap())).unwrap();

    // sym -> {phase, sell, cex, rot, usd, n}
    let mut cur: HashMap<String, Value> = HashMap::new();
    for g in cg["groups"].as_array().unwrap_or(&vec![]) {
        if !g["id"].as_str().unwrap_or("").starts_with("CAP:") { continue; }
        let sym = g["commonAssets"].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).unwrap_or("?").to_string();
        cur.insert(sym, json!({
            "phase": g["realPhase"].as_str().unwrap_or("?"),
            "sell": (g["sellPct"].as_f64().unwrap_or(0.0) * 100.0).round(),
            "cex": (g["toCexPct"].as_f64().unwrap_or(0.0) * 100.0).round(),
            "dex": (g["toDexPct"].as_f64().unwrap_or(0.0) * 100.0).round(),
            "rot": (g["rotatePct"].as_f64().unwrap_or(0.0) * 100.0).round(),
            "usd": g["totalUsd"].as_f64().unwrap_or(0.0), "n": g["size"].as_u64().unwrap_or(0),
        }));
    }
    let statef = format!("{root}/deploy/distrib-alert.state.json");
    let prev: HashMap<String, String> = fs::read_to_string(&statef).ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["phases"].as_object().map(|o| o.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect()))
        .unwrap_or_default();

    const DIST: &str = "РОЗДАЄ (біржа/DEX)";
    const ROT: &str = "РОТАЦІЯ (нові гаманці)";
    let mut msgs: Vec<String> = Vec::new();
    if prev.is_empty() {
        let mut dist: Vec<&String> = cur.iter().filter(|(_, d)| d["phase"] == DIST).map(|(s, _)| s).collect();
        dist.sort();
        msgs.push(format!("🐋 <b>Моніторинг роздачі китової мережі увімкнено</b>\n\nЗараз РОЗДАЮТЬ (біржа/DEX), {}:\n{}\n\nНадішлю, щойно актив ПОЧНЕ роздавати або ротувати.",
            dist.len(), dist.iter().take(25).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    } else {
        let mut items: Vec<(&String, &Value)> = cur.iter().collect();
        items.sort_by(|a, b| b.1["sell"].as_f64().unwrap_or(0.0).partial_cmp(&a.1["sell"].as_f64().unwrap_or(0.0)).unwrap());
        for (sym, d) in items {
            let ph = d["phase"].as_str().unwrap_or("");
            if prev.get(sym).map(|s| s == ph).unwrap_or(false) { continue; }
            if ph == DIST && prev.get(sym).map(|s| s != DIST).unwrap_or(true) {
                msgs.push(format!("🔴 <b>ПОЧАЛИ РОЗДАВАТИ: {sym}</b>\nпродаж {}% (DEX {}% / CEX {}%), ротація {}%\n{} · {} гам",
                    d["sell"].as_f64().unwrap_or(0.0), d["dex"].as_f64().unwrap_or(0.0), d["cex"].as_f64().unwrap_or(0.0), d["rot"].as_f64().unwrap_or(0.0), usd(d["usd"].as_f64().unwrap_or(0.0)), d["n"].as_u64().unwrap_or(0)));
            } else if ph == ROT && prev.get(sym).map(|s| s != ROT).unwrap_or(true) {
                msgs.push(format!("🟠 <b>РОТАЦІЯ У НОВІ ГАМАНЦІ: {sym}</b>\nперекладають між гаманцями (не продаж), ротація {}%\n{} · {} гам",
                    d["rot"].as_f64().unwrap_or(0.0), usd(d["usd"].as_f64().unwrap_or(0.0)), d["n"].as_u64().unwrap_or(0)));
            }
        }
    }
    let mut n = 0;
    for m in &msgs { if tg(&client, &token, &chat, m).await { n += 1; } }
    if msgs.is_empty() { eprintln!("[distrib-alert] змін фази нема"); }
    let phases: serde_json::Map<String, Value> = cur.iter().map(|(s, d)| (s.clone(), d["phase"].clone())).collect();
    let _ = fs::write(&statef, serde_json::to_string(&json!({"phases": phases})).unwrap());
    eprintln!("[distrib-alert] надіслано {n} повідомлень, стан збережено");
}
