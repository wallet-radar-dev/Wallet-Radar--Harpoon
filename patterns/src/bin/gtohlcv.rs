//! gtohlcv — ОНЧЕЙН-ЦІНОВИЙ ФІД для дрібняку (GeckoTerminal, безкоштовно, без ключів).
//! Для кожного каскад-токена: топ-пул → денний OHLCV (~180 днів) → ціна ЦІЛЬОВОГО токена (token=addr, usd).
//! Покриває small-cap цілі ротації, яких немає в DefiLlama/Binance. Rate-limit ~30/хв → пауза між токенами.
//! Вхід: cascade-curves.json (список токенів). Вихід: token-prices-onchain.json {tokens:{contract:{symbol,prices:[[ts,close]]}}}.
//! Args: ROOT. Env: NET(eth) SLEEP_MS(2500) MAX(400).
use serde_json::{json, Value};
use std::time::Duration;

const GT: &str = "https://api.geckoterminal.com/api/v2";

fn get(url: &str) -> Option<Value> {
    for attempt in 0..4 {
        match ureq::get(url).timeout(Duration::from_secs(25)).call() {
            Ok(r) => return r.into_json::<Value>().ok(),
            Err(ureq::Error::Status(429, _)) => std::thread::sleep(Duration::from_millis(4000 * (attempt + 1))),
            Err(_) => std::thread::sleep(Duration::from_millis(1500)),
        }
    }
    None
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let net = std::env::var("NET").unwrap_or_else(|_| "eth".into());
    let sleep = std::env::var("SLEEP_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(4500u64); // ~26/хв під ліміт GT
    let max: usize = std::env::var("MAX").ok().and_then(|s| s.parse().ok()).unwrap_or(400);
    let outpath = format!("{bf}/token-prices-onchain.json");

    // токени з cascade-curves (усі з подіями) — пріоритет тим, кого нема в hourly
    let cv: Value = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(format!("{bf}/cascade-curves.json")).expect("cascade-curves.json"))).unwrap();
    let mut toks: Vec<(String, String)> = cv["tokens"].as_array().cloned().unwrap_or_default().iter()
        .filter_map(|t| Some((t["token"].as_str()?.to_lowercase(), t["symbol"].as_str().unwrap_or("").to_string())))
        .collect();
    toks.truncate(max);

    // RESUME: підвантажити вже зібране, пропускати
    let mut out = std::fs::read_to_string(&outpath).ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["tokens"].as_object().cloned())
        .unwrap_or_default();
    let before = out.len();
    eprintln!("[gtohlcv] токенів на збір: {} (net={net}); вже маємо {before} (resume)", toks.len());
    let write = |m: &serde_json::Map<String, Value>| {
        let _ = std::fs::write(&outpath, serde_json::to_string(&json!({"network": net, "source": "geckoterminal", "tokens": m})).unwrap());
    };

    let (mut ok, mut i) = (0usize, 0usize);
    for (addr, sym) in &toks {
        i += 1;
        if out.contains_key(addr) { continue } // resume: пропустити зібране
        // 1) топ-пул токена
        let pools = get(&format!("{GT}/networks/{net}/tokens/{addr}/pools?page=1"));
        let pool = pools.as_ref().and_then(|v| v["data"].as_array())
            .and_then(|a| a.first()).and_then(|p| p["attributes"]["address"].as_str().map(String::from));
        let Some(pool) = pool else { std::thread::sleep(Duration::from_millis(sleep)); continue };
        // 2) денний OHLCV ціни цільового токена
        let oh = get(&format!("{GT}/networks/{net}/pools/{pool}/ohlcv/day?limit=1000&token={addr}&currency=usd"));
        if let Some(list) = oh.as_ref().and_then(|v| v["data"]["attributes"]["ohlcv_list"].as_array()) {
            let prices: Vec<Value> = list.iter().filter_map(|c| {
                let a = c.as_array()?; Some(json!([a[0].as_i64()?, a[4].as_f64()?])) }).collect();
            if prices.len() >= 20 {
                out.insert(addr.clone(), json!({"symbol": sym, "prices": prices}));
                ok += 1;
            }
        }
        if i % 15 == 0 { write(&out); eprintln!("[gtohlcv] {i}/{} · нових {ok} · усього {}", toks.len(), out.len()); }
        std::thread::sleep(Duration::from_millis(sleep));
    }
    write(&out);
    eprintln!("[gtohlcv] збережено token-prices-onchain.json (нових {ok}, усього {})", out.len());
}
