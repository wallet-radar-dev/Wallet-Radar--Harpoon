//! sellstudy — чи рухає ринок сам ПРОДАЖ мережі (а не заряджання роздавачем).
//!
//! Попередній тест (eventstudy) міряв хвилі виплат і дав нуль. Але виплати — це операційна подія,
//! а не ринкова. Тут перевіряємо дві події, яких ми ще не міряли:
//!   MODE=sells    — ПОТОКЕННО: день, коли група вилила токен X у DEX сильно вище своєї норми →
//!                   що робить ЦІНА САМОГО X далі. Нуль-модель береться з ЦЬОГО Ж токена,
//!                   тож власний дрейф монети не зараховується як сигнал.
//!   MODE=treasury — рухи казни (рідкі, великі) → кошик мережі, як у eventstudy.
//! Дохідність надлишкова (мінус ETH). Значущість — перестановкою на випадкових датах.
//!
//! Вихід: dataset/backfill/sellstudy-<mode>.json. Args: ROOT. Env: MODE(sells) SPIKE(3) MIN_USD(25000)
//!        HORIZONS(1,3,7,14) ITERS(300) CHAIN_FILTER(eth)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};


/// Перша адреса з тегу — замість зашитого значення за замовчуванням.
/// Порожній результат краще, ніж мовчки взята стара адреса: видно одразу.
fn default_tagged(tag: &str) -> String {
    let Ok(c) = wr_store::open() else { return String::new() };
    let Ok(mut q) =
        c.prepare("SELECT address FROM wallet_tags WHERE tag=?1 ORDER BY address LIMIT 1")
    else {
        return String::new();
    };
    q.query_row([tag], |r| r.get(0)).unwrap_or_default()
}

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
fn day_from_iso(ts: &str) -> i64 {
    if ts.len() < 10 { return 0; }
    let (y, m, d): (i64, i64, i64) = (ts[0..4].parse().unwrap_or(0), ts[5..7].parse().unwrap_or(0), ts[8..10].parse().unwrap_or(0));
    if y < 2015 { return 0; }
    let yy = if m <= 2 { y - 1 } else { y };
    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
    let yoe = yy - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + d - 1 - 719468
}
struct Rng(u64);
impl Rng { fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x } }

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let mode = std::env::var("MODE").unwrap_or_else(|_| "sells".into());
    let spike: f64 = std::env::var("SPIKE").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(25_000.0);
    let iters: usize = std::env::var("ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    let chain_filter = std::env::var("CHAIN_FILTER").unwrap_or_else(|_| "eth".into());
    let horizons: Vec<i64> = std::env::var("HORIZONS").unwrap_or_else(|_| "1,3,7,14".into())
        .split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let bf = format!("{root}/dataset/backfill");

    // ціни
    let hp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices-hist.json")).expect("token-prices-hist"))).unwrap();
    let mut px: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for (c, t) in hp["tokens"].as_object().cloned().unwrap_or_default() {
        let m: HashMap<i64, f64> = t["prices"].as_array().cloned().unwrap_or_default().iter()
            .filter_map(|p| Some(((p[0].as_u64()? / 86400) as i64, p[1].as_f64()?))).collect();
        if m.len() >= 60 { px.insert(c.to_lowercase(), m); }
    }
    let eth = px.get("eth").cloned().unwrap_or_default();
    let ret = |m: &HashMap<i64, f64>, d: i64, h: i64| -> Option<f64> {
        let (a, b) = (m.get(&d)?, m.get(&(d + h))?);
        if *a <= 0.0 { return None; }
        Some(b / a - 1.0)
    };
    let excess = |m: &HashMap<i64, f64>, d: i64, h: i64| -> Option<f64> {
        Some(ret(m, d, h)? - ret(&eth, d, h).unwrap_or(0.0))
    };

    // члени групи
    let memb: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/group-membership.json")).expect("membership"))).unwrap();
    let members: std::collections::HashSet<String> = memb["wallets"].as_object().cloned().unwrap_or_default().iter()
        .filter(|(_, x)| x["group"].as_str() == Some("NET")).map(|(w, _)| w.to_lowercase()).collect();

    let mut events: Vec<(String, i64, f64)> = Vec::new();   // (токен, день, обсяг $)
    if mode == "sells" {
        // контракти = DEX; продаж = вихід токена на контракт
        let mut kind: HashMap<String, String> = HashMap::new();
        if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
            for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
        }
        let cur: HashMap<String, f64> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/token-prices.json")).unwrap())).ok()
            .and_then(|v| v["tokens"].as_object().map(|m| m.iter().filter_map(|(k, x)| x["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect())).unwrap_or_default();
        let mut daily: HashMap<(String, i64), f64> = HashMap::new();
        for line in BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).expect("transfers")).lines().map_while(Result::ok) {
            let mut it = line.splitn(6, ',');
            let w = it.next().unwrap_or(""); let ch = it.next().unwrap_or(""); let d = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or("");
            if w == "wallet" || d != "o" || ch != chain_filter { continue; }
            let wl = w.to_lowercase(); if !members.contains(&wl) { continue; }
            let tl = tok.to_lowercase(); if !px.contains_key(&tl) { continue; }
            let cp = line.rsplitn(4, ',').nth(2).unwrap_or("").to_lowercase();
            if kind.get(&cp).map(|k| k != "contract").unwrap_or(true) { continue; }   // лише у DEX/контракт
            let amt: f64 = line.rsplitn(4, ',').nth(1).unwrap_or("0").parse().unwrap_or(0.0);
            if amt <= 0.0 { continue; }
            let day = day_from_iso(ts); if day == 0 { continue; }
            *daily.entry((tl.clone(), day)).or_insert(0.0) += amt * cur.get(&tl).copied().unwrap_or(0.0);
        }
        // норма по кожному токену → сплеск
        let mut by_tok: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        for ((t, d), v) in &daily { by_tok.entry(t.clone()).or_default().push((*d, *v)); }
        for (t, mut v) in by_tok {
            if v.len() < 5 { continue; }
            v.sort_by(|a, b| a.0.cmp(&b.0));
            let mut vals: Vec<f64> = v.iter().map(|x| x.1).collect();
            vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = vals[vals.len() / 2];
            for (d, u) in v { if u >= min_usd && u >= spike * med.max(1.0) { events.push((t.clone(), d, u)); } }
        }
        eprintln!("[sellstudy] днів-сплесків продажу: {} у {} токенах", events.len(),
            events.iter().map(|e| &e.0).collect::<std::collections::HashSet<_>>().len());
    } else {
        // рухи казни → кошик мережі
        let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let kc = AtomicUsize::new(0);
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();
        let tre_s = default_tagged("cexsignal:treasury");
        let tre = tre_s.as_str();
        let mut days: HashMap<i64, f64> = HashMap::new();
        for p in 1..=8 {
            let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
            let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=txlist&address={tre}&page={p}&offset=1000&sort=desc&apikey={key}");
            // Ліміт Etherscan = HTTP 200 з `result` у вигляді рядка. Раніше після 4 спроб
            // `arr` лишався порожнім, `len < 1000` обривало пагінацію, і дні руху казни просто
            // не доходили до вибірки — подієве вікно будувалось на неповній історії.
            let mut arr: Vec<Value> = Vec::new();
            let mut got = false;
            for a in 0..7u64 {
                if let Ok(r) = client.get(&url).send().await {
                    let st = r.status().as_u16();
                    if st != 429 && st < 500 {
                        if let Ok(v) = r.json::<Value>().await {
                            let msg = v["message"].as_str().unwrap_or("");
                            let res = v["result"].as_str().unwrap_or("");
                            let limited = msg == "NOTOK" || res.contains("rate limit")
                                || res.contains("Max calls") || res.contains("Too many");
                            if !limited {
                                if let Some(x) = v["result"].as_array() { arr = x.clone(); got = true; break; }
                                if msg == "No transactions found" { got = true; break; }
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << a.min(6)))).await;
            }
            if !got {
                eprintln!("[sellstudy] ⚠ ліміт Etherscan обірвав історію казни на сторінці {p} — вибірка днів неповна, подієве вікно не публікувати");
                std::process::exit(2);
            }
            let len = arr.len();
            for t in &arr {
                if t["from"].as_str().unwrap_or("").to_lowercase() != tre || t["value"].as_str() == Some("0") { continue; }
                let ts: u64 = t["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                let eth_v: f64 = t["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
                if ts > 0 { *days.entry((ts / 86400) as i64).or_insert(0.0) += eth_v; }
            }
            if len < 1000 { break; }
        }
        // кошик = токени NET із цінами
        let sig: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/group-token-signals.json")).unwrap())).unwrap();
        let basket: Vec<String> = sig["tokens"].as_array().cloned().unwrap_or_default().iter()
            .filter_map(|t| t["token"].as_str().map(|s| s.to_lowercase())).filter(|t| px.contains_key(t)).collect();
        eprintln!("[sellstudy] днів руху казни: {} · кошик {} активів", days.len(), basket.len());
        for (d, v) in days { for t in &basket { events.push((t.clone(), d, v)); } }
    }
    if events.len() < 10 { eprintln!("[sellstudy] замало подій ({})", events.len()); return; }

    // спостережене vs випадкове (нуль береться з ТОГО Ж токена)
    let mut rows: Vec<Value> = Vec::new();
    eprintln!("[sellstudy] ===== РЕЖИМ {mode}: що з ціною після події =====");
    eprintln!("{:<9} {:>8} {:>13} {:>13} {:>8}", "горизонт", "подій", "після події", "випадково", "p");
    // ПСЕВДОРЕПЛІКАЦІЯ: у режимі treasury подій насправді стільки, скільки ДАТ, а не дата×токен.
    // Рахуючи 6 дат × 53 токени як 312 незалежних спостережень, тест дає хибну значущість
    // (p=0.005 на кошику з 6 подій). Тому для кошикових режимів спершу усереднюємо по даті.
    let per_date = mode != "sells";
    let dates: Vec<i64> = { let mut d: Vec<i64> = events.iter().map(|e| e.1).collect(); d.sort_unstable(); d.dedup(); d };
    let basket: Vec<String> = { let mut t: Vec<String> = events.iter().map(|e| e.0.clone()).collect(); t.sort(); t.dedup(); t };
    for h in &horizons {
        let obs: Vec<f64> = if per_date {
            dates.iter().filter_map(|d| {
                let v: Vec<f64> = basket.iter().filter_map(|t| excess(&px[t], *d, *h)).collect();
                if v.len() < 3 { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) }
            }).collect()
        } else { events.iter().filter_map(|(t, d, _)| excess(&px[t], *d, *h)).collect() };
        if obs.len() < if per_date { 5 } else { 10 } { eprintln!("+{h}д — подій {} , замало для висновку", obs.len()); continue; }
        let m_obs = obs.iter().sum::<f64>() / obs.len() as f64;
        let mut rng = Rng(0x5EED_1234_ABCD_0007 ^ (*h as u64));
        let (mut ge, mut sum) = (0usize, 0.0);
        for _ in 0..iters {
            let mut v: Vec<f64> = Vec::new();
            if per_date {
                // стільки ж ВИПАДКОВИХ ДАТ, скільки спостережених, і так само усереднюємо по кошику
                let m0 = &px[&basket[0]];
                let (lo, hi) = (m0.keys().min().copied().unwrap_or(0), m0.keys().max().copied().unwrap_or(0));
                for _ in 0..obs.len() {
                    if hi - lo <= *h + 1 { continue; }
                    let d = lo + (rng.next() % ((hi - lo - *h) as u64)) as i64;
                    let b: Vec<f64> = basket.iter().filter_map(|t| excess(&px[t], d, *h)).collect();
                    if b.len() >= 3 { v.push(b.iter().sum::<f64>() / b.len() as f64); }
                }
            } else {
                for (t, _, _) in events.iter() {
                    let m = &px[t];
                    let (lo, hi) = (m.keys().min().copied().unwrap_or(0), m.keys().max().copied().unwrap_or(0));
                    if hi - lo <= *h + 1 { continue; }
                    let d = lo + (rng.next() % ((hi - lo - *h) as u64)) as i64;
                    if let Some(x) = excess(m, d, *h) { v.push(x); }
                }
            }
            if v.is_empty() { continue; }
            let m = v.iter().sum::<f64>() / v.len() as f64;
            sum += m;
            if m.abs() >= m_obs.abs() { ge += 1; }
        }
        let m_null = sum / iters as f64;
        let p = (ge as f64 + 1.0) / (iters as f64 + 1.0);
        eprintln!("+{:<8} {:>8} {:>12.2}% {:>12.2}% {:>8.3}", format!("{h}д"), obs.len(), m_obs * 100.0, m_null * 100.0, p);
        rows.push(json!({"horizonDays": h, "events": obs.len(), "meanExcess": m_obs, "meanExcessRandom": m_null, "p": p}));
    }
    // ПОПРАВКА НА МНОЖИННІСТЬ: перевіряємо кілька горизонтів, тож поріг ділиться на їх кількість
    // (Бонферроні). Без цього p=0.03 на одному з чотирьох горизонтів виглядав би відкриттям.
    let thr = 0.05 / rows.len().max(1) as f64;
    let sig_n = rows.iter().filter(|r| r["p"].as_f64().unwrap_or(1.0) <= thr).count();
    let nominal = rows.iter().filter(|r| r["p"].as_f64().unwrap_or(1.0) <= 0.05).count();
    let verdict = if sig_n > 0 { "є горизонти, значущі ПІСЛЯ поправки на множинність" }
        else if nominal > 0 { "номінально значуще є, але поправку на множинність НЕ проходить — гіпотеза, не сигнал" }
        else { "жоден горизонт не значущий → подія НЕ дає передбачення ціни" };
    eprintln!("[sellstudy] поріг із поправкою: {:.4} (горизонтів {})", thr, rows.len());
    eprintln!("[sellstudy] ВЕРДИКТ: {verdict}");
    fs::write(format!("{bf}/sellstudy-{mode}.json"), serde_json::to_string_pretty(&json!({
        "mode": mode, "events": events.len(), "spike": spike, "minUsd": min_usd, "iters": iters,
        "horizons": rows, "verdict": verdict})).unwrap()).ok();
    eprintln!("[sellstudy] збережено sellstudy-{mode}.json");
}
