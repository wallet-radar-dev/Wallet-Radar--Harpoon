//! Live Blockscout HTTP client — the network layer behind the compute kernels.
//! Ports the fetching half of server/blockscout.js: the multi-chain host table,
//! the global polite scheduler (bounded concurrency + minimum gap between
//! request starts), and `bget`'s retry/backoff on rate limits.
//!
//! Response parsing reuses the golden-tested parsers in `blockscout.rs`, so the
//! shaped output matches Node. Native-transfer fetching mirrors Node's three
//! sources (Etherscan V2 ↔ Routescan round-robin, Blockscout last); pricing lives
//! in `prices.rs`.

use crate::blockscout::parse_transfers_rich;
use crate::jsmath::js_num;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// blockscout.js HOSTS — chain id → explorer base URL. `None` for a chain we
/// don't have a Blockscout host for (caller returns an empty list, as the JS does).
/// blockscout.js `bsChains()` = `Object.keys(HOSTS)`, in declaration order. The
/// order is load-bearing: `neighborsOf` flattens per-chain transfers into one
/// list and a stable frequency sort keeps first-seen order on ties.
pub fn bs_chains() -> &'static [&'static str] {
    &[
        "eth", "base", "optimism", "gnosis", "polygon", "arbitrum", "zksync", "scroll", "mode",
        "fraxtal", "blast", "linea", "zora", "celo", "mantle", "sonic", "rootstock", "fuse",
        "metis", "aurora", "lisk", "redstone", "immutable", "lukso",
    ]
}

fn host(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" => "https://eth.blockscout.com",
        "base" => "https://base.blockscout.com",
        "optimism" => "https://optimism.blockscout.com",
        "gnosis" => "https://gnosis.blockscout.com",
        "polygon" => "https://polygon.blockscout.com",
        "arbitrum" => "https://arbitrum.blockscout.com",
        "zksync" => "https://zksync.blockscout.com",
        "scroll" => "https://scroll.blockscout.com",
        "mode" => "https://explorer.mode.network",
        "fraxtal" => "https://fraxscan.blockscout.com",
        "blast" => "https://blast.blockscout.com",
        "linea" => "https://explorer.linea.build",
        "zora" => "https://explorer.zora.energy",
        "celo" => "https://celo.blockscout.com",
        "mantle" => "https://mantle.blockscout.com",
        "sonic" => "https://sonic.blockscout.com",
        "rootstock" => "https://rootstock.blockscout.com",
        "fuse" => "https://explorer.fuse.io",
        "metis" => "https://andromeda-explorer.metis.io",
        "aurora" => "https://explorer.aurora.dev",
        "lisk" => "https://blockscout.lisk.com",
        "redstone" => "https://explorer.redstone.xyz",
        "immutable" => "https://explorer.immutable.com",
        "lukso" => "https://explorer.execution.mainnet.lukso.network",
        _ => return None,
    })
}

// ── Global polite scheduler ──
// blockscout.js:55 — all callers share ONE budget: at most MAX_CONCURRENT in
// flight, and request starts spaced ≥ MIN_GAP_MS apart. Keeps us off the rate
// limiter without per-caller coordination.
const MAX_CONCURRENT: usize = 7;
const MIN_GAP: Duration = Duration::from_millis(90);

struct Client {
    http: reqwest::Client,
    sem: Semaphore,
    /// Timestamp of the last scheduled request start (min-gap enforcement).
    last_start: Mutex<Instant>,
}

fn client() -> &'static Client {
    static C: OnceLock<Client> = OnceLock::new();
    C.get_or_init(|| Client {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent("wallet-radar/rust")
            .build()
            .expect("reqwest client builds from static config"),
        sem: Semaphore::new(MAX_CONCURRENT),
        last_start: Mutex::new(Instant::now() - MIN_GAP),
    })
}

/// blockscout.js `bget`'s default — 8s covers nearly every endpoint.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
/// …but not `/tokens` for a wallet the size of Vitalik's: that one legitimately takes
/// ~15s. Node survives it only because its `cached()` layer had a warm entry from hours
/// ago; on a cold cache Node times out too, and every "material" position vanishes from
/// the scan. Give the heavy endpoints room instead of inheriting the bug.
const HEAVY_TIMEOUT: Duration = Duration::from_secs(25);

/// One scheduled GET returning parsed JSON. `Ok(None)` signals a retryable
/// rate-limit (429/503); `Err` is a hard failure. Holds a concurrency permit
/// for the whole request and honours the min-gap before starting.
async fn raw_get(url: &str, timeout: Duration) -> anyhow::Result<Option<Value>> {
    let c = client();
    let _permit = c.sem.acquire().await.expect("semaphore is never closed");

    // Reserve this request's start slot, then wait for it — guard dropped before await.
    let wait = {
        let mut last = c.last_start.lock().unwrap();
        let now = Instant::now();
        let start = now.max(*last + MIN_GAP);
        *last = start;
        start.saturating_duration_since(now)
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }

    let resp = c.http.get(url).header("accept", "application/json").timeout(timeout).send().await?;
    let status = resp.status().as_u16();
    if status == 429 || status == 503 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {status}");
    }
    Ok(Some(resp.json::<Value>().await?))
}

/// blockscout.js `cached(key, ttl, fn)` — the response cache the Rust port never had.
/// Without it every call goes to the network, and the slow endpoints fail every time
/// rather than once. In-memory only, deliberately: Node owns the shared dataset/cache.json
/// and two writers would race. Failures are NOT cached (a `[]` must be retried).
fn cache() -> &'static Mutex<std::collections::HashMap<String, (Instant, Value)>> {
    static C: OnceLock<Mutex<std::collections::HashMap<String, (Instant, Value)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn cache_get(key: &str, ttl: Duration) -> Option<Value> {
    let c = cache().lock().unwrap();
    let (at, v) = c.get(key)?;
    (at.elapsed() < ttl).then(|| v.clone())
}

fn cache_set(key: &str, v: &Value) {
    cache().lock().unwrap().insert(key.to_string(), (Instant::now(), v.clone()));
}

/// Той самий кеш, але для викликачів поза bs.rs. У Node `cached()` — один спільний шар на всі
/// модулі (assetflow, ethplorer, cg…), тож заводити тут другий, паралельний, означало б розійтися
/// з оригіналом і мати два місця, де щось «чомусь не кешується».
pub fn cached_value(key: &str, ttl: Duration) -> Option<Value> {
    cache_get(key, ttl)
}

pub fn cache_value(key: &str, v: &Value) {
    cache_set(key, v);
}

/// blockscout.js `bget` — polite GET through the scheduler with backoff+retry on
/// 429/503 so we never earn a ban. Up to 3 attempts.
async fn bget(host: &str, path: &str) -> anyhow::Result<Value> {
    bget_with(host, path, DEFAULT_TIMEOUT).await
}

/// `bget` with an explicit per-request timeout, for the endpoints that are genuinely
/// slow rather than broken (see HEAVY_TIMEOUT).
async fn bget_with(host: &str, path: &str, timeout: Duration) -> anyhow::Result<Value> {
    let url = format!("{host}{path}");
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..3u32 {
        match raw_get(&url, timeout).await {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => {
                // Rate-limited: linear backoff before the next attempt.
                tokio::time::sleep(Duration::from_millis(2000 * (attempt as u64 + 1))).await;
            }
            Err(e) => {
                if attempt == 2 {
                    return Err(e);
                }
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(1500 * (attempt as u64 + 1))).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("HTTP 429 (rate-limited after retries)")))
}

