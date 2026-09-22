//! histprices — ІСТОРИЧНІ денні ціни токенів (DefiLlama /chart, безкоштовно, без ключа).
//! Навіщо: у нас лише поточні ціни, тож перевірити «чи рух мережі щось передбачає» неможливо —
//! нема на чому міряти наслідок. Це передумова подієвого дослідження (eventstudy).
//! Вихід: dataset/backfill/token-prices-hist.json {contract:{sym,prices:[[ts,price],…]}} (крок 1 день)
//! Args: ROOT. Env: SPAN(730 днів) BATCH(12) SLEEP_MS(350) ONLY_LIQ(1 — лише токени з ліквідністю)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    // DefiLlama віддає максимум 500 точок за запит («Requested N data points exceeds the maximum of 500»)
    let span: u32 = std::env::var("SPAN").ok().and_then(|s| s.parse().ok()).unwrap_or(500);
    // Стелі тут бути не може: ліміт 500 точок стосується ОДНОГО запиту, а глибина
    // нарізається вікнами по WINDOW днів нижче. Обрізання span до 500 знецінювало
    // всю віконну логіку — ряди покривали лише останні ~1.4 роки, і аналіз за
    // 2020-2023 мовчки виходив порожнім.
    // ліміт 500 точок рахується на ВЕСЬ запит, тож при span=500 можна лише ОДИН токен за раз
    let batch: usize = std::env::var("BATCH").ok().and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
    let sleep_ms: u64 = std::env::var("SLEEP_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(350);
    let only_liq = std::env::var("ONLY_LIQ").map(|v| v != "0").unwrap_or(true);
    let bf = format!("{root}/dataset/backfill");

    // токени: з ліквідністю (тобто ті, що реально торгуються) або весь priced-всесвіт
    let mut toks: Vec<String> = Vec::new();
    // TOKENS_FILE — явний список активів. Потрібен, бо вибір «за ліквідністю СЬОГОДНІ»
    // губить те, що торгувалось у 2021-му й відтоді померло: саме такі активи й цікаві,
    // коли питання про минулий цикл.
    let tfile = std::env::var("TOKENS_FILE").unwrap_or_default();
    if !tfile.is_empty() {
        for l in std::fs::read_to_string(&tfile).expect("TOKENS_FILE").lines() {
            let c = l.trim().to_lowercase();
            if c.starts_with("0x") { toks.push(c); }
        }
    }
    if toks.is_empty() && only_liq {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/token-liquidity.json")).expect("token-liquidity"))) {
            if let Some(m) = v["tokens"].as_object() {
                for (k, t) in m { if t["reserveUsd"].as_f64().unwrap_or(0.0) > 1000.0 { toks.push(k.to_lowercase()); } }
            }
        }
    }
    if toks.is_empty() && tfile.is_empty() {
        let m: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())).unwrap_or_default();
        toks = m.keys().map(|k| k.to_lowercase()).collect();
    }
    toks.sort(); toks.dedup();
    let start = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() - (span as u64 * 86400);
    eprintln!("[histprices] токенів: {} · глибина {span} днів (з {start})", toks.len());

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build().unwrap();
    // ЗЛИТТЯ з наявним файлом, а не перезапис: прогін по вузькому списку інакше
    // стирав би всі раніше зібрані ряди (їх 253) і мовчки обнуляв чужі дослідження.
    let mut out: serde_json::Map<String, Value> = std::fs::File::open(format!("{bf}/token-prices-hist.json"))
        .ok()
        .and_then(|f| serde_json::from_reader::<_, Value>(BufReader::new(f)).ok())
        .and_then(|v| v["tokens"].as_object().cloned())
        .unwrap_or_default();
    eprintln!("[histprices] у файлі вже рядів: {}", out.len());
    let (mut ok, mut miss) = (0usize, 0usize);
    // DefiLlama віддає щонайбільше 500 точок на запит: span=1600 повертав
    // {"message":"Requested 1600 data points exceeds the maximum of 500."} і 0 рядів.
    // Тому глибина ріжеться на вікна по WINDOW днів, а ряди склеюються за часом.
    const WINDOW: u32 = 450;
    let mut acc: HashMap<String, (String, Vec<(u64, f64)>)> = HashMap::new();
    let n_win = span.div_ceil(WINDOW);
    for (i, chunk) in toks.chunks(batch).enumerate() {
        let ids: Vec<String> = chunk.iter().map(|c| format!("ethereum:{c}")).collect();
        let mut got_any = false;
        for wi in 0..n_win {
            let w_span = WINDOW.min(span - wi * WINDOW);
            let w_start = start + (wi * WINDOW) as u64 * 86400;
            let url = format!("https://coins.llama.fi/chart/{}?start={w_start}&span={w_span}&period=1d&searchWidth=6h", ids.join(","));
            for attempt in 0..3 {
                let v: Value = match client.get(&url).send().await { Ok(r) => r.json().await.unwrap_or(json!({})), Err(_) => json!({}) };
                if let Some(coins) = v["coins"].as_object() {
                    if !coins.is_empty() {
                        for (k, c) in coins {
                            let Some(addr) = k.split(':').nth(1) else { continue };
                            let e = acc.entry(addr.to_lowercase())
                                .or_insert_with(|| (c["symbol"].as_str().unwrap_or("").to_string(), Vec::new()));
                            for p in c["prices"].as_array().cloned().unwrap_or_default() {
                                if let (Some(t), Some(pr)) = (p["timestamp"].as_u64(), p["price"].as_f64()) {
                                    e.1.push((t, pr));
                                }
                            }
                        }
                        got_any = true;
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms * (attempt + 2) as u64)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        }
        if !got_any { miss += chunk.len(); }
        if i % 5 == 0 { eprintln!("[histprices] {}/{} пачок · токенів із рядом {}", i + 1, toks.len().div_ceil(batch), acc.len()); }
    }
    for (addr, (sym, mut pts)) in acc {
        pts.sort_by_key(|p| p.0);
        pts.dedup_by_key(|p| p.0);
        if pts.len() < 10 { continue; }
        let prices: Vec<Value> = pts.iter().map(|(t, p)| json!([t, p])).collect();
        out.insert(addr, json!({"sym": sym, "prices": prices}));
        ok += 1;
    }
    // ETH окремо — потрібен як бенчмарк ринку. Теж ВІКНАМИ: одним запитом на 1600 днів
    // DefiLlama відмовляє (максимум 500 точок), і ряд мовчки покривав би лише останній рік,
    // а всі давніші операції оцінювались би найдавнішою відомою ціною.
    let mut eth_pts: Vec<(u64, f64)> = Vec::new();
    for wi in 0..n_win {
        let w_span = WINDOW.min(span - wi * WINDOW);
        let w_start = start + (wi * WINDOW) as u64 * 86400;
        if let Ok(r) = client.get(format!("https://coins.llama.fi/chart/coingecko:ethereum?start={w_start}&span={w_span}&period=1d")).send().await {
            if let Ok(v) = r.json::<Value>().await {
                if let Some(c) = v["coins"]["coingecko:ethereum"].as_object() {
                    for p in c["prices"].as_array().cloned().unwrap_or_default() {
                        if let (Some(t), Some(pr)) = (p["timestamp"].as_u64(), p["price"].as_f64()) {
                            eth_pts.push((t, pr));
                        }
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
    }
    eth_pts.sort_by_key(|p| p.0);
    eth_pts.dedup_by_key(|p| p.0);
    if !eth_pts.is_empty() {
        let prices: Vec<Value> = eth_pts.iter().map(|(t, p)| json!([t, p])).collect();
        eprintln!("[histprices] ETH-ряд: {} точок", prices.len());
        out.insert("eth".into(), json!({"sym": "ETH", "prices": prices}));
    }
    std::fs::write(format!("{bf}/token-prices-hist.json"), serde_json::to_string(&json!({"span": span, "tokens": out})).unwrap()).ok();
    eprintln!("[histprices] ГОТОВО: рядів {ok}, без даних {miss}; ETH-бенчмарк {}",
        if out.contains_key("eth") { "є" } else { "НЕМА" });
}
