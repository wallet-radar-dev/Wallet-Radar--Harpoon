//! tokprices — реальні ринкові ціни токенів з DefiLlama (той самий канал, що вже юзає застосунок,
//! rust-server/src/prices.rs). ПРИЧИНА: onchain-prices.json (ціни зі свопів) містить артефакти —
//! COMP там $41 046 замість ~$17 (×2350), окремі токени — трильйони. Будь-який USD-підсумок на них
//! брехливий, тому всі нові біни (cexcrawl/leadlag/cexsignal) мають брати ціну ЗВІДСИ, а на
//! onchain-prices відкочуватись лише коли DefiLlama не знає токен І ціна виглядає осудною.
//! Вихід: dataset/backfill/token-prices.json {"eth":usd,"asOf":ts,"tokens":{contract:{usd,symbol,conf}}}
//! Args: ROOT. Env: BATCH(50) MIN_CONF(0.5)
use serde_json::{json, Value};
use std::fs::File;
use std::io::BufReader;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let batch: usize = std::env::var("BATCH").ok().and_then(|s| s.parse().ok()).unwrap_or(50);
    let min_conf: f64 = std::env::var("MIN_CONF").ok().and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let bf = format!("{root}/dataset/backfill");

    let onchain: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).expect("onchain-prices"))).unwrap_or_default();
    let contracts: Vec<String> = onchain.keys().map(|k| k.to_lowercase()).collect();
    eprintln!("[tokprices] токенів у priced-всесвіті: {}", contracts.len());

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap();
    let mut tokens: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut hit = 0usize;
    for chunk in contracts.chunks(batch) {
        let ids: Vec<String> = chunk.iter().map(|c| format!("ethereum:{c}")).collect();
        let url = format!("https://coins.llama.fi/prices/current/{}", ids.join(","));
        let v: Value = match client.get(&url).send().await { Ok(r) => r.json().await.unwrap_or(json!({})), Err(_) => json!({}) };
        if let Some(coins) = v["coins"].as_object() {
            for (k, c) in coins {
                let Some(addr) = k.split(':').nth(1) else { continue };
                let (Some(p), conf) = (c["price"].as_f64(), c["confidence"].as_f64().unwrap_or(1.0)) else { continue };
                if p <= 0.0 || conf < min_conf { continue; }
                tokens.insert(addr.to_lowercase(), json!({"usd": p, "symbol": c["symbol"].as_str().unwrap_or(""), "conf": conf}));
                hit += 1;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    let ethv: Value = client.get("https://coins.llama.fi/prices/current/coingecko:ethereum").send().await
        .ok().unwrap().json().await.unwrap_or(json!({}));
    let eth = ethv["coins"]["coingecko:ethereum"]["price"].as_f64().unwrap_or(0.0);

    // діагностика: наскільки бреше ончейн-ціна
    let mut worst: Vec<(String, f64, f64, f64)> = Vec::new();
    for (c, t) in &tokens {
        let real = t["usd"].as_f64().unwrap_or(0.0);
        let oc = onchain.get(c).and_then(|v| v["usd"].as_f64()).unwrap_or(0.0);
        if real > 0.0 && oc > 0.0 { let r = oc / real; if r > 3.0 || r < 0.33 { worst.push((t["symbol"].as_str().unwrap_or("?").to_string(), oc, real, r)); } }
    }
    worst.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    std::fs::write(format!("{bf}/token-prices.json"),
        serde_json::to_string_pretty(&json!({"eth": eth, "asOf": now, "source": "defillama", "tokens": tokens})).unwrap()).ok();

    eprintln!("[tokprices] ціни отримано: {hit}/{} · ETH ${:.2}", contracts.len(), eth);
    eprintln!("[tokprices] --- де ончейн-ціна бреше (топ-15 за перекосом) ---");
    for (s, oc, real, r) in worst.iter().take(15) { eprintln!("     {:<10} ончейн ${:>16.4} · реально ${:>12.6} · ×{:.0}", s, oc, real, r); }
    eprintln!("[tokprices] всього перекошених ≥3×: {} з {}", worst.len(), tokens.len());
    eprintln!("[tokprices] збережено token-prices.json");
}
