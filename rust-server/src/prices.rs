//! Port of the DefiLlama pricing used by the wallet-score pipeline —
//! server/prices.js `priceTransfersHistorical`. Prices each transfer in USD at
//! its (monthly-bucketed) transaction time via coins.llama.fi batchHistorical,
//! mutating `t.usd` in place. Stables are treated as $1.
//!
//! Only the historical repricing is ported (that is what walletscore.js uses);
//! current-price / market-cap / chart helpers come later. Historical prices are
//! immutable, so a cold Rust process and a cold Node process issue the same
//! requests and produce the same `usd` values.

use chrono::{Datelike, TimeZone, Utc};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// TTL соц-кешу. 12 годин — не «щоб було», а прямий наслідок арифметики бюджету LunarCrush:
/// добова квота 100 викликів (`x-rate-limit-day: 100`), воркер фаз біжить раз на 6 год і обходить
/// 30 активів. При 12 год кожен актив оновлюється двічі на добу → 60 викликів, і ще ~40
/// лишається на запити користувача. Коротший TTL просто не влазить у бюджет.
const SOCIAL_TTL: Duration = Duration::from_secs(12 * 3600);

/// TTL для НЕВДАЛОЇ відповіді — і він навмисно КОРОТКИЙ.
///
/// Кешувати відмову так само довго, як успіх, було б пасткою: квота LunarCrush скидається щодоби,
/// і якби ми тримали закешований `null` 12 годин, то після скидання ще пів дня віддавали б
/// порожнечу — хоча джерело вже працює. Але й не кешувати відмову не можна: тоді кожен запит
/// знову довбав би джерело, яке щойно сказало «ні».
///
/// Година — компроміс: джерело не страждає, а після скидання квоти ми підхоплюємо дані швидко.
const SOCIAL_TTL_FAIL: Duration = Duration::from_secs(3600);

/// Де соц-кеш живе МІЖ перезапусками.
///
/// Кеш у памʼяті тут не працює, і це не дрібниця. Node тримає `cached()` із дампом на диск
/// (cache.js:42), а порт цього не переніс — тож кожен рестарт кора обнуляв кеш, і наступний цикл
/// воркера палив 30 викликів наново. При бюджеті 100 на добу кілька рестартів (звичайна річ під
/// час роботи) вичерпують квоту самі по собі — саме так вона сьогодні й вигоріла.
static SOCIAL_FILE: OnceLock<PathBuf> = OnceLock::new();

type SocialCache = HashMap<String, (i64, Value)>; // symbol → (unix-секунди, значення)

fn social_cache() -> &'static Mutex<SocialCache> {
    static C: OnceLock<Mutex<SocialCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(SocialCache::new()))
}

/// Підняти соц-кеш із диска. Викликається з main перед стартом воркерів.
pub fn init(dataset_dir: &std::path::Path) {
    let path = dataset_dir.join("social-cache.json");
    let loaded: SocialCache = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<HashMap<String, (i64, Value)>>(&t).ok())
        .unwrap_or_default();
    if !loaded.is_empty() {
        tracing::info!("[social] кеш піднято з диска: {} символів (квота LunarCrush — 100/добу, тож це не дрібниця)", loaded.len());
    }
    *social_cache().lock().unwrap() = loaded;
    let _ = SOCIAL_FILE.set(path);
}