/// blockscout.js `transfersRich` — enriched ERC-20 transfers with attribution,
/// paginated up to `max_pages`. Each page's items are shaped by the golden-tested
/// `parse_transfers_rich`. A failed page stops pagination (returns what we have),
/// matching the JS `catch { break }`.
pub async fn transfers_rich(chain: &str, address: &str, max_pages: u32) -> Vec<Value> {
    let Some(host) = host(chain) else { return vec![] };
    // blockscout.js:228 `cached("tr:…", 300000)`.
    let key = format!("tr:{chain}:{address}:{max_pages}");
    if let Some(v) = cache_get(&key, Duration::from_secs(300)) {
        return v.as_array().cloned().unwrap_or_default();
    }
    let mut out: Vec<Value> = Vec::new();
    let mut params = "?type=ERC-20".to_string();
    // A host we already know is gone: skip its three attempts and their backoff entirely.
    let mut first_page_failed = host_is_dead(chain);
    for page in 0..(if first_page_failed { 0 } else { max_pages }) {
        let path = format!("/api/v2/addresses/{address}/token-transfers{params}");
        // A busy wallet's transfer pages are the other genuinely slow endpoint.
        let d = match bget_with(host, &path, HEAVY_TIMEOUT).await {
            Ok(d) => d,
            Err(e) => {
                // Distinguish "the explorer is gone" from "the wallet has no transfers":
                // a failure on the FIRST page means we learned nothing, and returning []
                // would be a lie shaped exactly like a quiet wallet.
                first_page_failed = page == 0;
                if first_page_failed && is_structural_failure(&e) {
                    mark_host_dead(chain);
                }
                break;
            }
        };
        out.extend(parse_transfers_rich(&d));
        let next = &d["next_page_params"];
        if next.is_null() {
            break;
        }
        params = next_params(&[("type", Value::String("ERC-20".into()))], next);
    }
    if first_page_failed {
        if let Some(v) = es_token_transfers(chain, address, max_pages.min(3)).await {
            tracing::debug!("[tt] {chain} {address} via etherscan tokentx: {} transfers", v.len());
            cache_set(&key, &Value::Array(v.clone()));
            return v;
        }
        tracing::warn!("[tt] {chain} {address}: blockscout down and no tokentx source — ERC-20 history is EMPTY, not quiet");
    }
    cache_set(&key, &Value::Array(out.clone()));
    out
}

/// Rebuild the query string as `new URLSearchParams({ ...base, ...next }).toString()`:
/// the fixed base keys first, then every key of `next_page_params`, each value
/// coerced to a string (numbers via their JSON form) and percent-encoded.
fn next_params(base: &[(&str, Value)], next: &Value) -> String {
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in base {
        ser.append_pair(k, &js_string(v));
    }
    if let Some(obj) = next.as_object() {
        for (k, v) in obj {
            ser.append_pair(k, &js_string(v));
        }
    }
    format!("?{}", ser.finish())
}

/// A JSON value as a URLSearchParams value: strings verbatim, other scalars via
/// their JSON text, `null` as the empty string (JS `String(null)` is "null" but
/// Blockscout never returns null cursor fields, so empty is the safe minimum).
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ── Token metadata / holders / address attribution (Blockscout, deterministic) ──

/// JS truthiness of a JSON value (for the `a || b || c` fallbacks below).
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `Number(v)` — number as-is, decimal string parsed, bool 0/1, null 0, else NaN.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() { 0.0 } else { t.parse().unwrap_or(f64::NAN) }
        }
        Value::Bool(b) => if *b { 1.0 } else { 0.0 },
        Value::Null => 0.0,
        _ => f64::NAN,
    }
}

/// blockscout.js `tokenInfo` — Ethplorer-shaped token metadata. Errors propagate
/// (the concentration route turns them into a 502, as Node does).
pub async fn token_info(contract: &str) -> anyhow::Result<Value> {
    // blockscout.js:111 `cached("bti:…", 1800000)`. token_info смикають з кількох місць одразу
    // (asset-flow, token_holders, strategy), тож без кешу той самий запит летить по кілька разів
    // у межах одного розрахунку.
    let ck = format!("bti:{contract}");
    if let Some(v) = cache_get(&ck, Duration::from_secs(1800)) {
        return Ok(v);
    }
    let host = host("eth").expect("eth host is present");
    let d = bget(host, &format!("/api/v2/tokens/{contract}")).await?;
    // Number(d.holders || d.holders_count || 0)
    let holders = if truthy(&d["holders"]) { &d["holders"] } else { &d["holders_count"] };
    let decimals = if truthy(&d["decimals"]) { num(&d["decimals"]) } else { 18.0 };
    let out = json!({
        "name": d["name"].as_str().unwrap_or(""),
        "symbol": d["symbol"].as_str().unwrap_or(""),
        "holdersCount": js_num(num(holders)),
        "decimals": js_num(decimals),
        "totalSupply": d["total_supply"].as_str().unwrap_or("0"),
    });
    // Кешуємо лише успіх: помилка bget() виходить через `?` вище й до кешу не потрапляє.
    cache_set(&ck, &out);
    Ok(out)
}

/// blockscout.js `tokenHolders` — top holders with resilient share normalisation
/// (`value / max(totalSupply, fetchedSum)`), paginated up to `limit`.
pub async fn token_holders(contract: &str, limit: usize) -> Value {
    // blockscout.js:120 `cached("bth:…", 1800000)`. Пагінований запит на 40-50 холдерів, і його
    // смикають ТРИ роути (asset-flow, concentration, asset-groups) — щоразу заново.
    let ck = format!("bth:{contract}:{limit}");
    if let Some(v) = cache_get(&ck, Duration::from_secs(1800)) {
        return v;
    }
    let v = token_holders_uncached(contract, limit).await;
    // Порожній список холдерів — це «Blockscout не відповів», а не «власників немає».
    if v["holders"].as_array().is_some_and(|a| !a.is_empty()) {
        cache_set(&ck, &v);
    }
    v
}

async fn token_holders_uncached(contract: &str, limit: usize) -> Value {
    let host = host("eth").expect("eth host is present");
    // supplyRaw = meta && meta.totalSupply ? Number(meta.totalSupply) : 0
    let supply_raw = match token_info(contract).await {
        Ok(m) if truthy(&m["totalSupply"]) => num(&m["totalSupply"]),
        _ => 0.0,
    };

    // raw holders: (address, value, isContract, name)
    let mut raw: Vec<(String, f64, bool, String)> = Vec::new();
    let mut params = String::new();
    let max_pages = limit.div_ceil(50) + 1;
    for _ in 0..max_pages {
        if raw.len() >= limit {
            break;
        }
        let path = format!("/api/v2/tokens/{contract}/holders{params}");
        let d = match bget(host, &path).await {
            Ok(d) => d,
            Err(_) => break,
        };
        if let Some(items) = d["items"].as_array() {
            for it in items {
                raw.push((
                    it["address"]["hash"].as_str().unwrap_or("").to_lowercase(),
                    num(&it["value"]),
                    it["address"]["is_contract"].as_bool().unwrap_or(false),
                    it["address"]["name"].as_str().unwrap_or("").to_string(),
                ));
                if raw.len() >= limit {
                    break;
                }
            }
        }
        let next = &d["next_page_params"];
        if next.is_null() {
            break;
        }
        params = next_params(&[], next);
    }

    let fetched_sum: f64 = raw.iter().map(|h| h.1).sum();
    // denom = Math.max(supplyRaw, fetchedSum) || 1
    let denom = {
        let m = supply_raw.max(fetched_sum);
        if m == 0.0 || m.is_nan() { 1.0 } else { m }
    };
    let holders: Vec<Value> = raw
        .into_iter()
        .map(|(address, value, is_contract, name)| {
            json!({
                "address": address,
                "share": crate::jsmath::js_num(crate::jsmath::round_fixed(value / denom * 100.0, 4)),
                "isContract": is_contract,
                "name": name,
            })
        })
        .collect();
    json!({ "holders": holders })
}

