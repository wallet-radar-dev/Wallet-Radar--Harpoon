//! entrylag — ХТО ВЕДЕ, А ХТО ПОВТОРЮЄ: лід-лаг першого входу в актив, з точністю до хвилин.
//!
//! Наша попередня нуль-модель працювала на ДОБОВИХ інтервалах — на такій роздільності координація
//! і випадковий збіг виглядають однаково. Повна транзакційна база дає точний час, тож питання
//! ставиться інакше: чи є пари гаманців, де один СТАБІЛЬНО заходить в актив раніше за іншого?
//!
//! Метод: для кожного гаманця — час ПЕРШОГО отримання кожного токена. Для пари, що ділить
//! ≥MIN_SHARED токенів, рахуємо медіанний лаг і УЗГОДЖЕНІСТЬ ЗНАКУ (як часто A попереду B).
//! Випадкові пари дають узгодженість ~50%; ведучий-повторювач дає значно більше.
//! ⚠️ ПЕРША ВЕРСІЯ БУЛА ХИБНОЮ. Вона міряла узгодженість знаку лагу й дала 0.920 проти 0.769
//! (p=0.005) — але медіанні лаги виявились 90–190 ДНІВ. Це не координація, а РІЗНИЦЯ ВІКУ:
//! гаманець 2021 року заходить у кожен токен раніше за гаманець 2024-го зі 100% узгодженістю
//! і без жодної змови. Перемішування міток вік не контролює.
//! ПРАВИЛЬНА МІРА: кількість входів у ТІСНОМУ вікні (година / доба). Нуль-модель зсуває часи
//! КОЖНОГО гаманця на випадкову величину ±SHIFT_D днів — вік і активність зберігаються,
//! руйнується саме синхронність. Плюс контракти виключаються (у топі «ведучих» був роутер 1inch).
//!
//! Токени, які тримають майже всі (USDT тощо), виключаються: спільне тримання ≠ координація.
//! Вихід: dataset/backfill/entrylag-<chain>.json
//! Args: ROOT. Env: CHAIN(1) MIN_SHARED(3) MAX_HOLDERS(40) MIN_USD(1000) ITERS(200) TOPN(25)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