fn persist_social(cache: &SocialCache) {
    let Some(path) = SOCIAL_FILE.get() else { return };
    if let Ok(t) = serde_json::to_string(cache) {
        // Помилка запису не має валити виклик: гірше, ніж не зберегти кеш, лише впасти через це.
        if let Err(e) = crate::atomicw::write(path, t) {
            tracing::warn!("[social] не зміг зберегти кеш у {}: {e}", path.display());
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// prices.js LLAMA — chain id → DefiLlama chain slug. Chains absent here are left
/// unpriced (their token transfers keep whatever usd Blockscout provided).
fn llama(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" => "ethereum",
        "polygon" => "polygon",
        "base" => "base",
        "optimism" => "optimism",
        "arbitrum" => "arbitrum",
        "gnosis" => "xdai",
        "zksync" => "era",
        "scroll" => "scroll",
        "celo" => "celo",
        "fuse" => "fuse",
        "linea" => "linea",
        "blast" => "blast",
        "mantle" => "mantle",
        "aurora" => "aurora",
        "metis" => "metis",
        "mode" => "mode",
        "zora" => "zora",
        "sonic" => "sonic",
        "lisk" => "lisk",
        "fraxtal" => "fraxtal",
        "rootstock" => "rsk",
        "solana" => "solana",
        _ => return None,
    })
}

const STABLE: [&str; 16] = [
    "USDT", "USDC", "DAI", "BUSD", "FDUSD", "TUSD", "USDE", "PYUSD", "USDD", "GUSD", "USDP", "LUSD",
    "USDS", "USD1", "CRVUSD", "FRAX",
];

fn is_stable(asset: &str) -> bool {
    STABLE.contains(&asset.to_uppercase().as_str())
}

/// prices.js `monthTs` — bucket a ms timestamp to the 15th of its UTC month, in
/// seconds. `Date.UTC(y, m, 15)/1000`, floored (already integral).
fn month_ts(ms: f64) -> i64 {
    let dt = Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap());
    Utc.with_ymd_and_hms(dt.year(), dt.month(), 15, 0, 0, 0).unwrap().timestamp()
}

/// Process-global historical price cache, `"{coin}@{monthSec}" → price`, mirroring
/// the JS module-level `histCache` (persists across requests).
fn hist_cache() -> &'static Mutex<HashMap<String, f64>> {
    static C: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    // The user agent is load-bearing: CoinGecko answers 403 to a UA-less request, so
    // without it `market_data` silently returned all-nulls — which is what dragged
    // marketCap to null and `liquidity.tier` to "unknown" everywhere downstream.
    // (cg.rs already learned this; prices.rs never did.)
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("wallet-radar/rust")
            .build()
            .expect("reqwest client builds from static config")
    })
}

/// wallet-asset-groups.js `ethPrice` — current ETH/USD via DefiLlama, cached
/// 10 minutes. Returns 0 on failure (callers treat 0 as "unpriced").
pub async fn eth_price() -> f64 {
    static CACHE: OnceLock<Mutex<Option<(std::time::Instant, f64)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, v)) = *cache.lock().unwrap() {
        if at.elapsed() < Duration::from_secs(600) {
            return v;
        }
    }
    let v = fetch_eth_price().await;
    *cache.lock().unwrap() = Some((std::time::Instant::now(), v));
    v
}

async fn fetch_eth_price() -> f64 {
    let url = "https://coins.llama.fi/prices/current/coingecko:ethereum";
    let d: Value = match http().get(url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => return 0.0,
    };
    d["coins"]["coingecko:ethereum"]["price"].as_f64().unwrap_or(0.0)
}

/// prices.js `NATIVE_ID` — chain → DefiLlama coingecko coin id for the native coin.
fn native_id(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" | "base" | "arbitrum" | "optimism" | "zksync" | "scroll" | "linea" | "blast" => "coingecko:ethereum",
        "polygon" => "coingecko:matic-network",
        "gnosis" => "coingecko:xdai",
        "celo" => "coingecko:celo",
        "mantle" => "coingecko:mantle",
        "sonic" => "coingecko:sonic-3",
        "bnb" => "coingecko:binancecoin",
        "avalanche" => "coingecko:avalanche-2",
        "bitcoin" => "coingecko:bitcoin",
        "solana" => "coingecko:solana",
        "tron" => "coingecko:tron",
        _ => return None,
    })
}

/// prices.js `nativePriceUsd(chain)` — current native-coin price via DefiLlama,
/// cached 10 min per coin id. Returns 0 for an unknown chain or on failure.
pub async fn native_price_usd(chain: &str) -> f64 {
    let Some(id) = native_id(chain) else { return 0.0 };
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::Instant, f64)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().unwrap().get(id) {
        if at.elapsed() < Duration::from_secs(600) {
            return *v;
        }
    }
    let url = format!("https://coins.llama.fi/prices/current/{id}");
    let v = match http().get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => {
            let d: Value = r.json().await.unwrap_or(Value::Null);
            d["coins"][id]["price"].as_f64().unwrap_or(0.0)
        }
        _ => 0.0,
    };
    // JS caches only on a successful fetch (the catch leaves the old entry); a
    // failure here returns 0 without poisoning the cache.
    if v != 0.0 {
        cache.lock().unwrap().insert(id.to_string(), (std::time::Instant::now(), v));
    }
    v
}

