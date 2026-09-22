//! tokliq — РЕАЛЬНА ЛІКВІДНІСТЬ токенів (GeckoTerminal, безкоштовно, без ключа).
//! Причина: паперова оцінка «net-холдинг × ціна» бреше не лише через криві ціни, а принципово —
//! у пулах COMP лежить $2.4М ліквідності й $8k обороту за добу, тож «група тримає COMP на $8.2 млрд»
//! неможливо перетворити на гроші. Реальний капітал обмежений тим, що ЗАЛИТО в токен.
//! Бере: total_reserve_in_usd (скільки грошей у пулах), volume_usd.h24 (реальний оборот),
//! price_usd, market_cap_usd, fdv_usd.
//! Вихід: dataset/backfill/token-liquidity.json {contract:{symbol,price,reserveUsd,vol24,mcap,fdv}}
//! Вхід — ОБ'ЄДНАННЯ двох джерел: onchain-prices.json і контракти активів груп
//! капіталу (group-assets.json). Друге додано тому, що портфель зважується саме
//! по активах груп, а самих лише onchain-prices вистачало на 8 позицій із 70:
//! решта отримувала «немає даних», що не те саме, що «немає ліквідності».
//! Args: ROOT. Env: NET(eth) BATCH(30) SLEEP_MS(2500 — ліміт 30 запитів/хв)
use serde_json::{json, Value};
use std::fs::File;
use std::io::BufReader;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let net = std::env::var("NET").unwrap_or_else(|_| "eth".into());
    let batch: usize = std::env::var("BATCH").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    let sleep_ms: u64 = std::env::var("SLEEP_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(2500);
    let bf = format!("{root}/dataset/backfill");

    let onchain: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).expect("onchain-prices"))).unwrap_or_default();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut contracts: Vec<String> = Vec::new();
    for k in onchain.keys() {
        let c = k.to_lowercase();
        if seen.insert(c.clone()) { contracts.push(c) }
    }
    let from_prices = contracts.len();

    // Активи груп капіталу — те, з чого будується портфель. Без них ліквідність
    // більшості позицій просто відсутня, і фільтр у portfolio.rs не має на чому працювати.
    let ga_path = format!("{root}/dataset/group-assets.json");
    let mut from_groups = 0usize;
    if let Ok(txt) = std::fs::read_to_string(&ga_path) {
        if let Ok(v) = serde_json::from_str::<Value>(&txt) {
            for g in v["groups"].as_array().unwrap_or(&vec![]) {
                for a in g["assets"].as_array().unwrap_or(&vec![]) {
                    if let Some(c) = a["contract"].as_str() {
                        let c = c.to_lowercase();
                        if c.starts_with("0x") && c.len() == 42 && seen.insert(c.clone()) {
                            contracts.push(c);
                            from_groups += 1;
                        }
                    }
                }
            }
        }
    } else {
        eprintln!("[tokliq] УВАГА: {ga_path} не прочитано — покриття буде лише з onchain-prices");
    }
    eprintln!("[tokliq] токенів до опитування: {} (з onchain-prices {from_prices}, з груп капіталу +{from_groups}; мережа {net}, пачка {batch})", contracts.len());

    let client = reqwest::Client::builder().user_agent("wallet-radar-tokliq/0.1").timeout(std::time::Duration::from_secs(40)).build().unwrap();
    let mut out: serde_json::Map<String, Value> = serde_json::Map::new();
    let (mut done, mut miss) = (0usize, 0usize);
    let num = |v: &Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()).unwrap_or(0.0);

    for (i, chunk) in contracts.chunks(batch).enumerate() {
        let url = format!("https://api.geckoterminal.com/api/v2/networks/{net}/tokens/multi/{}", chunk.join(","));
        // GeckoTerminal ріже за rate-limit (порожня відповідь / 429) — ретрай із наростанням паузи
        let mut arr = Vec::new();
        for attempt in 0..4 {
            let resp = client.get(&url).send().await;
            let (status, v): (u16, Value) = match resp {
                Ok(r) => { let s = r.status().as_u16(); (s, r.json().await.unwrap_or(json!({}))) }
                Err(_) => (0, json!({})),
            };
            arr = v["data"].as_array().cloned().unwrap_or_default();
            if !arr.is_empty() { break; }
            eprintln!("[tokliq] пачка {i}: порожньо (HTTP {status}), спроба {}", attempt + 1);
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms * (attempt + 2) as u64)).await;
        }
        if arr.is_empty() { miss += chunk.len(); }
        for d in &arr {
            let a = &d["attributes"];
            let Some(addr) = a["address"].as_str() else { continue };
            out.insert(addr.to_lowercase(), json!({
                "symbol": a["symbol"].as_str().unwrap_or(""),
                "price": num(&a["price_usd"]),
                "reserveUsd": num(&a["total_reserve_in_usd"]),
                "vol24": num(&a["volume_usd"]["h24"]),
                "mcap": num(&a["market_cap_usd"]),
                "fdv": num(&a["fdv_usd"]),
            }));
            done += 1;
        }
        if i % 5 == 0 { eprintln!("[tokliq] {}/{} пачок · знайдено {done}", i + 1, contracts.len().div_ceil(batch)); }
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
    }

    // діагностика: де ліквідність мізерна проти «капіталізації»
    let mut rows: Vec<(String, f64, f64, f64)> = out.values().map(|t| (
        t["symbol"].as_str().unwrap_or("?").to_string(), t["reserveUsd"].as_f64().unwrap_or(0.0),
        t["vol24"].as_f64().unwrap_or(0.0), t["mcap"].as_f64().unwrap_or(0.0))).collect();
    rows.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());
    std::fs::write(format!("{bf}/token-liquidity.json"),
        serde_json::to_string_pretty(&json!({"network": net, "asOf": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), "source": "geckoterminal", "tokens": out})).unwrap()).ok();

    eprintln!("[tokliq] отримано {done}, без даних {miss}");
    eprintln!("[tokliq] --- топ-15 за капіталізацією: скільки РЕАЛЬНО в пулах ---");
    eprintln!("     {:<10} {:>16} {:>16} {:>14}  {}", "тікер", "mcap", "ліквідність", "оборот 24г", "ліквідн./mcap");
    for (s, r, v, m) in rows.iter().take(15) {
        eprintln!("     {:<10} {:>16.0} {:>16.0} {:>14.0}  {:.4}%", s, m, r, v, if *m > 0.0 { r / m * 100.0 } else { 0.0 });
    }
    eprintln!("[tokliq] збережено token-liquidity.json");
}
