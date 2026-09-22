// Port of server/universe.js asset-universe loading. Verified ERC-20 base set
// (`BASE`), optionally overridden once at startup by the daily-refreshed
// `dataset/universe.json` cache (CoinGecko-enriched market-cap sizes) when it
// holds at least as many entries as BASE. `active_universe()` == getUniverse().

use serde_json::{json, Value};
use std::sync::{OnceLock, RwLock};

// universe.js BASE — verbatim (30 verified eth-mainnet contracts + rough sizes).


/// Базовий всесвіт — 30 перевірених контрактів. Раніше лежав ОДНИМ рядковим
/// літералом на 2.5 КБ прямо в коді: дані, що прикидались кодом. Тепер таблиці
/// `tokens` + `universe`, звідки їх можна і прочитати, і змінити без перезбирання.
fn base() -> Vec<Value> {
    let Ok(c) = wr_store::open() else {
        tracing::error!("[universe] сховище недоступне — базовий всесвіт порожній");
        return Vec::new();
    };
    let Ok(mut q) = c.prepare(
        "SELECT t.symbol, u.contract, t.sector, u.size
         FROM universe u JOIN tokens t ON t.chain=u.chain AND t.contract=u.contract
         ORDER BY u.position",
    ) else {
        return Vec::new();
    };
    let rows: Vec<Value> = q
        .query_map([], |r| {
            Ok(json!({
                "sym": r.get::<_, String>(0)?,
                "c": r.get::<_, String>(1)?,
                "sector": r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                "size": r.get::<_, f64>(3)?,
            }))
        })
        .map(|it| it.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();
    if rows.is_empty() {
        // Порожній всесвіт означав би «активів для аналізу немає» — і сервер
        // віддавав би порожні відповіді як нормальні.
        tracing::error!("[universe] у базі порожня таблиця universe — запусти `wr-import load`");
    }
    rows
}

fn file_path() -> std::path::PathBuf {
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&root).join("dataset").join("universe.json")
}

/// The universe is behind a lock because the daily scan REPLACES it — it used to be
/// a `OnceLock`, so a running server could never pick up refreshed market caps.
fn cell() -> &'static RwLock<Vec<Value>> {
    static U: OnceLock<RwLock<Vec<Value>>> = OnceLock::new();
    U.get_or_init(|| {
        let base = base();
        // `if (Array.isArray(j) && j.length >= BASE.length) UNI = j`
        let loaded = std::fs::read_to_string(file_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| match v {
                Value::Array(a) if a.len() >= base.len() => Some(a),
                _ => None,
            });
        RwLock::new(loaded.unwrap_or(base))
    })
}

/// getUniverse() — the active universe. Cloned: the scan can swap it underneath.
pub fn active_universe() -> Vec<Value> {
    cell().read().unwrap().clone()
}

/// universe.js CoinGecko ids, in BASE order.
const CG_IDS: &str = "bitcoin,ethereum,chainlink,uniswap,aave,arbitrum,ondo-finance,render-token,fetch-ai,injective-protocol,maker,the-graph,lido-dao,immutable-x,the-sandbox,ethereum-name-service,decentraland,curve-dao-token,axie-infinity,gala,apecoin,havven,compound-governance-token,1inch,pax-gold,shiba-inu,pepe";

/// universe.js `buildUniverse` — best-effort: re-size BASE by real market caps.
/// CoinGecko's free tier rate-limits, and the JS treats that as "keep the base set"
/// rather than an error, so a failure leaves the universe untouched.
pub async fn build_universe() {
    let url = format!(
        "https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&ids={CG_IDS}&order=market_cap_desc&per_page=80&page=1"
    );
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("wallet-radar/rust") // CoinGecko 403s a UA-less request
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let m: Value = match client.get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let Some(rows) = m.as_array().filter(|a| !a.is_empty()) else {
        tracing::info!("[universe] CoinGecko unavailable (rate limit) — keeping the base set");
        return;
    };

    // capBySym[SYM] = Math.max(1, Math.round(market_cap / 1e8))
    let mut cap: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for x in rows {
        let sym = x["symbol"].as_str().unwrap_or("").to_uppercase();
        if let Some(mc) = x["market_cap"].as_f64().filter(|v| *v != 0.0) {
            cap.insert(sym, crate::jsmath::js_round(mc / 1e8).max(1.0));
        }
    }
    let next: Vec<Value> = base()
        .into_iter()
        .map(|mut a| {
            let sym = a["sym"].as_str().unwrap_or("").to_string();
            // Stables keep size 0; everything else takes the real cap, or its base size.
            let size = if a["sector"] == json!("Stable") {
                0.0
            } else {
                cap.get(&sym).copied().unwrap_or_else(|| a["size"].as_f64().unwrap_or(0.0))
            };
            a["size"] = crate::jsmath::js_num(size);
            a
        })
        .collect();

    if let Ok(text) = serde_json::to_string(&next) {
        let _ = crate::atomicw::write(file_path(), text);
    }
    let n = cap.len();
    *cell().write().unwrap() = next;
    tracing::info!("[universe] sizes refreshed from real market caps ({n} assets)");
}