/// prices.js `tokenPricesCurrent(items)` — current USD price per `(chain, contract)`
/// via DefiLlama `/prices/current`, batched by 100. Returns a map keyed
/// `"{chain}:{contract}"` holding only strictly-positive prices. Chains without a
/// DefiLlama slug, and empty contracts, are skipped.
pub async fn token_prices_current(items: &[(String, String)]) -> HashMap<String, f64> {
    // id_to_key: "{llama}:{contract}" → "{chain}:{contract}", dedup on the llama id.
    let mut id_to_key: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
    for (chain, contract) in items {
        let Some(lc) = llama(chain) else { continue };
        if contract.is_empty() {
            continue;
        }
        let c = contract.to_lowercase();
        id_to_key.insert(format!("{lc}:{c}"), format!("{chain}:{c}"));
    }
    let ids: Vec<String> = id_to_key.keys().cloned().collect();
    let mut out: HashMap<String, f64> = HashMap::new();
    for chunk in ids.chunks(100) {
        let url = format!("https://coins.llama.fi/prices/current/{}", chunk.join(","));
        let d: Value = match http().get(&url).header("accept", "application/json").send().await {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
            _ => continue,
        };
        for id in chunk {
            // Number(d.coins?.[id]?.price) || 0 — only keep p > 0.
            let p = d["coins"][id.as_str()]["price"].as_f64().unwrap_or(0.0);
            if p > 0.0 {
                out.insert(id_to_key[id].clone(), p);
            }
        }
    }
    out
}

/// Process-global current token-price cache, `"{llamaChain}:{contract}" → USD`,
/// mirroring prices.js module-level `cache`.
fn cur_cache() -> &'static Mutex<HashMap<String, f64>> {
    static C: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// prices.js `priceTransfers` — current-price USD valuation of transfers via
/// coins.llama.fi `/prices/current`, mutating each transfer's `usd` in place.
/// Already-valued (`usd > 0`) and stable transfers are left/set without a fetch;
/// tokens on chains DefiLlama covers are priced at their current contract price;
/// chains with no DefiLlama slug keep whatever `usd` they arrived with. Returns
/// the JS coverage summary (unused by the behaviour route, kept for parity).
pub async fn price_transfers(by_chain: &mut Map<String, Value>) -> Value {
    // 1) Collect needed `{lc}:{token}` ids (dedup, first-occurrence order),
    //    skipping already-cached / already-priced / stable transfers.
    let mut need: indexmap::IndexSet<String> = indexmap::IndexSet::new();
    {
        let cache = cur_cache().lock().unwrap();
        for (chain, trs) in by_chain.iter() {
            let Some(lc) = llama(chain) else { continue };
            let Some(arr) = trs.as_array() else { continue };
            for t in arr {
                let usd = t["usd"].as_f64().unwrap_or(f64::NAN);
                let asset = t["asset"].as_str().unwrap_or("");
                if usd > 0.0 || is_stable(asset) {
                    continue;
                }
                if let Some(token) = t["token"].as_str().filter(|s| !s.is_empty()) {
                    let id = format!("{lc}:{token}");
                    if !cache.contains_key(&id) {
                        need.insert(id);
                    }
                }
            }
        }
    }

    // 2) Fetch current prices in batches of 100; a failed/absent price caches 0.
    let ids: Vec<String> = need.into_iter().collect();
    for chunk in ids.chunks(100) {
        let url = format!("https://coins.llama.fi/prices/current/{}", chunk.join(","));
        let ok: Option<Value> = match http().get(&url).header("accept", "application/json").send().await {
            Ok(r) if r.status().is_success() => Some(r.json().await.unwrap_or(Value::Null)),
            _ => None,
        };
        let mut cache = cur_cache().lock().unwrap();
        match ok {
            Some(d) => {
                for id in chunk {
                    // Number(d.coins?.[id]?.price) || 0
                    cache.insert(id.clone(), d["coins"][id.as_str()]["price"].as_f64().unwrap_or(0.0));
                }
            }
            None => {
                for id in chunk {
                    cache.insert(id.clone(), 0.0);
                }
            }
        }
    }

    // 3) Reprice every transfer in place and accumulate the coverage summary.
    let cache = cur_cache().lock().unwrap();
    let (mut priced, mut total, mut valued) = (0i64, 0i64, 0.0f64);
    for (chain, trs) in by_chain.iter_mut() {
        let lc = llama(chain);
        let Some(arr) = trs.as_array_mut() else { continue };
        for t in arr.iter_mut() {
            total += 1;
            let usd = t["usd"].as_f64().unwrap_or(f64::NAN);
            if usd > 0.0 {
                priced += 1;
                valued += usd;
                continue;
            }
            let asset = t["asset"].as_str().unwrap_or("").to_string();
            let amount = t["amount"].as_f64().unwrap_or(0.0);
            if is_stable(&asset) {
                set_usd(t, amount);
                priced += 1;
                valued += amount;
                continue;
            }
            let token = t["token"].as_str().unwrap_or("").to_string();
            let p = match (lc, token.is_empty()) {
                (Some(lc), false) => *cache.get(&format!("{lc}:{token}")).unwrap_or(&0.0),
                _ => 0.0,
            };
            if p != 0.0 {
                set_usd(t, amount * p);
                priced += 1;
                valued += amount * p;
            }
        }
    }
    let coverage = if total != 0 { priced as f64 / total as f64 } else { 0.0 };
    serde_json::json!({"coverage": coverage, "total": total, "priced": priced, "valuedUsd": valued})
}

