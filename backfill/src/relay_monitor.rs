//! relay_monitor (Rust) — стежить за реле-хабами обфускованої групи, шле Telegram при значній
//! активності (рух priced-альтів реле↔ядро = ранній сигнал руху групи). Etherscan tokentx по кожному
//! реле з часу останньої перевірки. Стан: deploy/relay-monitor.state.json (останній блок/реле).
//! Reqwest+tokio, таймаути. Cron кожні ~20хв. Env: MIN_EVENTS(3).
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::BufReader;
use std::sync::atomic::{AtomicUsize, Ordering};

// (адреса, мітка, чи біржо-суміжне)


/// Реле мережі: (адреса, мітка, чи біржо-суміжне). Тег `relay_monitor:relays`.
fn relays() -> Vec<(String, String, bool)> {
    let c = wr_store::open().expect("relay_monitor: без сховища немає переліку реле");
    let mut q = c
        .prepare(
            "SELECT w.address, COALESCE(w.label,''), COALESCE(g.notes,'') FROM wallet_tags g
             JOIN wallets w ON w.chain=g.chain AND w.address=g.address
             WHERE g.tag='relay_monitor:relays'",
        )
        .expect("запит реле");
    let v: Vec<(String, String, bool)> = q
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,
                r.get::<_, String>(2)? == "exchange_adjacent"))
        })
        .expect("читання")
        .filter_map(|r| r.ok())
        .collect();
    assert!(!v.is_empty(), "relay_monitor: тег relay_monitor:relays порожній");
    v
}

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
async fn es(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, params: &str) -> Vec<Value> {
    for _ in 0..4 {
        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
        if let Ok(r) = client.get(format!("https://api.etherscan.io/v2/api?chainid=1&{params}&apikey={key}")).send().await {
            if let Ok(v) = r.json::<Value>().await { if let Some(a) = v["result"].as_array() { return a.clone(); } }
        }
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }
    Vec::new()
}
async fn tg(client: &reqwest::Client, token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() { eprintln!("[relay-monitor] нема TELEGRAM креденшлів"); return false; }
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    matches!(client.post(&url).form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"), ("disable_web_page_preview", "true")]).send().await,
             Ok(r) if r.status().is_success())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_events: usize = std::env::var("MIN_EVENTS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if keys.is_empty() { eprintln!("[relay-monitor] нема BACKFILL_ETHERSCAN_KEYS"); return; }
    let (token, chat) = (envv(&root, "TELEGRAM_BOT_TOKEN"), envv(&root, "TELEGRAM_CHAT_ID"));
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap();

    // ядро (охопл≥5) + priced-альти
    let mut cores: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(fs::File::open(format!("{bf}/rglead.json")).unwrap())) {
        if let Some(ws) = v["wallets"].as_array() { for w in ws { if w["coverage"].as_u64().unwrap_or(0) >= 5 { if let Some(a) = w["w"].as_str() { cores.insert(a.to_lowercase()); } } } }
    }
    let mut priced: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(fs::File::open(format!("{bf}/onchain-prices.json")).unwrap())) { for k in v.keys() { priced.insert(k.to_lowercase()); } }

    let statef = format!("{root}/deploy/relay-monitor.state.json");
    let mut state: HashMap<String, u64> = fs::read_to_string(&statef).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();

    let mut sent = 0;
    for (addr, label, is_exch) in relays() {
        let last = state.get(&addr).copied().unwrap_or(0);
        let txs = es(&client, &keys, &kc, &format!("module=account&action=tokentx&address={addr}&startblock={}&endblock=99999999&page=1&offset=200&sort=desc", last + 1)).await;
        let mut maxblk = last;
        let mut ev: Vec<(String, bool, bool)> = Vec::new(); // (sym, is_out, is_core)
        for t in &txs {
            let blk: u64 = t["blockNumber"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0); if blk > maxblk { maxblk = blk; }
            let tok = t["contractAddress"].as_str().unwrap_or("").to_lowercase(); if !priced.contains(&tok) { continue; }
            let frm = t["from"].as_str().unwrap_or("").to_lowercase(); let to = t["to"].as_str().unwrap_or("").to_lowercase();
            let is_out = frm == addr; let other = if is_out { to } else { frm };
            ev.push((t["tokenSymbol"].as_str().unwrap_or("?").to_string(), is_out, cores.contains(&other)));
        }
        state.insert(addr.to_string(), maxblk);
        let core_ev = ev.iter().filter(|e| e.2).count();
        if ev.len() >= min_events || core_ev >= 1 {
            let mut syms: HashMap<String, (u32, u32)> = HashMap::new();
            for (s, out, _) in &ev { let e = syms.entry(s.clone()).or_insert((0, 0)); if *out { e.0 += 1 } else { e.1 += 1 } }
            let top: Vec<String> = syms.iter().take(5).map(|(s, v)| format!("{s}(роздав {}/отримав {})", v.0, v.1)).collect();
            let flag = if is_exch { " ⚠️Paribu-сум." } else { "" };
            let msg = format!("🔴 <b>АКТИВНІСТЬ РЕЛЕ: {label}</b>{flag}\n<code>{addr}</code>\nнових priced-альт подій: {} (з ядром: {core_ev})\n{}\n→ можливий рух групи", ev.len(), top.join(", "));
            if tg(&client, &token, &chat, &msg).await { sent += 1; }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let _ = fs::write(&statef, serde_json::to_string(&json!(state)).unwrap());
    eprintln!("[relay-monitor] надіслано {sent} алертів, стан збережено");
}