/// blockscout.js `addressMeta` — name / contract flag / public tags. `None` on
/// any failure, as the JS `catch { return null }` does.
pub async fn address_meta(chain: &str, address: &str) -> Option<Value> {
    // blockscout.js:43 `cached("meta:…", 3600000)`. Найдовший TTL у Node не випадково: імена,
    // теги й ознака контракту майже не змінюються, а concentration смикає це по 50 адрес одразу.
    let ck = format!("meta:{chain}:{address}");
    if let Some(v) = cache_get(&ck, Duration::from_secs(3600)) {
        return Some(v);
    }
    let host = host(chain)?;
    let d = bget(host, &format!("/api/v2/addresses/{address}")).await.ok()?;
    let name = if truthy(&d["name"]) {
        d["name"].as_str().unwrap_or("")
    } else {
        d["ens_domain_name"].as_str().unwrap_or("")
    };
    let tags: Vec<Value> = d["public_tags"]
        .as_array()
        .map(|a| a.iter().map(|t| t["display_name"].clone()).collect())
        .unwrap_or_default();
    let out = json!({ "name": name, "isContract": d["is_contract"].as_bool().unwrap_or(false), "tags": tags });
    // Кешуємо лише успіх: збій bget() виходить через `?` вище.
    cache_set(&ck, &out);
    Some(out)
}

// ── Native transfers ──
// blockscout.js `nativeTransfers` offloads native (ETH/L2) fetching to the
// Etherscan↔Routescan round-robin, with Blockscout as the last resort. Both
// upstreams return the same on-chain txlist, so either will do — but only if it
// answers at all. Relying on Routescan alone left every funder-derived feature
// (asset-groups, group-coordination, whale-finder groups) silently empty once
// its keyless quota ran out and it started replying 429.

/// routescan.js CHAINID — chains Routescan's Etherscan-compatible API covers.
/// routescan.js CHAINID — the chains Routescan actually indexes. The other seven this
/// map used to claim (base, arbitrum, optimism, polygon, gnosis, linea, scroll) answer
/// `{"status":"0","message":"chain not supported"}`, so every native fetch on them
/// spent a serialized 220ms slot on a request that could never succeed.
fn routescan_cid(chain: &str) -> Option<u64> {
    Some(match chain {
        "eth" => 1,
        "blast" => 81457,
        "mantle" => 5000,
        _ => return None,
    })
}

/// etherscan.js CHAINID — chains the keyed Etherscan V2 API answers ON OUR PLAN.
/// V2 lists 64 chains, but the free tier refuses base (8453) and optimism (10) with
/// "Free API access is not supported for this chain" — listing them here only bought
/// a wasted request and a gate slot before the fall-through. The seven added chains
/// were each verified live with our key; they were silently on Blockscout's 50-item
/// pages while a 1000-item source was available. Re-probe before editing: coverage is
/// Etherscan's pricing decision, not a constant.
fn etherscan_cid(chain: &str) -> Option<u64> {
    Some(match chain {
        "eth" => 1,
        "arbitrum" => 42161,
        "polygon" => 137,
        "gnosis" => 100,
        "linea" => 59144,
        "blast" => 81457,
        "mantle" => 5000,
        "fraxtal" => 252,
        "celo" => 42220,
        "sonic" => 146,
        _ => return None,
    })
}

/// etherscan.js `etherscanEnabled` — the key from .env, or None when unset/empty.
fn etherscan_key() -> Option<&'static str> {
    static K: OnceLock<Option<String>> = OnceLock::new();
    K.get_or_init(|| std::env::var("ETHERSCAN_API_KEY").ok().filter(|s| !s.is_empty()))
        .as_deref()
}

/// A `schedule`-style gate: serialize one upstream's calls ≥220ms apart. Etherscan
/// and Routescan each get their own (their rate limits are independent).
fn gate_wait(g: &'static OnceLock<Mutex<Instant>>) -> Duration {
    let gate = g.get_or_init(|| Mutex::new(Instant::now() - Duration::from_millis(220)));
    let mut last = gate.lock().unwrap();
    let now = Instant::now();
    let start = now.max(*last + Duration::from_millis(220));
    *last = start;
    start.saturating_duration_since(now)
}

/// etherscan.js and routescan.js both give their fetch a 12s AbortController. The shared
/// reqwest client defaults to 8s, and a 1000-row txlist page for a wallet the size of
/// Vitalik's does not come back in 8 — so the Rust port timed out where Node read the
/// page fine, then "fell back" for a reason that never existed upstream.
const TXLIST_TIMEOUT: Duration = Duration::from_secs(12);

/// Чи доступний Etherscan узагалі: або ключ лежить тут (старий прямий режим), або є
/// сателіти, у яких він лежить замість нас.
fn etherscan_available() -> bool {
    crate::satellite::strict() || etherscan_key().is_some()
}

/// etherscan.js `esGet` — Etherscan V2. Note the V1 host (`/api?…`) is deprecated
/// and answers NOTOK, so the chainid-carrying V2 path is the only one that works.
///
/// У суворому режимі (сателіти налаштовані) запит іде на сателіт, і ключ підставляє ВІН.
/// Лічильник швидкості лишається на сателіті — ліміт належить АКАУНТУ провайдера, а не нашому
/// процесу, тож гейт має стояти там, де живе ключ (див. satellite/src/registry.rs).
/// Логіка повторів при ліміті лишається тут: це рішення викликача, а не транспорту.
async fn es_get(query: &[(&str, String)], key: Option<&str>) -> Option<Value> {
    if crate::satellite::strict() {
        for attempt in 0..3u32 {
            let d = crate::satellite::fetch_json("etherscan", "/v2/api", query).await.ok()?;
            if !is_rate_limited(&d) {
                return Some(d);
            }
            // Мовчазний повтор — саме те, через що ~45% відхилених викликів жили непоміченими
            // місяцями: дані врешті приходили, тож ніщо не виглядало зламаним, і ліміт у реєстрі
            // лишався хибним. Тепер перевищення ВИДНО в логах, а не лише в рахунку за латентність.
            tracing::warn!("[es] Etherscan відхилив за лімітом (спроба {}/3) — сателіт жене понад квоту акаунта?", attempt + 1);
            tokio::time::sleep(Duration::from_millis(1000 * (attempt as u64 + 1))).await;
        }
        tracing::error!("[es] Etherscan відхиляв за лімітом усі 3 спроби — запит втрачено");
        return None;
    }

    let key = key?;
    static G: OnceLock<Mutex<Instant>> = OnceLock::new();
    let mut wait = gate_wait(&G);
    let params: String = query
        .iter()
        .map(|(k, v)| format!("{k}={}", url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>()))
        .collect::<Vec<_>>()
        .join("&");
    let url = format!("https://api.etherscan.io/v2/api?{params}&apikey={key}");
    // Etherscan answers a burst over its 5/s allowance with HTTP 200 + a NOTOK body
    // ("Max rate limit reached"). Backing off and retrying is the right move: giving
    // up would drop us to Blockscout, whose 50-item pages silently truncate the
    // funder history and cost us real cluster members.
    for attempt in 0..3u32 {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let Ok(resp) =
            client().http.get(&url).header("accept", "application/json").timeout(TXLIST_TIMEOUT).send().await
        else {
            return None;
        };
        if !resp.status().is_success() {
            return None;
        }
        let Ok(d) = resp.json::<Value>().await else { return None };
        if !is_rate_limited(&d) {
            return Some(d);
        }
        tokio::time::sleep(Duration::from_millis(1000 * (attempt as u64 + 1))).await;
        wait = gate_wait(&G);
    }
    None
}