/// prices.js `marketData(contract)` — CoinGecko market + community metrics for an
/// Ethereum token. Best-effort: any failure yields the all-null default object.
///
/// prices.js:83 `mcCache`, 1 hour. Restoring this is not an optimisation: CoinGecko
/// is keyless here, and without the cache a single /api/coordinated-networks request
/// fires ~60 of these at once, earns a 429, and every marketCap comes back null —
/// which silently degrades `liquidity.tier` to "unknown" downstream. Note the JS
/// caches the FAILED result too (the `mcCache.set` is outside the try), so a bad
/// hour stays bad; we keep that.
pub async fn market_data(contract: &str) -> Value {
    let c = contract.to_lowercase();
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::Instant, Value)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().unwrap().get(&c) {
        if at.elapsed() < Duration::from_secs(3600) {
            return v.clone();
        }
    }
    let data = fetch_market_data(&c).await;
    cache.lock().unwrap().insert(c, (std::time::Instant::now(), data.clone()));
    data
}

async fn fetch_market_data(c: &str) -> Value {
    let null_data = serde_json::json!({
        "marketCap": Value::Null, "fdv": Value::Null, "price": Value::Null, "name": Value::Null, "symbol": Value::Null,
        "volume24h": Value::Null, "sentimentUp": Value::Null, "watchlistUsers": Value::Null, "redditActive48h": Value::Null,
        "redditSubscribers": Value::Null, "telegramUsers": Value::Null,
    });
    let url = format!("https://api.coingecko.com/api/v3/coins/ethereum/contract/{c}");
    let d: Value = match http().get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => return null_data,
    };
    if !d.is_object() {
        return null_data;
    }
    let md = &d["market_data"];
    let cd = &d["community_data"];
    // (d.symbol||"").toUpperCase() || null
    let symbol = d["symbol"].as_str().unwrap_or("").to_uppercase();
    serde_json::json!({
        "marketCap": md["market_cap"]["usd"].clone(),
        "fdv": md["fully_diluted_valuation"]["usd"].clone(),
        "price": md["current_price"]["usd"].clone(),
        "name": if d["name"].is_string() { d["name"].clone() } else { Value::Null },
        "symbol": if symbol.is_empty() { Value::Null } else { Value::String(symbol) },
        "volume24h": md["total_volume"]["usd"].clone(),
        "sentimentUp": nn(&d["sentiment_votes_up_percentage"]),
        "watchlistUsers": nn(&d["watchlist_portfolio_users"]),
        "redditActive48h": nn(&cd["reddit_accounts_active_48h"]),
        "redditSubscribers": nn(&cd["reddit_subscribers"]),
        "telegramUsers": nn(&cd["telegram_channel_user_count"]),
    })
}

/// `?? null` — absent (or JSON null) → Null, else the value.
fn nn(v: &Value) -> Value {
    if v.is_null() {
        Value::Null
    } else {
        v.clone()
    }
}

