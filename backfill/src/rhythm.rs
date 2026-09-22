//! rhythm — ТЕСТ РИТМУ ВИПЛАТ: сервіс чи люди?
//!
//! Альтернативна версія, яку ми досі НЕ спростували: рівні суми (32 ETH ×101) дає не лише
//! координована операція, а й стейкінг-пул / майнінг-пул / кастодіальний сервіс / OTC-десек
//! із фіксованим лотом. Розрізняє їх РИТМ, а не суми:
//!   • сервіс     — платить рівномірно, 7 днів на тиждень, цілодобово (Fano≈1, вихідні ≈2/7)
//!   • автомат    — суворо регулярний (Fano<1, інтервали майже однакові)
//!   • люди       — сплесками, у робочі години свого поясу, майже без вихідних (Fano>>1)
//! Заодно даємо ГОДИННИК ОПЕРАТОРА: пік годин у UTC → часовий пояс.
//!
//! Метрики: CV інтервалів (Пуассон≈1), Fano денних лічильників (var/mean), частка вихідних,
//! найкраще 9-годинне вікно (робочий день) і його частка.
//! Вихід: dataset/backfill/rhythm.json + друк. Args: ROOT ADDR_FILE. Env: PAGES(4) CHAIN(1)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
/// `Some(vec)` — валідна відповідь, `None` — стійкий ліміт/мережа.
///
/// Критично саме для цього біна: висновок «робочі години UTC+2…+3» будується на РОЗПОДІЛІ
/// часів доби. Etherscan під лімітом віддає HTTP 200 з `result` у вигляді РЯДКА
/// («Max rate limit reached»), і раніше це давало `Vec::new()` — сторінка мовчки випадала.
/// Втрачені сторінки — це не випадковий шум: ліміт частіше б'є під час найгустішого краулу,
/// тобто вирізає саме ті години, коли адреса найактивніша, і зміщує оцінку часового поясу.
async fn page(client: &reqwest::Client, keys: &[String], kc: &AtomicUsize, chain: u32, a: &str, p: u32) -> Option<Vec<Value>> {
    for attempt in 0..7u64 {
        let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
        let url = format!("https://api.etherscan.io/v2/api?chainid={chain}&module=account&action=txlist&address={a}&page={p}&offset=1000&sort=desc&apikey={key}");
        if let Ok(r) = client.get(&url).send().await {
            let st = r.status().as_u16();
            if st != 429 && st < 500 {
                if let Ok(v) = r.json::<Value>().await {
                    let msg = v["message"].as_str().unwrap_or("");
                    let res = v["result"].as_str().unwrap_or("");
                    let limited = msg == "NOTOK"
                        || res.contains("rate limit")
                        || res.contains("Max calls")
                        || res.contains("Too many");
                    if !limited {
                        if let Some(arr) = v["result"].as_array() { return Some(arr.clone()); }
                        if msg == "No transactions found" { return Some(Vec::new()); }
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400 * (1u64 << attempt.min(6)))).await;
    }
    None
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let afile = std::env::args().nth(2).expect("потрібен файл з адресами");
    let pages: u32 = std::env::var("PAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();

    let addrs: Vec<String> = fs::read_to_string(&afile).expect("addr file").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42).collect();
    let mut out: Vec<Value> = Vec::new();

    // ── ЛОКАЛЬНЕ ДЖЕРЕЛО ──────────────────────────────────────────────────────
    // Повна історія вже зібрана, тож ходити в API немає потреби: один прохід по
    // txfull дає часи ВСІХ виплат усіх адрес одразу. Це не лише швидше — це знімає
    // систематичне зміщення: ліміт Etherscan б'є частіше саме під час густого краулу,
    // вирізаючи найактивніші години й псуючи оцінку часового поясу.
    let use_local = std::env::var("LOCAL").is_ok();
    let mut local_ts: HashMap<String, Vec<u64>> = HashMap::new();
    if use_local {
        let want: std::collections::HashSet<&str> = addrs.iter().map(|s| s.as_str()).collect();
        let bf = format!("{root}/dataset/backfill");
        let f = std::fs::File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull");
        let mut n = 0u64;
        let mut cur = String::new();
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
            n += 1;
            if n % 20_000_000 == 0 { eprintln!("    ...{n} рядків"); }
            // лише ETH-виплати: токенні рядки мають власну логіку сум, а для ритму
            // важливий сам факт дії у часі
            if !line.contains("\"k\":\"ext\"") { continue; }
            let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };
            let from = t["f"].as_str().unwrap_or("").to_lowercase();
            if !want.contains(from.as_str()) { continue; }
            if t["v"].as_str() == Some("0") { continue; }   // службові виклики не є виплатою
            let w = t["w"].as_str().unwrap_or("");
            if w != cur { cur = w.to_string(); seen.clear(); }
            let mut h: u64 = 1469598103934665603;
            for b in t["h"].as_str().unwrap_or("").as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211) }
            if !seen.insert(h) { continue; }
            if let Some(ts) = t["ts"].as_u64() { local_ts.entry(from).or_default().push(ts); }
        }
        eprintln!("[rhythm] локально: адрес із виплатами {}", local_ts.len());
    }

    for a in &addrs {
        let mut ts: Vec<u64> = Vec::new();
        let mut truncated = false;
        if use_local {
            ts = local_ts.get(a).cloned().unwrap_or_default();
        }
        for p in 1..=pages {
            if use_local { break }
            let Some(arr) = page(&client, &keys, &kc, chain, a, p).await else {
                truncated = true;
                break;
            };
            let n = arr.len();
            for t in &arr {
                if t["from"].as_str().unwrap_or("").to_lowercase() != *a { continue; }   // лише ВИПЛАТИ
                if t["value"].as_str() == Some("0") { continue; }
                if let Some(x) = t["timeStamp"].as_str().and_then(|s| s.parse::<u64>().ok()) { ts.push(x); }
            }
            if n < 1000 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        // Часовий пояс з неповної вибірки — це часовий пояс краулу, а не оператора.
        if truncated { eprintln!("[rhythm] {a}: ПРОПУЩЕНО — сторінки обірвав ліміт Etherscan (зібрано {} виплат, розподіл годин зміщений)", ts.len()); continue; }
        if ts.len() < 20 { eprintln!("[rhythm] {a}: замало виплат ({})", ts.len()); continue; }
        ts.sort_unstable();

        // інтервали
        let iv: Vec<f64> = ts.windows(2).map(|w| (w[1] - w[0]) as f64).filter(|x| *x > 0.0).collect();
        let mean_iv = iv.iter().sum::<f64>() / iv.len().max(1) as f64;
        let sd_iv = (iv.iter().map(|x| (x - mean_iv).powi(2)).sum::<f64>() / iv.len().max(1) as f64).sqrt();
        let cv = if mean_iv > 0.0 { sd_iv / mean_iv } else { 0.0 };
        let mut sorted = iv.clone(); sorted.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let med_iv = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);

        // денні лічильники → Fano (var/mean): 1=випадково, <1=регулярно, >>1=сплесками
        let mut per_day: HashMap<u64, u32> = HashMap::new();
        for t in &ts { *per_day.entry(t / 86400).or_insert(0) += 1; }
        let (d0, d1) = (ts[0] / 86400, ts[ts.len() - 1] / 86400);
        let days = (d1 - d0 + 1) as f64;
        let counts: Vec<f64> = (d0..=d1).map(|d| per_day.get(&d).copied().unwrap_or(0) as f64).collect();
        let mean_c = counts.iter().sum::<f64>() / counts.len() as f64;
        let var_c = counts.iter().map(|x| (x - mean_c).powi(2)).sum::<f64>() / counts.len() as f64;
        let fano = if mean_c > 0.0 { var_c / mean_c } else { 0.0 };
        let active_days = per_day.len() as f64 / days;

        // години UTC і дні тижня (1970-01-01 = четвер)
        let mut hours = [0u32; 24];
        let mut dow = [0u32; 7];
        for t in &ts { hours[((t / 3600) % 24) as usize] += 1; dow[(((t / 86400) + 4) % 7) as usize] += 1; }
        let total = ts.len() as f64;
        // найкраще 9-годинне вікно = «робочий день»
        let (mut best_h, mut best_share) = (0usize, 0.0);
        for h in 0..24 {
            let s: u32 = (0..9).map(|k| hours[(h + k) % 24]).sum();
            if s as f64 / total > best_share { best_share = s as f64 / total; best_h = h; }
        }
        let weekend = (dow[0] + dow[6]) as f64 / total;   // нд + сб

        let verdict = if fano < 2.0 && weekend > 0.20 && best_share < 0.55 {
            "РІВНОМІРНО, 7/7, цілодобово → схоже на СЕРВІС (пул/стейкінг/кастодіан)"
        } else if fano > 5.0 && weekend < 0.15 {
            "СПЛЕСКАМИ, майже без вихідних → схоже на ЛЮДЕЙ за столом (десек)"
        } else if fano > 5.0 {
            "СПЛЕСКАМИ, але вихідні робочі → автомат, що запускається епізодично (операції, не сервіс)"
        } else {
            "проміжний ритм — однозначно не розрізняється"
        };
        eprintln!("[rhythm] {a}");
        eprintln!("   виплат {} за {:.0} днів · днів з активністю {:.0}% · медіанний інтервал {:.1} год",
            ts.len(), days, active_days * 100.0, med_iv / 3600.0);
        eprintln!("   CV інтервалів {:.2} (Пуассон≈1) · Fano денний {:.1} (1=випадково, >>1=сплески)", cv, fano);
        eprintln!("   робоче вікно {:02}:00–{:02}:00 UTC тримає {:.0}% виплат · вихідні {:.0}% (рівномірно=29%)",
            best_h, (best_h + 9) % 24, best_share * 100.0, weekend * 100.0);
        eprintln!("   ВЕРДИКТ: {verdict}");
        out.push(json!({"addr": a, "payouts": ts.len(), "days": days, "activeDayShare": active_days,
            "medianIntervalHours": med_iv / 3600.0, "cvIntervals": cv, "fanoDaily": fano,
            "workWindowUtc": format!("{:02}-{:02}", best_h, (best_h + 9) % 24), "workWindowShare": best_share,
            "weekendShare": weekend, "hours": hours.to_vec(), "dow": dow.to_vec(), "verdict": verdict}));
    }
    fs::write(format!("{root}/dataset/backfill/rhythm.json"), serde_json::to_string_pretty(&json!({"chain": chain, "addresses": out})).unwrap()).ok();
    eprintln!("[rhythm] збережено rhythm.json");
}