/// An Etherscan-dialect rate-limit refusal: HTTP 200, `status: "0"`, and a `result`
/// that says so. Distinct from `"No transactions found"`, which is a real answer.
fn is_rate_limited(d: &Value) -> bool {
    if d["status"].as_str() == Some("1") {
        return false;
    }
    let result = d["result"].as_str().unwrap_or("").to_lowercase();
    result.contains("rate limit") || result.contains("too many")
}

/// routescan.js `schedule` — serialize all Routescan calls ≥220ms apart.
fn rs_gate() -> &'static Mutex<Instant> {
    static G: OnceLock<Mutex<Instant>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(Instant::now() - Duration::from_millis(220)))
}

async fn rs_get(cid: u64, params: &str) -> Option<Value> {
    let wait = {
        let mut last = rs_gate().lock().unwrap();
        let now = Instant::now();
        let start = now.max(*last + Duration::from_millis(220));
        *last = start;
        start.saturating_duration_since(now)
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    let url = format!("https://api.routescan.io/v2/network/mainnet/evm/{cid}/etherscan/api?{params}");
    let resp =
        client().http.get(&url).header("accept", "application/json").timeout(TXLIST_TIMEOUT).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Value>().await.ok()
}

/// blockscout.js `tokenTransfers` — a wallet's ERC-20 transfers (single page),
/// shaped for the live feed with txHash/logIndex/block/valueUsd (live token rate).
pub async fn token_transfers(chain: &str, address: &str) -> Vec<Value> {
    let Some(host) = host(chain) else { return vec![] };
    let d = match bget(host, &format!("/api/v2/addresses/{address}/token-transfers?type=ERC-20")).await {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    let empty = vec![];
    d["items"].as_array().unwrap_or(&empty).iter().map(|it| {
        let dec = if truthy(&it["total"]["decimals"]) { num(&it["total"]["decimals"]) } else { 18.0 };
        let amount = num(&it["total"]["value"]) / 10f64.powf(dec);
        let rate = if truthy(&it["token"]["exchange_rate"]) { num(&it["token"]["exchange_rate"]) } else { 0.0 };
        let ts = crate::blockscout::date_parse_ms(it["timestamp"].as_str().unwrap_or(""));
        let sym = it["token"]["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?");
        json!({
            "txHash": it["transaction_hash"].as_str().unwrap_or(""),
            "logIndex": if it["log_index"].is_null() { json!(0) } else { it["log_index"].clone() },
            "block": js_num(num(&it["block_number"])),
            "ts": js_num(ts as f64),
            "from": it["from"]["hash"].as_str().unwrap_or("").to_lowercase(),
            "to": it["to"]["hash"].as_str().unwrap_or("").to_lowercase(),
            "asset": sym,
            "amount": js_num(amount),
            "valueUsd": js_num(crate::jsmath::js_round(amount * rate)),
        })
    }).collect()
}

/// blockscout.js `tokenTransfersByToken` — a wallet's transfers of ONE token
/// (spam-free), shaped `{from, to, amount, ts}`, paginated. Used by the strategy
/// backtest's event reconstruction.
pub async fn token_transfers_by_token(chain: &str, address: &str, contract: &str, max_pages: u32) -> Vec<Value> {
    // blockscout.js:311 `cached("ttt:…", 300000)`.
    let ck = format!("ttt:{chain}:{address}:{contract}:{max_pages}");
    if let Some(v) = cache_get(&ck, Duration::from_secs(300)) {
        return v.as_array().cloned().unwrap_or_default();
    }
    let out = token_transfers_by_token_uncached(chain, address, contract, max_pages).await;
    if !out.is_empty() {
        cache_set(&ck, &Value::Array(out.clone()));
    }
    out
}

async fn token_transfers_by_token_uncached(chain: &str, address: &str, contract: &str, max_pages: u32) -> Vec<Value> {
    let Some(host) = host(chain) else { return vec![] };
    let mut out = Vec::new();
    let mut params = format!("?type=ERC-20&token={contract}");
    for _ in 0..max_pages {
        let d = match bget(host, &format!("/api/v2/addresses/{address}/token-transfers{params}")).await {
            Ok(d) => d,
            Err(_) => break,
        };
        for it in d["items"].as_array().unwrap_or(&vec![]) {
            let total = &it["total"];
            let dec = if truthy(&total["decimals"]) { num(&total["decimals"]) } else { 18.0 };
            let amount = num(&total["value"]) / 10f64.powf(dec);
            if amount == 0.0 {
                continue; // !amount
            }
            out.push(json!({
                "from": it["from"]["hash"].as_str().unwrap_or("").to_lowercase(),
                "to": it["to"]["hash"].as_str().unwrap_or("").to_lowercase(),
                "amount": js_num(amount),
                "ts": js_num(crate::blockscout::date_parse_ms(it["timestamp"].as_str().unwrap_or("")) as f64),
            }));
        }
        let next = &d["next_page_params"];
        if next.is_null() {
            break;
        }
        params = next_params(&[("type", Value::String("ERC-20".into())), ("token", Value::String(contract.to_string()))], next);
    }
    out
}

/// blockscout.js `tokenTransfersGlobal` — every transfer OF a token (eth host),
/// paginated. Unlike `token_transfers_by_token` this carries `fromName`/`toName`
/// for CEX classification. Used by `/api/exchange-netflow`.
pub async fn token_transfers_global(contract: &str, max_pages: u32) -> Vec<Value> {
    // blockscout.js:283 `cached("ttg:…", 300000)`.
    let ck = format!("ttg:{contract}:{max_pages}");
    if let Some(v) = cache_get(&ck, Duration::from_secs(300)) {
        return v.as_array().cloned().unwrap_or_default();
    }
    let out = token_transfers_global_uncached(contract, max_pages).await;
    if !out.is_empty() {
        cache_set(&ck, &Value::Array(out.clone()));
    }
    out
}

async fn token_transfers_global_uncached(contract: &str, max_pages: u32) -> Vec<Value> {
    let host = host("eth").expect("eth host is present");
    let mut out = Vec::new();
    let mut params = String::new();
    for _ in 0..max_pages {
        let d = match bget(host, &format!("/api/v2/tokens/{contract}/transfers{params}")).await {
            Ok(d) => d,
            Err(_) => break,
        };
        for it in d["items"].as_array().unwrap_or(&vec![]) {
            let total = &it["total"];
            let dec = if truthy(&total["decimals"]) { num(&total["decimals"]) } else { 18.0 };
            let amount = num(&total["value"]) / 10f64.powf(dec);
            if amount == 0.0 {
                continue; // !amount
            }
            out.push(json!({
                "from": it["from"]["hash"].as_str().unwrap_or("").to_lowercase(),
                "to": it["to"]["hash"].as_str().unwrap_or("").to_lowercase(),
                "fromName": it["from"]["name"].as_str().unwrap_or(""),
                "toName": it["to"]["name"].as_str().unwrap_or(""),
                "amount": js_num(amount),
                "ts": js_num(crate::blockscout::date_parse_ms(it["timestamp"].as_str().unwrap_or("")) as f64),
            }));
        }
        let next = &d["next_page_params"];
        if next.is_null() {
            break;
        }
        params = next_params(&[], next);
    }
    out
}

// ── Wallet balances (tokens + native), Blockscout with an RPC fallback ──

/// blockscout.js `NATIVE_SYM` — chain → native-coin ticker (default ETH).
fn native_sym(chain: &str) -> &'static str {
    match chain {
        "polygon" => "MATIC",
        "gnosis" => "xDAI",
        "bnb" => "BNB",
        "celo" => "CELO",
        "avalanche" => "AVAX",
        "mantle" => "MNT",
        "sonic" => "S",
        _ => "ETH",
    }
}

/// rpc.js `RPC` — public node URLs per chain (no keys). `None` = unsupported.
fn rpc_urls(chain: &str) -> Option<&'static [&'static str]> {
    Some(match chain {
        "eth" => &["https://ethereum-rpc.publicnode.com", "https://rpc.ankr.com/eth", "https://cloudflare-eth.com"],
        "arbitrum" => &["https://arbitrum-one-rpc.publicnode.com", "https://arb1.arbitrum.io/rpc"],
        "base" => &["https://base-rpc.publicnode.com", "https://mainnet.base.org"],
        "optimism" => &["https://optimism-rpc.publicnode.com", "https://mainnet.optimism.io"],
        "polygon" => &["https://polygon-bor-rpc.publicnode.com", "https://polygon-rpc.com"],
        "bnb" => &["https://bsc-rpc.publicnode.com", "https://bsc-dataseed.binance.org"],
        "avalanche" => &["https://avalanche-c-chain-rpc.publicnode.com", "https://api.avax.network/ext/bc/C/rpc"],
        "mantle" => &["https://mantle-rpc.publicnode.com", "https://rpc.mantle.xyz"],
        "sonic" => &["https://sonic-rpc.publicnode.com", "https://rpc.soniclabs.com"],
        "flare" => &["https://flare-api.flare.network/ext/C/rpc"],
        _ => return None,
    })
}