/// social.js `getSocial(symbol)` — LunarCrush social metrics, or `Null` with no
/// key / empty symbol / any error (incl. an inactive-subscription error). Callers
/// fall back to the keyless CoinGecko path.
///
/// social.js:35 обгортає виклик у `cached("social:SYM", 30 хв)` — і порт цей кеш ЗАГУБИВ.
/// Ціна виявилася високою: ліміт LunarCrush не посекундний, а ДОБОВИЙ, тож кожен зайвий виклик
/// їв денний бюджет назавжди. Форензика обходить 72 активи за цикл — квота згоряла, LunarCrush
/// починав віддавати 429, а кор МОВЧКИ падав на слабший CoinGecko (`get_social_free`). Тобто
/// соц-складова сигналу пампу тихо вимикалася, і жоден лог про це не казав.
pub async fn get_social(symbol: &str) -> Value {
    if symbol.is_empty() {
        return Value::Null;
    }

    // Ключ кешу — символ у верхньому регістрі, як у social.js:35.
    let ck = symbol.to_uppercase();
    let now = now_secs();
    if let Some((at, v)) = social_cache().lock().unwrap().get(&ck) {
        // Успіх живе 12 год, відмова — годину. Різні TTL саме тому, що це різні події: перше —
        // «ось дані», друге — «джерело зараз не віддає», і друге мусить швидко перепитуватись.
        let ttl = if v.is_null() { SOCIAL_TTL_FAIL } else { SOCIAL_TTL };
        if now - at < ttl.as_secs() as i64 {
            return v.clone();
        }
    }
    let v = fetch_social(symbol).await;
    // Кешуємо й НЕВДАЛИЙ результат (як cached() у Node): інакше вичерпана добова квота означала б
    // новий виклик на КОЖЕН запит — ми довбали б джерело, яке вже сказало «ні».
    {
        let mut c = social_cache().lock().unwrap();
        c.insert(ck, (now, v.clone()));
        persist_social(&c);
    }
    v
}

async fn fetch_social(symbol: &str) -> Value {
    let enc: String = url::form_urlencoded::byte_serialize(symbol.as_bytes()).collect();

    // Суворий режим: ключ LunarCrush живе на сателіті.
    let j: Value = if crate::satellite::strict() {
        match crate::satellite::fetch_json("lunarcrush", &format!("/api4/public/coins/{enc}/v1"), &[]).await {
            Ok(v) => v,
            Err(e) => {
                // Мовчазний Null тут — не «соц-даних немає», а «джерело нас відрізало», і
                // викликач через це ТИХО перемкнеться на слабший CoinGecko. Раз ми деградуємо —
                // ми про це кажемо. Саме така мовчанка приховала вичерпану добову квоту.
                tracing::warn!("[social] LunarCrush недоступний для {symbol}: {e} → падаю на CoinGecko (сигнал пампу без соц-складової)");
                return Value::Null;
            }
        }
    } else {
        let key = std::env::var("LUNARCRUSH_API_KEY").unwrap_or_default();
        if key.is_empty() {
            return Value::Null;
        }
        let url = format!("https://lunarcrush.com/api4/public/coins/{enc}/v1");
        match http().get(&url).header("accept", "application/json").bearer_auth(&key).send().await {
            Ok(r) => r.json().await.unwrap_or(serde_json::json!({})),
            Err(_) => return Value::Null,
        }
    };
    // if (j?.error) return null — a truthy error string (subscription etc.).
    if j["error"].as_str().is_some_and(|s| !s.is_empty()) || j["error"] == serde_json::json!(true) {
        return Value::Null;
    }
    let d = &j["data"];
    serde_json::json!({
        "available": true,
        "galaxyScore": nn(&d["galaxy_score"]), "altRank": nn(&d["alt_rank"]), "sentiment": nn(&d["sentiment"]),
        "socialVolume24h": nn(&d["social_volume_24h"]), "socialDominance": nn(&d["social_dominance"]), "interactions24h": nn(&d["interactions_24h"]),
    })
}