struct Rng(u64);
impl Rng { fn next(&mut self) -> u64 { let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x } }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let min_shared: usize = std::env::var("MIN_SHARED").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let max_holders: usize = std::env::var("MAX_HOLDERS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(1000.0);
    let iters: usize = std::env::var("ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let topn: usize = std::env::var("TOPN").ok().and_then(|s| s.parse().ok()).unwrap_or(25);
    let bf = format!("{root}/dataset/backfill");

    let tight: Vec<u64> = std::env::var("TIGHT_H").unwrap_or_else(|_| "1,24".into())
        .split(',').filter_map(|s| s.trim().parse::<u64>().ok()).map(|h| h * 3600).collect();
    let shift_d: u64 = std::env::var("SHIFT_D").ok().and_then(|s| s.parse().ok()).unwrap_or(90);
    // контракти не є учасниками координації
    let flags: HashMap<String, bool> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/contract-flags.json")).unwrap_or_else(|_| File::open("/dev/null").unwrap()))).unwrap_or_default();
    let mut kindm: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kindm.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let coded: std::collections::HashSet<String> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/eth-balances.json")).unwrap()))
        .ok().and_then(|v| v["wallets"].as_object().map(|m| m.iter().filter(|(_, x)| x["code"] == json!(true)).map(|(k, _)| k.to_lowercase()).collect())).unwrap_or_default();
    let is_contract = |a: &str| flags.get(a).copied().unwrap_or(false) || coded.contains(a)
        || matches!(kindm.get(a).map(|s| s.as_str()), Some("contract") | Some("exchange") | Some("otc"));

    let px: HashMap<String, f64> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/token-prices.json")).unwrap())).ok()
        .and_then(|v| v["tokens"].as_object().map(|m| m.iter().filter_map(|(k, x)| x["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect())).unwrap_or_default();

    // перший вхід: токен -> гаманець -> час
    let mut first: HashMap<String, HashMap<String, u64>> = HashMap::new();
    let mut rows = 0u64;
    for line in BufReader::with_capacity(1 << 22, File::open(format!("{bf}/txfull-{chain}.jsonl")).expect("txfull")).lines().map_while(Result::ok) {
        rows += 1;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["k"].as_str() != Some("tok") || v["err"].as_str() == Some("1") { continue; }
        let w = v["w"].as_str().unwrap_or("").to_string();
        if v["t"].as_str().unwrap_or("") != w { continue; }              // саме ОТРИМАННЯ
        if is_contract(&w) { continue; }
        let tok = v["tok"].as_str().unwrap_or("").to_lowercase();
        let Some(p) = px.get(&tok) else { continue };                    // лише токени з ціною
        let dec: i32 = v["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
        let raw: f64 = v["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if raw / 10f64.powi(dec) * p < min_usd { continue; }             // відсікаємо пил і ейрдропи
        let ts = v["ts"].as_u64().unwrap_or(0);
        if ts == 0 { continue; }
        let e = first.entry(tok).or_default().entry(w).or_insert(ts);
        if ts < *e { *e = ts; }
        if rows % 20_000_000 == 0 { eprintln!("[entrylag] {rows} рядків, токенів {}", first.len()); }
    }
    // токени, які тримають майже всі — не сигнал
    first.retain(|_, h| h.len() >= 2 && h.len() <= max_holders);
    eprintln!("[entrylag] рядків {rows} · придатних токенів {} (2..{max_holders} власників)", first.len());

    // пари: медіанний лаг і узгодженість знаку
    let mut pairs: HashMap<(String, String), Vec<i64>> = HashMap::new();
    for (_t, h) in &first {
        let ws: Vec<(&String, &u64)> = h.iter().collect();
        for i in 0..ws.len() {
            for j in (i + 1)..ws.len() {
                let (a, b) = if ws[i].0 < ws[j].0 { (ws[i], ws[j]) } else { (ws[j], ws[i]) };
                pairs.entry((a.0.clone(), b.0.clone())).or_default().push(*a.1 as i64 - *b.1 as i64);
            }
        }
    }
    pairs.retain(|_, v| v.len() >= min_shared);
    eprintln!("[entrylag] пар зі спільними ≥{min_shared} токенами: {}", pairs.len());
    if pairs.is_empty() { return; }

    // ТІСНІ СПІВВХОДИ: скільки разів пара зайшла в той самий актив у межах вікна
    let tight_count = |v: &Vec<i64>, w: u64| -> usize { v.iter().filter(|x| x.unsigned_abs() <= w).count() };
    let stat = |v: &Vec<i64>| -> (f64, f64, i64) {
        let n = v.len() as f64;
        let ahead = v.iter().filter(|x| **x < 0).count() as f64;
        let cons = (ahead / n).max(1.0 - ahead / n);
        let mut s: Vec<i64> = v.iter().map(|x| x.abs()).collect();
        s.sort_unstable();
        (cons, n, s[s.len() / 2])
    };
    let mut rows_out: Vec<Value> = pairs.iter().map(|((a, b), v)| {
        let (cons, n, med) = stat(v);
        json!({"a": a, "b": b, "shared": n as u64, "consistency": cons, "medianLagMin": med / 60,
               "leader": if v.iter().filter(|x| **x < 0).count() as f64 > n / 2.0 { a } else { b }})
    }).collect();
    rows_out.sort_by(|x, y| {
        let k = |r: &Value| (r["consistency"].as_f64().unwrap_or(0.0), r["shared"].as_u64().unwrap_or(0) as f64);
        k(y).partial_cmp(&k(x)).unwrap()
    });

    // ГОЛОВНА МІРА: тісні співвходи. Нуль-модель зсуває часи КОЖНОГО гаманця на випадкову
    // величину ±SHIFT_D днів — вік і активність зберігаються, руйнується лише синхронність.
    // Без цього контролю різниця у віці гаманців сама давала «100% лідерства» з лагом у 190 днів.
    let obs: Vec<usize> = tight.iter().map(|w| pairs.values().map(|v| tight_count(v, *w)).sum()).collect();
    let mut wallets: Vec<String> = first.values().flat_map(|h| h.keys().cloned()).collect();
    wallets.sort(); wallets.dedup();
    let mut rng = Rng(0xC0FFEE_1234_5678);
    let mut ge = vec![0usize; tight.len()];
    let mut nullsum = vec![0f64; tight.len()];
    let span = (2 * shift_d * 86400).max(1);
    for _ in 0..iters {
        let shift: HashMap<&String, i64> = wallets.iter()
            .map(|w| (w, (rng.next() % span) as i64 - (shift_d * 86400) as i64)).collect();
        let mut pp: HashMap<(String, String), Vec<i64>> = HashMap::new();
        for (_t, h) in &first {
            let ws: Vec<(&String, &u64)> = h.iter().collect();
            for i in 0..ws.len() {
                for j in (i + 1)..ws.len() {
                    let (a, b) = if ws[i].0 < ws[j].0 { (ws[i], ws[j]) } else { (ws[j], ws[i]) };
                    let ta = *a.1 as i64 + shift.get(a.0).copied().unwrap_or(0);
                    let tb = *b.1 as i64 + shift.get(b.0).copied().unwrap_or(0);
                    pp.entry((a.0.clone(), b.0.clone())).or_default().push(ta - tb);
                }
            }
        }
        pp.retain(|_, v| v.len() >= min_shared);
        for (k, w) in tight.iter().enumerate() {
            let c: usize = pp.values().map(|v| tight_count(v, *w)).sum();
            nullsum[k] += c as f64;
            if c >= obs[k] { ge[k] += 1; }
        }
    }

    eprintln!("[entrylag] ===== СИНХРОННІСТЬ ВХОДУ (вік проконтрольовано) =====");
    eprintln!("{:<10} {:>14} {:>14} {:>8}", "вікно", "спостережено", "випадково", "p");
    let mut wins: Vec<Value> = Vec::new();
    for (k, w) in tight.iter().enumerate() {
        let nm = nullsum[k] / iters as f64;
        let p = (ge[k] as f64 + 1.0) / (iters as f64 + 1.0);
        eprintln!("{:<10} {:>14} {:>14.1} {:>8.3}", format!("≤{} год", w / 3600), obs[k], nm, p);
        wins.push(json!({"windowHours": w / 3600, "observed": obs[k], "null": nm, "p": p}));
    }

    // найтісніші пари за вікном 24 год
    let w24 = *tight.last().unwrap_or(&86400);
    let mut tp: Vec<(&(String, String), usize, i64)> = pairs.iter()
        .map(|(k, v)| { let mut a: Vec<i64> = v.iter().map(|x| x.abs()).collect(); a.sort_unstable();
                        (k, tight_count(v, w24), a[a.len() / 2] / 60) })
        .filter(|x| x.1 > 0).collect();
    tp.sort_by(|x, y| y.1.cmp(&x.1));
    eprintln!("[entrylag] --- пари з найбільшою к-стю тісних співвходів ---");
    for ((a, b), c, med) in tp.iter().take(topn.min(12)) {
        eprintln!("   {} ↔ {} · тісних {} · медіанний лаг {} хв", &a[0..12], &b[0..12], c, med);
    }
    let sig = wins.iter().filter(|w| w["p"].as_f64().unwrap_or(1.0) <= 0.05 / tight.len() as f64).count();
    let verdict = if sig > 0 { "синхронність вища за випадкову ПІСЛЯ контролю віку → координація підтверджується" }
        else { "тісних співвходів не більше, ніж дає випадковість при тому ж віці гаманців" };
    eprintln!("[entrylag] ВЕРДИКТ: {verdict}");
    std::fs::write(format!("{bf}/entrylag-{chain}.json"), serde_json::to_string_pretty(&json!({
        "chain": chain, "tokens": first.len(), "pairs": pairs.len(), "shiftDays": shift_d,
        "windows": wins, "verdict": verdict,
        "topTight": tp.iter().take(100).map(|((a, b), c, m)| json!({"a": a, "b": b, "tight": c, "medianLagMin": m})).collect::<Vec<Value>>()})).unwrap()).ok();
    eprintln!("[entrylag] збережено entrylag-{chain}.json");
}