/// rpc.js `evmChains()` — the chains with a public RPC node, in declaration order.
pub fn rpc_chains() -> &'static [&'static str] {
    &["eth", "arbitrum", "base", "optimism", "polygon", "bnb", "avalanche", "mantle", "sonic", "flare"]
}

/// rpc.js `rpcHas(chain)`.
pub fn rpc_has(chain: &str) -> bool {
    rpc_urls(chain).is_some()
}

/// rpc.js `nativeBalance` — native-coin amount (float) via `eth_getBalance`,
/// trying each node until one answers. `None` if the chain is unsupported or all
/// nodes fail. `Number(BigInt(hex))/1e18` — hex wei parsed then scaled (f64-lossy,
/// exactly as Node's `Number()` on the BigInt).
pub async fn rpc_native_amount(chain: &str, address: &str) -> Option<f64> {
    let urls = rpc_urls(chain)?;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "eth_getBalance", "params": [address, "latest"]});
    for url in urls {
        let resp = match client().http.post(*url).header("content-type", "application/json").json(&body).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };
        let d: Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(hex) = d["result"].as_str().and_then(|h| h.strip_prefix("0x")) {
            if let Ok(wei) = u128::from_str_radix(hex, 16) {
                return Some(wei as f64 / 1e18);
            }
        }
    }
    None
}

/// blockscout.js `nativeBalance` — native coin valued in USD. Blockscout's
/// `coin_balance × exchange_rate` when positive; otherwise (or for chains with no
/// Blockscout host, e.g. bnb/avalanche) the RPC amount × DefiLlama price. `None`
/// when nothing prices out. Returns `{symbol, valueUsd}`.
pub async fn native_balance(chain: &str, address: &str) -> Option<Value> {
    // blockscout.js:203/204 `cached("nb:…", 600000)`.
    let ck = format!("nb:{chain}:{address}");
    if let Some(v) = cache_get(&ck, Duration::from_secs(600)) {
        return Some(v);
    }
    let v = native_balance_uncached(chain, address).await;
    if let Some(ref v) = v {
        cache_set(&ck, v);
    }
    v
}

async fn native_balance_uncached(chain: &str, address: &str) -> Option<Value> {
    let via_rpc = || async {
        let amt = rpc_native_amount(chain, address).await?;
        if !(amt > 0.0) {
            return None;
        }
        let px = crate::prices::native_price_usd(chain).await;
        (px > 0.0).then(|| json!({"symbol": native_sym(chain), "valueUsd": js_num(amt * px)}))
    };

    let Some(host) = host(chain) else { return via_rpc().await };
    match bget(host, &format!("/api/v2/addresses/{address}")).await {
        Ok(d) => {
            let bal = num(&d["coin_balance"]) / 1e18; // Number(coin_balance||0) — null→0
            let rate = if truthy(&d["exchange_rate"]) { num(&d["exchange_rate"]) } else { 0.0 };
            if bal * rate > 0.0 {
                Some(json!({"symbol": native_sym(chain), "valueUsd": js_num(bal * rate)}))
            } else {
                via_rpc().await
            }
        }
        Err(_) => via_rpc().await,
    }
}

/// blockscout.js `tokenBalances` — priced ERC-20 holdings only (`usd > 0`),
/// shaped `{symbol, valueUsd}`. Used by the smart-money feature extractor.
pub async fn token_balances(chain: &str, address: &str) -> Vec<Value> {
    // blockscout.js:147 `cached("tb:…", 600000)`.
    let ck = format!("tb:{chain}:{address}");
    if let Some(v) = cache_get(&ck, Duration::from_secs(600)) {
        return v.as_array().cloned().unwrap_or_default();
    }
    let out = token_balances_uncached(chain, address).await;
    if !out.is_empty() {
        cache_set(&ck, &Value::Array(out.clone()));
    }
    out
}