/// index.js `nativePrices()` — the {eth, matic, xdai, celo} native price map for
/// the coordination valuation, cached 10 min. `xdai` falls back to 1. On failure
/// returns the last good map, or `{}` (as the JS `_natPx.v` starts empty).
pub async fn native_prices_coord() -> Value {
    static CACHE: OnceLock<Mutex<Option<(std::time::Instant, Value)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, v)) = &*cache.lock().unwrap() {
        if at.elapsed() < Duration::from_secs(600) {
            return v.clone();
        }
    }
    let url = "https://coins.llama.fi/prices/current/coingecko:ethereum,coingecko:matic-network,coingecko:xdai,coingecko:celo";
    let fetched = match http().get(url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => {
            let d: Value = r.json().await.unwrap_or(Value::Null);
            let g = |id: &str| d["coins"][id]["price"].as_f64().unwrap_or(0.0);
            let xdai = g("coingecko:xdai");
            Some(serde_json::json!({
                "eth": crate::jsmath::js_num(g("coingecko:ethereum")),
                "matic": crate::jsmath::js_num(g("coingecko:matic-network")),
                "xdai": crate::jsmath::js_num(if xdai != 0.0 { xdai } else { 1.0 }),
                "celo": crate::jsmath::js_num(g("coingecko:celo")),
            }))
        }
        _ => None,
    };
    let mut c = cache.lock().unwrap();
    match fetched {
        Some(v) => {
            *c = Some((std::time::Instant::now(), v.clone()));
            v
        }
        None => c.as_ref().map(|(_, v)| v.clone()).unwrap_or_else(|| serde_json::json!({})),
    }
}

/// prices.js `priceChart(contract, chain)` — the full DefiLlama price series
/// `[{ts(ms), price}]` for a token, dropping non-positive prices. `[]` on failure.
pub async fn price_chart(contract: &str, chain: &str) -> Vec<Value> {
    let lc = llama(chain).unwrap_or("ethereum");
    let id = format!("{lc}:{}", contract.to_lowercase());
    // prices.js:109 `chartCache`, 1 hour. Unlike marketData, the JS only caches a
    // SUCCESSFUL fetch — a failure returns [] and is retried next call.
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::Instant, Vec<Value>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().unwrap().get(&id) {
        if at.elapsed() < Duration::from_secs(3600) {
            return v.clone();
        }
    }
    let url = format!("https://coins.llama.fi/chart/{id}?period=1w&span=160&searchWidth=600");
    let d: Value = match http().get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => return vec![],
    };
    let empty = vec![];
    let series: Vec<Value> = d["coins"][id.as_str()]["prices"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|p| {
            let price = p["price"].as_f64().unwrap_or(0.0);
            (price > 0.0).then(|| serde_json::json!({"ts": crate::jsmath::js_num(p["timestamp"].as_f64().unwrap_or(0.0) * 1000.0), "price": p["price"].clone()}))
        })
        .collect();
    cache.lock().unwrap().insert(id, (std::time::Instant::now(), series.clone()));
    series
}

