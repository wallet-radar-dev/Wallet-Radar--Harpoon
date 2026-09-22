//! gasprint — ТЕХНІЧНИЙ ПОЧЕРК: зшиває покоління роздавачів, які НЕ мають спільних адрес.
//! Ідея: адресні зв'язки можна розірвати, але автоматика лишає слід у ПАРАМЕТРАХ транзакцій —
//! ліміт газу, квантування ціни газу, порожній input, темп усередині пачки. Якщо два покоління
//! мають однаковий профіль, це той самий скрипт (той самий оператор), навіть без жодного ребра.
//! Метрики на адресу: топ-значення gas limit і їх частки, к-сть різних цін газу, частка «круглих»
//! цін (кратних 1 gwei), частка порожнього input, медіанний інтервал усередині пачки (<5хв).
//! Схожість пар: косинус за гістограмою gas limit × збіг частки порожнього input.
//! Вихід: dataset/backfill/gasprint.json. Args: ROOT ADDR_FILE. Env: PAGES(6) CHAIN(1)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

/// `Some(vec)` — валідна відповідь, `None` — стійкий ліміт/мережа.
///
/// Тут це особливо болить: раніше запит робився БЕЗ ЖОДНОГО ретраю, а помилка згорталась
/// у `.unwrap_or_default()` → порожній вектор → `len < 1000` → `break`. Залімічена сторінка
/// обривала історію адреси, і та вилітала з аналізу як «замало вихідних (n<30)». Для біна,
/// чия єдина робота — порівнювати ПРОФІЛІ, обрізаний профіль гірший за відсутній: він тихо
/// спотворює гістограму gas limit і всі попарні косинуси.
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
    let afile = std::env::args().nth(2).expect("файл з адресами");
    let pages: u32 = std::env::var("PAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
    let chain: u32 = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());
    let kc = AtomicUsize::new(0);
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();

    let addrs: Vec<String> = fs::read_to_string(&afile).expect("addr file").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42).collect();

    struct P { addr: String, n: usize, gas_hist: HashMap<u64, usize>, empty_input: f64, gp_distinct: usize, gp_round: f64, gp_med: f64, burst_med: f64 }
    let mut profs: Vec<P> = Vec::new();

    for a in &addrs {
        let (mut gas_hist, mut gps, mut ts): (HashMap<u64, usize>, Vec<f64>, Vec<u64>) = (HashMap::new(), Vec::new(), Vec::new());
        let (mut n, mut empty) = (0usize, 0usize);
        let mut truncated = false;
        for p in 1..=pages {
            let Some(arr) = page(&client, &keys, &kc, chain, a, p).await else {
                truncated = true;
                break;
            };
            let len = arr.len();
            for t in &arr {
                if t["from"].as_str().unwrap_or("").to_lowercase() != *a { continue; }   // лише ВИХІДНІ (наш скрипт)
                n += 1;
                if let Some(g) = t["gas"].as_str().and_then(|s| s.parse::<u64>().ok()) { *gas_hist.entry(g).or_insert(0) += 1; }
                if let Some(gp) = t["gasPrice"].as_str().and_then(|s| s.parse::<f64>().ok()) { gps.push(gp); }
                if t["input"].as_str().map(|s| s.len() <= 2).unwrap_or(true) { empty += 1; }
                if let Some(x) = t["timeStamp"].as_str().and_then(|s| s.parse::<u64>().ok()) { ts.push(x); }
            }
            if len < 1000 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        }
        // Обрізаний профіль не порівнюємо ні з чим: косинус за неповною гістограмою gas limit
        // дав би схожість/несхожість, яка відображає глибину краулу, а не поведінку скрипта.
        if truncated { eprintln!("[gasprint] {a}: ПРОПУЩЕНО — історію обірвав ліміт Etherscan (зібрано {n} вихідних, профіль неповний)"); continue; }
        if n < 30 { eprintln!("[gasprint] {a}: замало вихідних ({n})"); continue; }
        ts.sort_unstable();
        let mut gaps: Vec<f64> = ts.windows(2).map(|w| (w[1] - w[0]) as f64).filter(|g| *g > 0.0 && *g < 300.0).collect();
        gaps.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let burst_med = gaps.get(gaps.len() / 2).copied().unwrap_or(0.0);
        let mut gpu: Vec<f64> = gps.clone(); gpu.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let gp_med = gpu.get(gpu.len() / 2).copied().unwrap_or(0.0);
        let distinct = { let mut s: Vec<u64> = gps.iter().map(|g| *g as u64).collect(); s.sort_unstable(); s.dedup(); s.len() };
        let round = gps.iter().filter(|g| (**g as u64) % 1_000_000_000 == 0).count() as f64 / gps.len().max(1) as f64;
        profs.push(P { addr: a.clone(), n, gas_hist, empty_input: empty as f64 / n as f64, gp_distinct: distinct, gp_round: round, gp_med, burst_med });
    }

    eprintln!("[gasprint] ===== ТЕХНІЧНИЙ ПОЧЕРК =====");
    for p in &profs {
        let mut gv: Vec<(&u64, &usize)> = p.gas_hist.iter().collect();
        gv.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<String> = gv.iter().take(3).map(|(g, c)| format!("{g}×{:.0}%", 100.0 * **c as f64 / p.n as f64)).collect();
        eprintln!("   {} · вих.tx {} · gas-limit: {} · різних цін газу {} · «круглих» {:.0}% · медіана {:.1} gwei · крок у пачці {:.0}с · порожній input {:.0}%",
            &p.addr[0..14], p.n, top.join(" "), p.gp_distinct, p.gp_round * 100.0, p.gp_med / 1e9, p.burst_med, p.empty_input * 100.0);
    }

    // косинус за гістограмою gas limit + збіг частки порожнього input
    let cos = |a: &HashMap<u64, usize>, b: &HashMap<u64, usize>| -> f64 {
        let (na, nb): (f64, f64) = (a.values().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt(), b.values().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt());
        if na == 0.0 || nb == 0.0 { return 0.0; }
        a.iter().map(|(k, v)| *v as f64 * b.get(k).copied().unwrap_or(0) as f64).sum::<f64>() / (na * nb)
    };
    let mut pairs: Vec<Value> = Vec::new();
    eprintln!("[gasprint] --- схожість пар (1.0 = ідентичний профіль) ---");
    for i in 0..profs.len() { for j in (i + 1)..profs.len() {
        let c = cos(&profs[i].gas_hist, &profs[j].gas_hist);
        let di = (profs[i].empty_input - profs[j].empty_input).abs();
        let verdict = if c > 0.9 && di < 0.1 { "ТОЙ САМИЙ ПРОФІЛЬ" } else if c > 0.6 { "схожі" } else { "різні" };
        eprintln!("   {} ↔ {} : gas-косинус {:.2} · різниця input {:.0}пп → {}", &profs[i].addr[0..10], &profs[j].addr[0..10], c, di * 100.0, verdict);
        pairs.push(json!({"a": profs[i].addr, "b": profs[j].addr, "gasCosine": c, "inputDelta": di, "verdict": verdict}));
    }}
    let mut profs_json: Vec<Value> = Vec::new();
    for p in &profs {
        let mut v: Vec<(&u64, &usize)> = p.gas_hist.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<Value> = v.into_iter().take(5).map(|(g, c)| json!({"gas": g, "n": c})).collect();
        profs_json.push(json!({"addr": p.addr, "outTx": p.n, "emptyInputShare": p.empty_input,
            "gasPriceDistinct": p.gp_distinct, "gasPriceRoundShare": p.gp_round,
            "gasPriceMedianGwei": p.gp_med / 1e9, "burstStepSec": p.burst_med, "gasLimitTop": top}));
    }
    fs::write(format!("{root}/dataset/backfill/gasprint.json"),
        serde_json::to_string_pretty(&json!({"profiles": profs_json, "pairs": pairs})).unwrap()).ok();
    eprintln!("[gasprint] збережено gasprint.json");
}