async fn token_balances_uncached(chain: &str, address: &str) -> Vec<Value> {
    let Some(host) = host(chain) else { return vec![] };
    let d = match bget(host, &format!("/api/v2/addresses/{address}/tokens?type=ERC-20")).await {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    let mut out = Vec::new();
    for it in d["items"].as_array().unwrap_or(&vec![]) {
        let tok = &it["token"];
        let dec = if truthy(&tok["decimals"]) { num(&tok["decimals"]) } else { 18.0 };
        let bal = num(&it["value"]) / 10f64.powf(dec);
        let rate = if truthy(&tok["exchange_rate"]) { num(&tok["exchange_rate"]) } else { 0.0 };
        let usd = bal * rate;
        if usd > 0.0 {
            let symbol = tok["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?").to_uppercase();
            out.push(json!({"symbol": symbol, "valueUsd": js_num(usd)}));
        }
    }
    out
}

/// blockscout.js `tokenBalancesRaw` — all ERC-20 holdings (even unpriced ones, so
/// DefiLlama can reprice them). Each item is `{symbol, contract, balance, usd}`
/// with `usd = balance × Blockscout exchange_rate` (0 when unrated). Zero-balance
/// rows are dropped. A failed fetch yields an empty list.
pub async fn token_balances_raw(chain: &str, address: &str) -> Vec<Value> {
    let Some(host) = host(chain) else { return vec![] };
    // blockscout.js:166 `cached("tbr:…", 600000)`.
    let key = format!("tbr:{chain}:{address}");
    if let Some(v) = cache_get(&key, Duration::from_secs(600)) {
        return v.as_array().cloned().unwrap_or_default();
    }
    // A host already known to be gone: don't pay bget's three attempts to relearn it.
    let attempt = if host_is_dead(chain) {
        Err(anyhow::anyhow!("HTTP 404 (host known dead)"))
    } else {
        bget_with(host, &format!("/api/v2/addresses/{address}/tokens?type=ERC-20"), HEAVY_TIMEOUT).await
    };
    let d = match attempt {
        Ok(d) => d,
        Err(e) => {
            if is_structural_failure(&e) {
                mark_host_dead(chain);
            }
            // The explorer is gone, not the holdings. Returning [] here is what made a
            // wallet's linea/blast/mantle/sonic positions invisible in every balance we
            // report — an empty list reads as "owns nothing", which is a lie.
            if let Some(v) = crate::moralis::token_balances(chain, address).await {
                tracing::debug!("[tb] {chain} {address} via moralis: {} tokens", v.len());
                cache_set(&key, &Value::Array(v.clone()));
                return v;
            }
            tracing::warn!(
                "[tb] {chain} {address}: blockscout down and moralis does not cover this chain — \
                 token balances are UNKNOWN, reported as empty"
            );
            return vec![];
        }
    };
    let mut out = Vec::new();
    for it in d["items"].as_array().unwrap_or(&vec![]) {
        let tok = &it["token"];
        // Number(token.decimals || 18): JS-truthy decimals kept, else 18.
        let dec = if truthy(&tok["decimals"]) { num(&tok["decimals"]) } else { 18.0 };
        let bal = num(&it["value"]) / 10f64.powf(dec);
        if bal <= 0.0 {
            continue;
        }
        let rate = if truthy(&tok["exchange_rate"]) { num(&tok["exchange_rate"]) } else { 0.0 };
        let symbol = tok["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?").to_uppercase();
        // token.address || token.address_hash || "" , lowercased
        let contract = [tok["address"].as_str(), tok["address_hash"].as_str()]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .to_lowercase();
        out.push(json!({"symbol": symbol, "contract": contract, "balance": js_num(bal), "usd": js_num(bal * rate)}));
    }
    cache_set(&key, &Value::Array(out.clone()));
    out
}

/// One page of an Etherscan-compatible `txlist` (Etherscan V2 and Routescan speak
/// the same dialect) → transfers shaped like the Blockscout branch.
///
/// The `Option` is the whole point: `None` means the SOURCE FAILED (HTTP error,
/// 429, malformed body) and the caller must try another one; `Some` means the
/// source answered, even if it answered with nothing. Collapsing those two into an
/// empty list is what silently blinded both servers — see `native_transfers`.
fn parse_txlist_page(d: &Value) -> Option<Vec<Value>> {
    let result = match d["result"].as_array() {
        Some(r) if d["status"].as_str() == Some("1") => r,
        // "No transactions found" is a legitimate empty answer; status 0 with any
        // other message (NOTOK, rate limit, deprecated endpoint) is a failure.
        _ => {
            let msg = d["message"].as_str().unwrap_or("");
            return if msg.contains("No transactions found") { Some(Vec::new()) } else { None };
        }
    };
    let mut out = Vec::with_capacity(result.len());
    for it in result {
        let amount = it["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18;
        if amount == 0.0 {
            continue;
        }
        let ts = it["timeStamp"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * 1000.0;
        out.push(json!({
            "from": it["from"].as_str().unwrap_or("").to_lowercase(),
            "to": it["to"].as_str().unwrap_or("").to_lowercase(),
            "fromName": "", "toName": "", "fromContract": false, "toContract": false,
            "asset": "native", "token": "", "amount": js_num(amount), "native": true, "usd": 0,
            "ts": js_num(ts),
            "txHash": it["hash"].as_str().unwrap_or(""),
            "block": it["blockNumber"].as_str().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0),
        }));
    }
    // A short page means there is no next one — the caller stops paginating.
    Some(out)
}

/// etherscan.js `esNativeTransfers` — Etherscan V2 (`/v2/api?chainid=…`), keyed.
/// `None` = no key / unsupported chain / the fetch failed.
async fn es_native_transfers(chain: &str, address: &str, pages: u32) -> Option<Vec<Value>> {
    let cid = etherscan_cid(chain)?;
    if !etherscan_available() {
        return None;
    }
    let key = etherscan_key();
    let mut out: Vec<Value> = Vec::new();
    for page in 1..=pages {
        let q = [
            ("chainid", cid.to_string()),
            ("module", "account".into()),
            ("action", "txlist".into()),
            ("address", address.to_string()),
            ("startblock", "0".into()),
            ("endblock", "99999999".into()),
            ("page", page.to_string()),
            ("offset", "1000".into()),
            ("sort", "desc".into()),
        ];
        let d = es_get(&q, key).await?;
        let items = parse_txlist_page(&d)?;
        let full = d["result"].as_array().map_or(0, |r| r.len()) == 1000;
        out.extend(items);
        if !full {
            break;
        }
    }
    Some(out)
}

/// One page of Etherscan V2 `action=tokentx` → the `parse_transfers_rich` shape.
/// `None` on a source failure, `Some(vec![])` on a genuine empty answer — the same
/// distinction `parse_txlist_page` makes, and for the same reason.
///
/// The txlist dialect carries no counterparty names or is_contract flags, so those come
/// back empty exactly as they do on the native Etherscan path. `usd` is left at 0 here
/// and filled by the caller, which prices the whole page in one batch.
fn parse_tokentx_page(d: &Value) -> Option<Vec<Value>> {
    let result = match d["result"].as_array() {
        Some(r) if d["status"].as_str() == Some("1") => r,
        _ => {
            let msg = d["message"].as_str().unwrap_or("");
            return if msg.contains("No transactions found") { Some(Vec::new()) } else { None };
        }
    };
    let mut out = Vec::with_capacity(result.len());
    for it in result {
        let dec = it["tokenDecimal"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(18.0);
        let amount = it["value"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 10f64.powf(dec);
        if amount == 0.0 {
            continue;
        }
        let ts = it["timeStamp"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * 1000.0;
        let sym = it["tokenSymbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?");
        out.push(json!({
            "from": it["from"].as_str().unwrap_or("").to_lowercase(),
            "to": it["to"].as_str().unwrap_or("").to_lowercase(),
            "fromName": "", "toName": "", "fromContract": false, "toContract": false,
            "asset": sym,
            "token": it["contractAddress"].as_str().unwrap_or("").to_lowercase(),
            "amount": js_num(amount),
            "usd": 0,
            "hash": it["hash"].as_str().unwrap_or("").to_lowercase(),
            "ts": js_num(ts),
        }));
    }
    Some(out)
}

/// Etherscan V2 `action=tokentx` — ERC-20 history from the keyed source we already use
/// for native transfers. New in the port (Node had no equivalent): the Blockscout
/// instances for linea/blast/mantle/sonic/fraxtal are GONE (404), and Blockscout was the
/// ONLY source of token transfers, so those chains returned an empty history that looked
/// exactly like a wallet that never traded.
async fn es_token_transfers(chain: &str, address: &str, pages: u32) -> Option<Vec<Value>> {
    let cid = etherscan_cid(chain)?;
    if !etherscan_available() {
        return None;
    }
    let key = etherscan_key();
    let mut out: Vec<Value> = Vec::new();
    for page in 1..=pages {
        let q = [
            ("chainid", cid.to_string()),
            ("module", "account".into()),
            ("action", "tokentx".into()),
            ("address", address.to_string()),
            ("page", page.to_string()),
            ("offset", "1000".into()),
            ("sort", "desc".into()),
        ];
        let d = es_get(&q, key).await?;
        let items = parse_tokentx_page(&d)?;
        let full = d["result"].as_array().map_or(0, |r| r.len()) == 1000;
        out.extend(items);
        if !full {
            break;
        }
    }
    // Blockscout hands us an exchange_rate per token; the txlist dialect does not. Without
    // this the whole history would arrive valued at $0 — present, but worthless to every
    // flow/whale metric downstream.
    let pairs: Vec<(String, String)> = out
        .iter()
        .map(|t| (chain.to_string(), t["token"].as_str().unwrap_or("").to_string()))
        .filter(|(_, c)| !c.is_empty())
        .collect();
    let px = crate::prices::token_prices_current(&pairs).await;
    for t in out.iter_mut() {
        let contract = t["token"].as_str().unwrap_or("").to_string();
        if let Some(p) = px.get(&format!("{chain}:{contract}")) {
            let amount = t["amount"].as_f64().unwrap_or(0.0);
            t["usd"] = json!(js_num(amount * p));
        }
    }
    Some(out)
}

/// routescan.js `rsNativeTransfers` — keyless Routescan, same txlist dialect.
async fn rs_native_transfers(chain: &str, address: &str, pages: u32) -> Option<Vec<Value>> {
    let cid = routescan_cid(chain)?;
    let mut out: Vec<Value> = Vec::new();
    for page in 1..=pages {
        let params = format!(
            "module=account&action=txlist&address={address}&startblock=0&endblock=99999999&page={page}&offset=1000&sort=desc"
        );
        let d = rs_get(cid, &params).await?;
        let items = parse_txlist_page(&d)?;
        let full = d["result"].as_array().map_or(0, |r| r.len()) == 1000;
        out.extend(items);
        if !full {
            break;
        }
    }
    Some(out)
}

/// How long a chain stays marked as "no compat endpoint" after a refusal. Time-boxed
/// on purpose: a permanent mark would let one transient 502 quietly demote a chain to
/// 50-item pages for the rest of the process's life, and a quiet loss of history is
/// exactly the failure this whole source chain exists to prevent.
const COMPAT_RETRY: Duration = Duration::from_secs(1800);

fn compat_dead() -> &'static Mutex<std::collections::HashMap<String, Instant>> {
    static D: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Chains whose Blockscout instance is answering nothing at all (404/403 on every path —
/// linea, blast, mantle, sonic, fraxtal, redstone as of 2026-07-12). Without this every
/// single call to a dead host pays `bget`'s three attempts and their 1.5s + 3s backoff
/// before the fallback even starts, and each of the 20 workers pays it again, on every
/// wallet. Same time-boxing as `compat_dead`: a host that comes back is picked up within
/// half an hour, and one transient failure can never demote a live host permanently.
fn host_dead() -> &'static Mutex<std::collections::HashMap<String, Instant>> {
    static D: OnceLock<Mutex<std::collections::HashMap<String, Instant>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn host_is_dead(chain: &str) -> bool {
    host_dead().lock().unwrap().get(chain).is_some_and(|at| at.elapsed() < COMPAT_RETRY)
}

fn mark_host_dead(chain: &str) {
    host_dead().lock().unwrap().insert(chain.to_string(), Instant::now());
}

/// Is this failure the host telling us the endpoint is GONE, rather than busy?
///
/// The distinction matters more than it looks: the tokentx fallback carries no
/// counterparty names and no `is_contract` flags, so demoting a chain to it costs real
/// attribution. A 429 or a timeout must therefore NOT count — only a structural refusal.
fn is_structural_failure(e: &anyhow::Error) -> bool {
    let s = e.to_string();
    s.contains("HTTP 404") || s.contains("HTTP 403") || s.contains("HTTP 410")
}

/// Blockscout's Etherscan-compatible `/api?module=account&action=txlist` — the SAME
/// dialect `parse_txlist_page` already handles, but 1000 rows per page against v2's 50.
/// This is the only deep native source left for base and optimism, which Etherscan's
/// free tier now refuses and Routescan never indexed; without it their funder history
/// comes back truncated and cluster members silently disappear.
///
/// Not every instance still exposes it (Blockscout deprecated it), so a refusal is
/// remembered for COMPAT_RETRY and the chain skips straight to v2 until then.
async fn bs_txlist_compat(chain: &str, address: &str, pages: u32) -> Option<Vec<Value>> {
    if let Some(at) = compat_dead().lock().unwrap().get(chain) {
        if at.elapsed() < COMPAT_RETRY {
            return None;
        }
    }
    let host = host(chain)?;
    let mut out: Vec<Value> = Vec::new();
    for page in 1..=pages {
        let path = format!(
            "/api?module=account&action=txlist&address={address}&startblock=0&endblock=99999999&page={page}&offset=1000&sort=desc"
        );
        // HEAVY_TIMEOUT, not the default 8s: a full 1000-row page off a busy explorer
        // legitimately takes ~15s, and timing it out would both discard the deepest
        // source and mark the chain dead for half an hour — punishing the endpoint for
        // returning exactly the volume of data we asked it for.
        //
        // A hard failure here is either "this instance has no compat endpoint" (404/HTML)
        // or a flake; both are answered the same way — remember it, fall through, retry
        // in half an hour.
        let d = match bget_with(host, &path, HEAVY_TIMEOUT).await {
            Ok(d) => d,
            Err(_) => {
                compat_dead().lock().unwrap().insert(chain.to_string(), Instant::now());
                return None;
            }
        };
        let Some(items) = parse_txlist_page(&d) else {
            compat_dead().lock().unwrap().insert(chain.to_string(), Instant::now());
            return None;
        };
        let full = d["result"].as_array().map_or(0, |r| r.len()) == 1000;
        out.extend(items);
        if !full {
            break;
        }
    }
    Some(out)
}

/// blockscout.js `nativeTransfers` last resort — the explorer's own tx list.
/// Blockscout is the one source that needs no key and no third-party quota, so it
/// is what keeps native transfers flowing when Etherscan and Routescan are down.
async fn bs_native_transfers(chain: &str, address: &str, max_pages: u32) -> Vec<Value> {
    let Some(host) = host(chain) else { return vec![] };
    let mut out: Vec<Value> = Vec::new();
    let mut params = String::new();
    for _ in 0..max_pages {
        let path = format!("/api/v2/addresses/{address}/transactions{params}");
        let Ok(d) = bget(host, &path).await else { break };
        for it in d["items"].as_array().unwrap_or(&vec![]) {
            let amount = num(&it["value"]) / 1e18;
            if amount == 0.0 {
                continue;
            }
            let name_of = |side: &Value| -> String {
                [side["name"].as_str(), side["ens_domain_name"].as_str()]
                    .into_iter()
                    .flatten()
                    .find(|s| !s.is_empty())
                    .unwrap_or("")
                    .to_string()
            };
            out.push(json!({
                "from": it["from"]["hash"].as_str().unwrap_or("").to_lowercase(),
                "to": it["to"]["hash"].as_str().unwrap_or("").to_lowercase(),
                "fromName": name_of(&it["from"]), "toName": name_of(&it["to"]),
                "fromContract": truthy(&it["from"]["is_contract"]),
                "toContract": truthy(&it["to"]["is_contract"]),
                "asset": "native", "token": "", "amount": js_num(amount), "native": true, "usd": 0,
                "ts": js_num(crate::blockscout::date_parse_ms(it["timestamp"].as_str().unwrap_or("")) as f64),
            }));
        }
        let next = &d["next_page_params"];
        if !truthy(next) {
            break;
        }
        params = next_params(&[], next);
    }
    out
}

/// blockscout.js `nativeTransfers` — native-coin transfers. Etherscan and Routescan
/// alternate (round-robin, ≈2× throughput) and Blockscout is the last resort, so a
/// dead or rate-limited upstream costs coverage, not correctness. `max_pages` is
/// capped at 3 as the JS does before delegating.
pub async fn native_transfers(chain: &str, address: &str, max_pages: u32) -> Vec<Value> {
    let pages = max_pages.min(3);

    // blockscout.js:397 `cached("nt:…", 300000)` — обгортка, яку порт загубив. Ціна пропуску тут
    // найвища з усіх: цей шлях веде в ETHERSCAN, а його ліміт (3/с на АКАУНТ) ми весь день і
    // рятували. Тобто половину тиску на квоту ми створювали собі самі, повторюючи ті самі
    // запити. Невдачі не кешуємо — порожній список мусить бути перепитаний, а не закріплений.
    let key = format!("nt:{chain}:{address}:{pages}");
    if let Some(v) = cache_get(&key, Duration::from_secs(300)) {
        return v.as_array().cloned().unwrap_or_default();
    }

    let v = native_transfers_uncached(chain, address, max_pages, pages).await;
    // Порожній результат НЕ кешуємо: він означає або «переказів немає», або «всі джерела лягли»,
    // а закріпити другий випадок на 5 хвилин — це саме та тиха неправда, проти якої тут усе й
    // написано.
    if !v.is_empty() {
        cache_set(&key, &Value::Array(v.clone()));
    }
    v
}

async fn native_transfers_uncached(chain: &str, address: &str, max_pages: u32, pages: u32) -> Vec<Value> {
    static RR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let es_first = RR.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 2 == 0;
    let order: [&str; 2] = if es_first { ["es", "rs"] } else { ["rs", "es"] };
    for src in order {
        let r = match src {
            "es" => es_native_transfers(chain, address, pages).await,
            _ => rs_native_transfers(chain, address, pages).await,
        };
        if let Some(v) = r {
            tracing::debug!("[nt] {chain} {address} via {src}: {} transfers", v.len());
            return v;
        }
    }
    // Blockscout's own Etherscan-dialect endpoint: same 1000-row depth, no key, no quota.
    if let Some(v) = bs_txlist_compat(chain, address, pages).await {
        tracing::debug!("[nt] {chain} {address} via bs-compat: {} transfers", v.len());
        return v;
    }
    // Only NOW is the warning earned: v2 pages 50 at a time, so a wallet whose history
    // the txlist APIs would have returned in full comes back truncated — funder-derived
    // features quietly lose members.
    let v = bs_native_transfers(chain, address, max_pages).await;
    tracing::warn!(
        "[nt] {chain} {address}: no txlist source (etherscan/routescan/bs-compat all unavailable), \
         fell back to Blockscout v2 50-item pages ({} transfers)",
        v.len()
    );
    v
}

#[cfg(test)]
mod live_tests {
    /// Live probe (network): which native-transfer source actually answers, per chain.
    /// The source maps are a bet on someone else's pricing and indexing decisions, so
    /// they rot silently — this is how you re-check them. Run:
    /// `cargo test -- --ignored --nocapture live_native_sources`
    #[tokio::test]
    #[ignore]
    async fn live_native_sources() {
        const A: &str = "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"; // vitalik.eth
        // The test binary is not the server: nothing has loaded .env for it, so without
        // this the Etherscan key is absent and every keyed source reports "unavailable"
        // for the wrong reason.
        dotenvy::from_path("../.env").ok();
        // Ключ у ../.env більше НЕ лежить — його тримають сателіти. Тож умова тепер «Etherscan
        // узагалі досяжний», а не «ключ у мене». Щоб ганяти цей тест через сателіти, задайте
        // SATELLITE_ENDPOINTS + PKI_DIR; інакше він чесно скаже, що джерела немає, — і це буде
        // правда про конфігурацію, а не хибний провал.
        assert!(
            super::etherscan_available(),
            "Etherscan недосяжний: ані ключа в оточенні, ані SATELLITE_ENDPOINTS. \
             Запускайте так: SATELLITE_ENDPOINTS=... PKI_DIR=... cargo test -- --ignored live_native_sources"
        );
        let mut truncated = Vec::new();
        println!("{:<10} {:>10} {:>10} {:>10}", "chain", "etherscan", "routescan", "bs-compat");
        for chain in super::bs_chains() {
            let es = super::es_native_transfers(chain, A, 1).await;
            let rs = super::rs_native_transfers(chain, A, 1).await;
            let compat = super::bs_txlist_compat(chain, A, 1).await;
            let show = |r: &Option<Vec<serde_json::Value>>| {
                r.as_ref().map_or("—".to_string(), |v| v.len().to_string())
            };
            println!("{:<10} {:>10} {:>10} {:>10}", chain, show(&es), show(&rs), show(&compat));
            if es.is_none() && rs.is_none() && compat.is_none() {
                truncated.push(*chain);
            }
        }
        // Not an assertion on any single chain — upstreams go down. The point is to see
        // WHICH chains are stuck on 50-item pages, because that is where funder history
        // (and with it, cluster membership) quietly goes missing.
        println!("\nбез глибокого джерела (сторінки по 50): {truncated:?}");
    }

    /// Live probe (network): ERC-20 history and token balances on the chains whose
    /// Blockscout instance is dead (404). Before the tokentx/Moralis fallbacks these all
    /// returned empty — a wallet that looked like it had never traded and held nothing.
    /// `cargo test -- --ignored --nocapture live_dead_host_tokens`
    #[tokio::test]
    #[ignore]
    async fn live_dead_host_tokens() {
        const A: &str = "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"; // vitalik.eth
        dotenvy::from_path("../.env").ok();
        println!("{:<9} {:>10} {:>9} {:>10}", "chain", "transfers", "priced", "balances");
        for chain in ["linea", "blast", "mantle", "sonic", "fraxtal"] {
            let tr = super::transfers_rich(chain, A, 1).await;
            let priced = tr.iter().filter(|t| t["usd"].as_f64().unwrap_or(0.0) > 0.0).count();
            let bal = super::token_balances_raw(chain, A).await;
            println!("{:<9} {:>10} {:>9} {:>10}", chain, tr.len(), priced, bal.len());
        }
    }

    /// Live probe (network) — run explicitly: `cargo test -- --ignored live_balances`.
    #[tokio::test]
    #[ignore]
    async fn live_balances() {
        let b = super::token_balances_raw("eth", "0xd8da6bf26964af9d7eed9e03e53415d37aa96045").await;
        println!("token_balances_raw → {} записів", b.len());
        for x in b.iter().take(3) {
            println!("  {x}");
        }
        let big = b.iter().filter(|x| x["usd"].as_f64().unwrap_or(0.0) >= 1000.0).count();
        println!("із usd>=1000: {big}");
    }
}