/// prices.js `priceTransfersHistorical`. `by_chain` maps chain → transfer array;
/// each transfer's `usd` is set to `amount × historical price` (stables → amount).
/// `eth_native_chains` marks chains whose native asset is ETH — unused here in
/// practice since walletscore feeds only ERC-20 transfers, but ported faithfully.
pub async fn price_transfers_historical(by_chain: &mut Map<String, Value>, eth_native_chains: &HashSet<&str>) -> Value {
    // 1) Collect wanted (coin → months) skipping anything already cached.
    let mut wanted: indexmap::IndexMap<String, indexmap::IndexSet<i64>> = indexmap::IndexMap::new();
    {
        let cache = hist_cache().lock().unwrap();
        let mut want = |coin: &str, ms: f64| {
            if coin.is_empty() || ms == 0.0 || ms.is_nan() {
                return;
            }
            let s = month_ts(ms);
            if cache.contains_key(&format!("{coin}@{s}")) {
                return;
            }
            wanted.entry(coin.to_string()).or_default().insert(s);
        };
        for (chain, trs) in by_chain.iter() {
            let lc = llama(chain);
            let Some(arr) = trs.as_array() else { continue };
            for t in arr {
                let asset = t["asset"].as_str().unwrap_or("");
                let ts = t["ts"].as_f64().unwrap_or(0.0);
                if asset == "native" {
                    if eth_native_chains.contains(chain.as_str()) {
                        want("coingecko:ethereum", ts);
                    }
                } else if !is_stable(asset) {
                    if let (Some(lc), Some(token)) = (lc, t["token"].as_str().filter(|s| !s.is_empty())) {
                        want(&format!("{lc}:{token}"), ts);
                    }
                }
            }
        }
    }

    // 2) Fetch in batches of 15 coins; fill the cache from the responses.
    let entries: Vec<(String, Vec<i64>)> =
        wanted.into_iter().map(|(coin, set)| (coin, set.into_iter().collect())).collect();
    for chunk in entries.chunks(15) {
        let mut coins_obj = Map::new();
        for (coin, months) in chunk {
            coins_obj.insert(coin.clone(), Value::Array(months.iter().map(|m| json_i64(*m)).collect()));
        }
        let json = serde_json::to_string(&Value::Object(coins_obj)).unwrap();
        let encoded: String = url::form_urlencoded::byte_serialize(json.as_bytes()).collect();
        let url = format!("https://coins.llama.fi/batchHistorical?coins={encoded}&searchWidth=12h");
        let d: Value = match http().get(&url).header("accept", "application/json").send().await {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
            _ => continue,
        };
        if let Some(coins) = d["coins"].as_object() {
            let mut cache = hist_cache().lock().unwrap();
            for (coin, info) in coins {
                for p in info["prices"].as_array().unwrap_or(&vec![]) {
                    let (Some(ts), Some(price)) = (p["timestamp"].as_f64(), p["price"].as_f64()) else { continue };
                    cache.insert(format!("{coin}@{}", month_ts(ts * 1000.0)), price);
                }
            }
        }
    }

    // 3) Reprice every transfer in place, accumulating the coverage summary
    //    (prices.js `priceTransfersHistorical` return: coverage/total/priced/valuedUsd).
    let cache = hist_cache().lock().unwrap();
    let (mut total, mut priced, mut valued) = (0i64, 0i64, 0.0f64);
    for (chain, trs) in by_chain.iter_mut() {
        let lc = llama(chain);
        let is_eth_native = eth_native_chains.contains(chain.as_str());
        let is_gnosis = chain == "gnosis";
        let Some(arr) = trs.as_array_mut() else { continue };
        for t in arr.iter_mut() {
            total += 1;
            let asset = t["asset"].as_str().unwrap_or("").to_string();
            let amount = t["amount"].as_f64().unwrap_or(0.0);
            let ts = t["ts"].as_f64().unwrap_or(0.0);
            let px = if asset == "native" {
                if is_eth_native {
                    *cache.get(&format!("coingecko:ethereum@{}", month_ts(ts))).unwrap_or(&0.0)
                } else if is_gnosis {
                    1.0
                } else {
                    0.0
                }
            } else if is_stable(&asset) {
                set_usd(t, amount);
                priced += 1;
                valued += amount;
                continue;
            } else if let (Some(lc), Some(token)) = (lc, t["token"].as_str().filter(|s| !s.is_empty())) {
                *cache.get(&format!("{lc}:{token}@{}", month_ts(ts))).unwrap_or(&0.0)
            } else {
                0.0
            };
            if px > 0.0 {
                set_usd(t, amount * px);
                priced += 1;
                valued += amount * px;
            }
        }
    }
    serde_json::json!({"coverage": if total != 0 { priced as f64 / total as f64 } else { 0.0 }, "total": total, "priced": priced, "valuedUsd": crate::jsmath::js_num(valued)})
}

fn set_usd(t: &mut Value, v: f64) {
    // js_num, not from_f64: JSON.stringify writes an integral number without a
    // decimal point (`500` not `500.0`), so a stable's usd matches Node exactly.
    if let Some(o) = t.as_object_mut() {
        o.insert("usd".into(), crate::jsmath::js_num(v));
    }
}

fn json_i64(n: i64) -> Value {
    Value::Number(serde_json::Number::from(n))
}

/// prices.js `llamaChart(id, period, span)` — the DAILY series the backtest needs.
/// `price_chart` above is the weekly one used by the UI; the backtest cannot use it,
/// because with weekly candles an entry lands up to 3.5 days from its true price.
/// Cached 1h, successes only (a failure returns [] and is retried).
async fn llama_chart(id: &str, period: &str, span: u32) -> Vec<Value> {
    let key = format!("{id}|{period}|{span}");
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::Instant, Vec<Value>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, v)) = cache.lock().unwrap().get(&key) {
        if at.elapsed() < Duration::from_secs(3600) {
            return v.clone();
        }
    }
    let url = format!("https://coins.llama.fi/chart/{id}?period={period}&span={span}&searchWidth=600");
    let d: Value = match http().get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
        _ => return vec![],
    };
    let empty = vec![];
    let series: Vec<Value> = d["coins"][id]["prices"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|p| {
            let price = p["price"].as_f64().unwrap_or(0.0);
            (price > 0.0).then(|| {
                serde_json::json!({"ts": crate::jsmath::js_num(p["timestamp"].as_f64().unwrap_or(0.0) * 1000.0), "price": p["price"].clone()})
            })
        })
        .collect();
    cache.lock().unwrap().insert(key, (std::time::Instant::now(), series.clone()));
    series
}

