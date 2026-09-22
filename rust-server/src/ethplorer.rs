//! Port of the /api/holder-assets reduction over Ethplorer `getAddressInfo`, plus
//! a minimal Ethplorer client (freekey, backoff on 429/503).
//!
//! `holder_assets_build` is pure and golden-tested; the fetch is best-effort and
//! current-state. JS quirks kept: `typeof d.ETH.balance === "number"` guard,
//! `ti.price && ti.price.rate` (price can be `false`), `decimals || 18`,
//! `symbol || "?"`, totals summed before the top-30 slice.

use crate::jsmath::js_num;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `Number(v)` — number as-is, decimal string parsed, else 0.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

/// index.js holder-assets reduction over an Ethplorer `getAddressInfo` response.
pub fn holder_assets_build(address: &str, d: &Value) -> Value {
    let mut assets: Vec<Value> = Vec::new();

    let eth = &d["ETH"];
    if eth.is_object() && eth["balance"].is_number() {
        let bal = eth["balance"].as_f64().unwrap_or(0.0);
        let rate = if eth["price"].is_object() && truthy(&eth["price"]["rate"]) { num(&eth["price"]["rate"]) } else { 0.0 };
        assets.push(json!({"symbol": "ETH", "name": "Ethereum", "amount": js_num(bal), "valueUsd": js_num(bal * rate)}));
    }

    let empty = vec![];
    for tk in d["tokens"].as_array().unwrap_or(&empty) {
        let ti = &tk["tokenInfo"];
        let dec = if truthy(&ti["decimals"]) { num(&ti["decimals"]) } else { 18.0 };
        let amt = num(&tk["balance"]) / 10f64.powf(dec);
        let rate = if ti["price"].is_object() && truthy(&ti["price"]["rate"]) { num(&ti["price"]["rate"]) } else { 0.0 };
        let symbol = ti["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?");
        let name = ti["name"].as_str().unwrap_or("");
        assets.push(json!({"symbol": symbol, "name": name, "amount": js_num(amt), "valueUsd": js_num(amt * rate)}));
    }

    assets.sort_by(|x, y| {
        y["valueUsd"].as_f64().unwrap_or(0.0).partial_cmp(&x["valueUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let total: f64 = assets.iter().map(|a| a["valueUsd"].as_f64().unwrap_or(0.0)).sum();
    let top: Vec<Value> = assets.iter().take(30).cloned().collect();
    json!({"address": address, "totalUsd": js_num(total), "assets": top})
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap())
}

/// ethplorer.js `addressInfo` — `getAddressInfo/{addr}`, retrying 429/503 with
/// backoff (up to 4 attempts). `Err` on a fatal HTTP/API error.
pub async fn address_info(addr: &str) -> anyhow::Result<Value> {
    // ethplorer.js:46 `cached("ai:…", 600000)` — обгортка, яку порт загубив. Тут вона особливо
    // важлива: ключ за замовчуванням "freekey", і Ethplorer лімітує його жорстко (нижче вже є
    // ретраї на 429/503). Без кешу ми самі ж і викликаємо ті 429, які потім героїчно повторюємо.
    let ck = format!("ai:{}", addr.to_lowercase());
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(600)) {
        return Ok(v);
    }
    let key = std::env::var("ETHPLORER_API_KEY").unwrap_or_else(|_| "freekey".to_string());
    let url = format!("https://api.ethplorer.io/getAddressInfo/{addr}?apiKey={key}");
    for attempt in 0..4u32 {
        match http().get(&url).send().await {
            Ok(r) => {
                let status = r.status().as_u16();
                if status == 429 || status == 503 {
                    tokio::time::sleep(Duration::from_millis(2000 * (attempt as u64 + 1))).await;
                    continue;
                }
                if !r.status().is_success() {
                    anyhow::bail!("ethplorer HTTP {status}");
                }
                let d: Value = r.json().await?;
                if truthy(&d["error"]) {
                    anyhow::bail!("{}", d["error"]["message"].as_str().unwrap_or("ethplorer error"));
                }
                // Кешуємо лише успіх: помилки виходять через bail! і до кешу не доходять.
                crate::bs::cache_value(&ck, &d);
                return Ok(d);
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(2000 * (attempt as u64 + 1))).await,
        }
    }
    anyhow::bail!("ethplorer HTTP 429 (rate-limited after retries)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holder_assets_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/holderassets-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/holderassets-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            assert_eq!(holder_assets_build(c["address"].as_str().unwrap(), &c["d"]), golden[i], "case {i}");
        }
    }
}
