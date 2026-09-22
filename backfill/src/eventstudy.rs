//! eventstudy — ЧИ ПЕРЕДБАЧАЄ рух мережі ціну? Подієве дослідження хвиль виплат роздавача.
//!
//! Логіка: роздавач час від часу заряджає десятки гаманців (хвиля). Питання — що відбувається
//! ДАЛІ з кошиком активів самої мережі: +1, +3, +7, +14 днів. Порівнюємо з випадковими датами
//! того ж періоду (нуль-модель), інакше будь-який тренд ринку виглядав би «сигналом».
//! Дохідність рахуємо НАДЛИШКОВУ — мінус ETH за той самий період, щоб не міряти загальний ринок.
//!
//! Вхід: історичні ціни (histprices), активи мережі (cap-real.json), виплати роздавача (краул).
//! Вихід: dataset/backfill/eventstudy.json + друк.
//! Args: ROOT [DISPENSER]. Env: PAGES(8) GAP_H(12) MIN_PAY(5) ITERS(400)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::{AtomicUsize, Ordering};

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
struct Rng(u64);
impl Rng { fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x } }


/// Роздавачі з довідника. Порожньо — це не «немає подій», а незавантажена база,
/// тож кажемо вголос, а не рахуємо по нулю.
fn dispensers_from_store() -> Vec<String> {
    let Ok(c) = wr_store::open() else {
        eprintln!("[eventstudy] сховище недоступне — перелік роздавачів порожній");
        return Vec::new();
    };
    let Ok(mut q) = c.prepare(
        "SELECT DISTINCT address FROM wallet_roles WHERE role='distributor'
         UNION SELECT address FROM wallet_tags WHERE tag='walletrole:dispensers'",
    ) else {
        return Vec::new();
    };
    let v: Vec<String> = q
        .query_map([], |r| r.get(0))
        .map(|it| it.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();
    if v.is_empty() {
        eprintln!("[eventstudy] у базі немає роздавачів — нема за чим стежити");
    }
    v
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    // усі покоління роздавачів — інакше подій замало: нинішній працює лише кілька місяців,
    // а цінове вікно 500 днів захоплює й попереднє покоління
    // Усі покоління роздавачів. Перелік — у базі (тег `walletrole:dispensers`
    // та роль `distributor`), а не в коді: адреси під наглядом не мають лежати
    // в репозиторії. Аргумент командного рядка досі перекриває вибір.
    let disps: Vec<String> = std::env::args().nth(2)
        .map(|a| a.split(',').map(|s| s.trim().to_lowercase()).collect::<Vec<_>>())
        .unwrap_or_else(dispensers_from_store)
        .into_iter().filter(|s| s.len() == 42).collect();
    let disp = disps.join(",");
    let pages: u32 = std::env::var("PAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let gap_h: u64 = std::env::var("GAP_H").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
    let min_pay: usize = std::env::var("MIN_PAY").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let iters: usize = std::env::var("ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(400);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();

    // 1) ціни: контракт -> (день -> ціна)
    let hp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices-hist.json")).expect("token-prices-hist — спершу histprices"))).unwrap();
    let mut px: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for (c, t) in hp["tokens"].as_object().cloned().unwrap_or_default() {
        let mut m: HashMap<i64, f64> = HashMap::new();
        for p in t["prices"].as_array().cloned().unwrap_or_default() {
            if let (Some(ts), Some(v)) = (p[0].as_u64(), p[1].as_f64()) { m.insert((ts / 86400) as i64, v); }
        }
        if m.len() >= 30 { px.insert(c.to_lowercase(), m); }
    }
    let eth = px.get("eth").cloned().unwrap_or_default();
    eprintln!("[eventstudy] цінових рядів: {} · ETH-бенчмарк: {} днів", px.len(), eth.len());

    // 2) кошик мережі: активи групи NET
    let cr: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/cap-real.json")).expect("cap-real"))).unwrap();
    let mut basket: Vec<String> = Vec::new();
    for g in cr["groups"].as_array().cloned().unwrap_or_default() {
        if g["id"].as_str() != Some("NET") { continue; }
        for a in g["assets"].as_array().cloned().unwrap_or_default() {
            if let Some(t) = a["token"].as_str() { let t = t.to_lowercase(); if px.contains_key(&t) { basket.push(t); } }
        }
    }
    basket.sort(); basket.dedup();
    eprintln!("[eventstudy] кошик мережі з цінами: {} активів", basket.len());
    if basket.len() < 3 { eprintln!("[eventstudy] замало активів із цінами — висновок неможливий"); return; }

    // 3) виплати роздавача -> хвилі
    let mut ts_all: Vec<u64> = Vec::new();
    for d in &disps {
        for p in 1..=pages {
            let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
            let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=txlist&address={d}&page={p}&offset=1000&sort=desc&apikey={key}");
            // РЕТРАЇ обовʼязкові: без них один відказ ліміту обриває історію роздавача
            // (через це спершу набралось 404 виплати замість тисяч і подій не було з чого будувати)
            let mut arr: Vec<Value> = Vec::new();
            let mut got = false;
            for attempt in 0..7u64 {
                match client.get(&url).send().await {
                    Ok(r) => {
                        let st = r.status().as_u16();
                        if st != 429 && st < 500 {
                            if let Ok(v) = r.json::<Value>().await {
                                // Ліміт = HTTP 200 з `result` як РЯДОК. Раніше спроби мовчки
                                // вигорали й порожній `arr` обривав пагінацію — рівно та поломка,
                                // через яку набралось 404 виплати замість тисяч.
                                let msg = v["message"].as_str().unwrap_or("");
                                let res = v["result"].as_str().unwrap_or("");
                                let limited = msg == "NOTOK" || res.contains("rate limit")
                                    || res.contains("Max calls") || res.contains("Too many");
                                if !limited {
                                    if let Some(a) = v["result"].as_array() { arr = a.clone(); got = true; break; }
                                    if msg == "No transactions found" { got = true; break; }
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << attempt.min(6)))).await;
            }
            if !got {
                eprintln!("[eventstudy] ⚠ ліміт Etherscan обірвав історію на сторінці {p} — вибірка подій неповна, результат не публікувати");
                std::process::exit(2);
            }
            let len = arr.len();
            for t in &arr {
                if t["from"].as_str().unwrap_or("").to_lowercase() != *d { continue; }
                if t["value"].as_str() == Some("0") { continue; }
                if let Some(x) = t["timeStamp"].as_str().and_then(|s| s.parse::<u64>().ok()) { ts_all.push(x); }
            }
            if len < 1000 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    ts_all.sort_unstable();
    // ПОДІЄВИЙ ДЕНЬ = доба з ≥MIN_PAY виплатами. Кластеризація за розривом злипалась в одну
    // хвилю (виплати щільні), а нам потрібні саме окремі дати для порівняння з випадковими.
    let _ = gap_h;
    let mut per_day: std::collections::BTreeMap<i64, usize> = std::collections::BTreeMap::new();
    for t in &ts_all { *per_day.entry((t / 86400) as i64).or_insert(0) += 1; }
    let mut waves: Vec<(i64, usize)> = per_day.into_iter().filter(|(_, n)| *n >= min_pay).collect();
    waves.sort();
    let (dmin, dmax) = (px[&basket[0]].keys().min().copied().unwrap_or(0), px[&basket[0]].keys().max().copied().unwrap_or(0));
    waves.retain(|(d, _)| *d >= dmin && *d <= dmax - 14);
    eprintln!("[eventstudy] виплат {} → хвиль {} у межах цінових даних ({} … {})", ts_all.len(), waves.len(), dmin, dmax);
    if waves.len() < 5 { eprintln!("[eventstudy] замало хвиль для висновку"); return; }

    // 4) надлишкова дохідність кошика після дати
    let ret = |m: &HashMap<i64, f64>, d: i64, h: i64| -> Option<f64> {
        let (a, b) = (m.get(&d)?, m.get(&(d + h))?);
        if *a <= 0.0 { return None; }
        Some(b / a - 1.0)
    };
    let basket_excess = |d: i64, h: i64| -> Option<f64> {
        let e = ret(&eth, d, h).unwrap_or(0.0);
        let v: Vec<f64> = basket.iter().filter_map(|t| ret(&px[t], d, h)).collect();
        if v.len() < 3 { return None; }
        Some(v.iter().sum::<f64>() / v.len() as f64 - e)
    };
    let horizons = [1i64, 3, 7, 14];
    let mut rows: Vec<Value> = Vec::new();
    eprintln!("[eventstudy] ===== ЩО БУВАЄ ПІСЛЯ ХВИЛІ ВИПЛАТ =====");
    eprintln!("{:<8} {:>12} {:>14} {:>10}", "горизонт", "після хвиль", "випадково", "p");
    for h in horizons {
        let obs: Vec<f64> = waves.iter().filter_map(|(d, _)| basket_excess(*d, h)).collect();
        if obs.len() < 5 { continue; }
        let m_obs = obs.iter().sum::<f64>() / obs.len() as f64;
        let mut rng = Rng(0xA5A5_1234_DEAD_BEEF ^ (h as u64));
        let (mut ge, mut sum) = (0usize, 0.0);
        for _ in 0..iters {
            let mut v: Vec<f64> = Vec::new();
            for _ in 0..obs.len() {
                let d = dmin + (rng.next() % ((dmax - dmin - 14).max(1) as u64)) as i64;
                if let Some(x) = basket_excess(d, h) { v.push(x); }
            }
            if v.is_empty() { continue; }
            let m = v.iter().sum::<f64>() / v.len() as f64;
            sum += m;
            if m.abs() >= m_obs.abs() { ge += 1; }
        }
        let m_null = sum / iters as f64;
        let p = (ge as f64 + 1.0) / (iters as f64 + 1.0);
        eprintln!("+{:<7} {:>11.2}% {:>13.2}% {:>10.3}", format!("{h}д"), m_obs * 100.0, m_null * 100.0, p);
        rows.push(json!({"horizonDays": h, "meanExcessAfterWave": m_obs, "meanExcessRandom": m_null, "p": p, "n": obs.len()}));
    }
    let sig = rows.iter().filter(|r| r["p"].as_f64().unwrap_or(1.0) <= 0.05).count();
    let verdict = if sig == 0 { "жоден горизонт не значущий → хвилі виплат НЕ передбачають ціну кошика" }
        else { "є значущі горизонти — дивись знак: мінус = роздача передує падінню" };
    eprintln!("[eventstudy] ВЕРДИКТ: {verdict}");
    fs::write(format!("{bf}/eventstudy.json"), serde_json::to_string_pretty(&json!({
        "dispenser": disp, "waves": waves.len(), "payouts": ts_all.len(), "basket": basket.len(),
        "gapHours": gap_h, "iters": iters, "horizons": rows, "verdict": verdict})).unwrap()).ok();
    eprintln!("[eventstudy] збережено eventstudy.json");
}