/// prices.js:128 — a token's daily chart, 365 days.
pub async fn price_chart_daily(contract: &str, chain: &str) -> Vec<Value> {
    let lc = llama(chain).unwrap_or("ethereum");
    llama_chart(&format!("{lc}:{}", contract.to_lowercase()), "1d", 365).await
}

/// prices.js:130 — the benchmark. A −37% return means nothing if the market fell 40%
/// over the same window, so every backtest return is measured against ETH.
pub async fn benchmark_chart_daily() -> Vec<Value> {
    llama_chart("coingecko:ethereum", "1d", 365).await
}

/// prices.js `priceAtBounded` — the price at `ts`, but only within a tolerance.
///
/// The unbounded version silently handed a three-year-old purchase the price from the
/// start of the chart. Out of tolerance now returns None and the event is dropped: an
/// honest gap beats an invented entry price.
pub fn price_at_bounded(series: &[Value], ts: f64, tolerance_ms: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    let mut bd = f64::INFINITY;
    for p in series {
        let d = (p["ts"].as_f64().unwrap_or(0.0) - ts).abs();
        if d < bd {
            bd = d;
            best = p["price"].as_f64();
        }
    }
    (bd <= tolerance_ms).then_some(best).flatten()
}

/// prices.js:135 default — 36h, i.e. a day and a half either side of a daily candle.
pub const PRICE_TOLERANCE_MS: f64 = 36.0 * 3600.0 * 1000.0;

#[cfg(test)]
mod social_cache_tests {
    use super::*;

    /// Соц-кеш мусить влазити в добовий бюджет LunarCrush (100 викликів, `x-rate-limit-day`).
    /// Воркер фаз: 30 активів, цикл раз на 6 год. Якщо хтось поверне TTL до 30 хв «щоб свіжіше»,
    /// квота вигорить мовчки — саме так вона вже вигоряла.
    #[test]
    fn social_ttl_fits_the_daily_budget() {
        const BUDGET_PER_DAY: u64 = 100;
        const ASSETS: u64 = 30;
        const WORKER_CYCLES_PER_DAY: u64 = 4; // раз на 6 годин

        let refreshes_per_day = (24 * 3600 / SOCIAL_TTL.as_secs()).min(WORKER_CYCLES_PER_DAY);
        let worker_calls = ASSETS * refreshes_per_day;
        assert!(
            worker_calls < BUDGET_PER_DAY,
            "воркер спалив би {worker_calls} викликів на добу при бюджеті {BUDGET_PER_DAY} — TTL {} год замалий",
            SOCIAL_TTL.as_secs() / 3600
        );
        // І має лишитись запас на запити користувача, а не «рівно впритул».
        assert!(BUDGET_PER_DAY - worker_calls >= 30, "не лишилось запасу на запити користувача");
    }

    /// Відмову НЕ можна кешувати так само довго, як успіх: квота скидається щодоби, і 12-годинний
    /// закешований null означав би пів дня порожнечі вже ПІСЛЯ того, як джерело ожило.
    #[test]
    fn failures_expire_much_sooner_than_successes() {
        assert!(
            SOCIAL_TTL_FAIL < SOCIAL_TTL / 4,
            "відмова мусить перепитуватись значно раніше за оновлення успішних даних"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live parity for the DefiLlama historical repricing against Node's
    /// `priceTransfersHistorical`. Ignored by default (hits coins.llama.fi);
    /// run with `cargo test -- --ignored price_transfers`. Historical prices are
    /// immutable, so a cold process reproduces the golden exactly.
    #[tokio::test]
    #[ignore]
    async fn price_transfers_historical_matches_node() {
        let input: Value = serde_json::from_str(&fixture!("pricing-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("pricing-golden.json")).unwrap();
        let mut by_chain: Map<String, Value> = input["byChain"].as_object().unwrap().clone();
        let eth_native: HashSet<&str> = ["eth", "base", "arbitrum", "optimism"].into_iter().collect();
        price_transfers_historical(&mut by_chain, &eth_native).await;
        assert_eq!(Value::Object(by_chain), golden);
    }
}
