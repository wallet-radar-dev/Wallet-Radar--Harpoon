// Thin CoinGecko proxies (server/index.js:259-280) — avoid browser CORS and add
// a cache against rate limits. Pure passthroughs: the response is whatever
// CoinGecko returns, so it matches Node byte-for-byte for the same upstream
// answer. On any error they degrade to the JS empty shapes.

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(9)).user_agent("wallet-radar/1.0").build().unwrap())
}

/// index.js:259 `/api/cg-price` — CoinGecko simple/price. Empty ids → `{}`.
pub async fn cg_price(ids_raw: &str) -> Value {
    let ids: String = ids_raw.chars().take(1000).collect();
    if ids.is_empty() {
        return json!({});
    }
    // index.js:263 `cached("cgprice:…", 60000)`. CoinGecko безключовий і 429-иться швидко, а
    // помилка тут повертає порожній {} — тобто «ціни немає», що не відрізнити від «ціна нуль».
    let ck = format!("cgprice:{ids}");
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(60)) {
        return v;
    }
    let url = format!("https://api.coingecko.com/api/v3/simple/price?ids={ids}&vs_currencies=usd");
    let v = match http().get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_else(|_| json!({})),
        _ => json!({}),
    };
    // Порожню відповідь не закріплюємо: інакше одна 429 замовкла б ціну на цілу хвилину.
    if v.as_object().is_some_and(|o| !o.is_empty()) {
        crate::bs::cache_value(&ck, &v);
    }
    v
}

/// index.js:270 `/api/cg-search` — CoinGecko search. Empty query → `{coins:[]}`.
pub async fn cg_search(query_raw: &str) -> Value {
    let q: String = query_raw.chars().take(100).collect();
    if q.is_empty() {
        return json!({ "coins": [] });
    }
    // index.js:274 `cached("cgsearch:…", 300000)`. Пошук користувач тисне повторно (той самий
    // запит під час набору), а квота CoinGecko безключова — без кешу це найшвидший спосіб дістати 429.
    let ck = format!("cgsearch:{}", q.to_lowercase());
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(300)) {
        return v;
    }
    let v = match http()
        .get("https://api.coingecko.com/api/v3/search")
        .query(&[("query", q.as_str())])
        .header("accept", "application/json")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_else(|_| json!({ "coins": [] })),
        _ => json!({ "coins": [] }),
    };
    // Порожній результат — це або «не знайдено», або 429. Не закріплюємо: друге мусить бути
    // перепитане, а відрізнити їх тут ми не можемо.
    if v["coins"].as_array().is_some_and(|a| !a.is_empty()) {
        crate::bs::cache_value(&ck, &v);
    }
    v
}


/// Compact per-asset market snapshot from CoinGecko — by ERC-20 `contract` (our
/// assets) or by CoinGecko `id` (search hits). Returns
/// `{contract, priceUsd, marketCap, rank, symbol, name}`; `{}` on 429 / miss.
/// Cached 5 min (shares the keyless rate-limit budget).
pub async fn cg_asset(contract: &str, id: &str) -> Value {
    let ck = if !contract.is_empty() {
        format!("cgasset:c:{}", contract.to_lowercase())
    } else if !id.is_empty() {
        format!("cgasset:i:{}", id.to_lowercase())
    } else {
        return json!({});
    };
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(300)) {
        return v;
    }
    let base = if !contract.is_empty() {
        format!("https://api.coingecko.com/api/v3/coins/ethereum/contract/{}", contract.to_lowercase())
    } else {
        format!("https://api.coingecko.com/api/v3/coins/{}", id.to_lowercase())
    };
    let url = format!("{base}?localization=false&tickers=false&community_data=false&developer_data=false&sparkline=false");
    let v: Value = match http().get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => Value::Null,
    };
    if v.is_null() {
        return json!({});
    }
    let eth = v["platforms"]["ethereum"].as_str().unwrap_or("");
    let contract_out = if !contract.is_empty() { contract.to_lowercase() } else { eth.to_lowercase() };
    let out = json!({
        "contract": contract_out,
        "priceUsd": v["market_data"]["current_price"]["usd"],
        "marketCap": v["market_data"]["market_cap"]["usd"],
        "rank": v["market_cap_rank"],
        "symbol": v["symbol"].as_str().map(|s| s.to_uppercase()),
        "name": v["name"],
    });
    // Cache only a real answer (has a price or a resolved contract).
    if out["priceUsd"].is_number() || out["contract"].as_str().is_some_and(|c| !c.is_empty()) {
        crate::bs::cache_value(&ck, &out);
    }
    out
}
