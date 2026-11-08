//! Moralis — ERC-20 balances for chains whose Blockscout instance no longer exists.
//!
//! New in the port; Node has no equivalent. `blockscout.js` was the only source of token
//! balances, and the instances for linea/blast/mantle/sonic/fraxtal are gone (404), so a
//! wallet's holdings on those chains came back as an empty list — indistinguishable from
//! a wallet that holds nothing. Four of them (linea, blast, mantle, sonic) sit in
//! `BAL_CHAINS`, so this was a silent hole in every balance figure we report.
//!
//! Moralis only closes part of it: it answers for linea but rejects blast/mantle/sonic
//! outright ("chain must be a valid enum value"). Chains it does not know return `None`
//! here so the caller keeps looking rather than believing an empty list.

use crate::jsmath::js_num;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Moralis' chain enum. Verified live against our key — blast (0x13e31), mantle (0x1388)
/// and sonic (0x92) are deliberately ABSENT: Moralis rejects all three, and listing them
/// would spend a request to learn that every time.
fn chain_param(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" => "0x1",
        "base" => "0x2105",
        "arbitrum" => "0xa4b1",
        "optimism" => "0xa",
        "polygon" => "0x89",
        "gnosis" => "0x64",
        "linea" => "0xe708",
        _ => return None,
    })
}

fn api_key() -> Option<&'static str> {
    static K: OnceLock<Option<String>> = OnceLock::new();
    K.get_or_init(|| std::env::var("MORALIS_API_KEY").ok().filter(|s| !s.is_empty())).as_deref()
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("wallet-radar/rust")
            .build()
            .expect("reqwest client builds from static config")
    })
}

/// `result[]` → the `token_balances_raw` shape `{symbol, contract, balance, usd}`.
///
/// Two things must be dropped or the totals lie:
/// - `native_token` entries (Moralis reports the native coin as 0xeee…eee in this list,
///   while Blockscout's `/tokens` is ERC-20 only and the native balance is fetched
///   separately) — keeping them double-counts ETH.
/// - non-positive balances, exactly as the Blockscout branch does.
pub fn parse_balances(d: &Value) -> Vec<Value> {
    let empty = vec![];
    let mut out = Vec::new();
    for it in d["result"].as_array().unwrap_or(&empty) {
        if it["native_token"].as_bool().unwrap_or(false) {
            continue;
        }
        let dec = it["decimals"].as_f64().unwrap_or(18.0);
        let raw = it["balance"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let bal = raw / 10f64.powf(dec);
        if bal <= 0.0 {
            continue;
        }
        let symbol = it["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?").to_uppercase();
        let contract = it["token_address"].as_str().unwrap_or("").to_lowercase();
        let usd = it["usd_value"].as_f64().unwrap_or(0.0);
        out.push(json!({
            "symbol": symbol,
            "contract": contract,
            "balance": js_num(bal),
            "usd": js_num(if usd > 0.0 { usd } else { 0.0 }),
        }));
    }
    out
}

/// Чому Moralis не дав балансів.
///
/// Різниця між «Moralis не знає цього ланцюга» і «акаунт Moralis заблоковано» — це різниця
/// між «цього гаманця тут не існує» і «ми не можемо запитати зараз». Раніше обидва випадки
/// зливалися в `None`, а лог виправдовував порожній список балансів словами «moralis не
/// покриває цей ланцюг» — тобто ми тихо брехали про причину: ланцюг Moralis покриває,
/// просто акаунт на паузі.
#[derive(Debug, Clone)]
pub enum BalancesError {
    /// Moralis не знає цього ланцюга (і не дізнається) — лише цей ланцюг не варто питати.
    UnsupportedChain,
    /// Акаунт/ключ Moralis відмовив: план зупинено або ключ недійсний (401/403).
    AccountBlocked(String),
    /// Мережа, 5xx або неочікувана відповідь — можливо, тимчасово.
    FetchFailed(String),
}

impl std::fmt::Display for BalancesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BalancesError::UnsupportedChain => write!(f, "Moralis не знає цього ланцюга"),
            BalancesError::AccountBlocked(m) => write!(f, "Moralis відмовив: {m}"),
            BalancesError::FetchFailed(m) => write!(f, "Moralis не відповів: {m}"),
        }
    }
}

/// Скільки пам'ятаємо про заблокований акаунт Moralis.
///
/// Без цього кожен гаманець у черзі б'є в Moralis (і через сателіт — теж), а відповідь
/// одна й та сама: план зупинено. На реальному профілі це тисячі однакових 401 за хвилину,
/// що глушить і справжні помилки в журналі.
const BLOCKED_TTL: Duration = Duration::from_secs(1800);

fn blocked_memo() -> &'static Mutex<Option<(Instant, String)>> {
    static M: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// Moralis відмовив — запам'ятати на [`BLOCKED_TTL`], щоб не питати знову щохвилини.
fn remember_blocked(msg: &str) {
    let mut m = blocked_memo().lock().unwrap();
    *m = Some((Instant::now(), msg.to_string()));
}

/// Акаунт Moralis заблоковано нещодавно? Тоді не йдемо в мережу зовсім.
fn blocked_reason() -> Option<String> {
    let m = blocked_memo().lock().unwrap();
    m.as_ref()
        .filter(|(at, _)| at.elapsed() < BLOCKED_TTL)
        .map(|(_, msg)| msg.clone())
}

/// Баланси ERC-20 з Moralis.
///
/// `Err` — не «нуль», а «невідомо»: усі три варіанти означають, що викликач НЕ має права
/// читати результат як «гаманець нічого не має».
pub async fn token_balances(chain: &str, address: &str) -> Result<Vec<Value>, BalancesError> {
    let param = chain_param(chain).ok_or(BalancesError::UnsupportedChain)?;

    // Акаунт уже відмовив — не довбимо те саме питання в стіну.
    if let Some(msg) = blocked_reason() {
        return Err(BalancesError::AccountBlocked(msg));
    }

    // Суворий режим: ключ живе на сателіті, кор його не має й мати не повинен.
    let (status, body) = if crate::satellite::strict() {
        crate::satellite::fetch(
            "moralis",
            &format!("/api/v2.2/wallets/{address}/tokens"),
            &[("chain", param.to_string())],
        )
        .await
        .map_err(|e| BalancesError::FetchFailed(e.to_string()))?
    } else {
        let key = api_key().ok_or_else(|| BalancesError::FetchFailed("немає ключа Moralis".into()))?;
        let url = format!("https://deep-index.moralis.io/api/v2.2/wallets/{address}/tokens?chain={param}");
        let resp = http()
            .get(&url)
            .header("X-API-Key", key)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| BalancesError::FetchFailed(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| BalancesError::FetchFailed(e.to_string()))?
            .to_vec();
        (status, body)
    };

    let d: Value = serde_json::from_slice(&body)
        .map_err(|e| BalancesError::FetchFailed(format!("не-JSON відповідь: {e}")))?;

    if !(200..300).contains(&status) {
        // Тіло Moralis каже ЧОМУ: «Your Moralis Free usage is paused…» або «Invalid API Key».
        // Саме це треба показати в логу, а не «ланцюг не покритий».
        let msg = d["message"].as_str().unwrap_or("").trim().to_string();
        let looks_blocked = status == 401
            || status == 403
            || msg.contains("paused")
            || msg.contains("Invalid API Key")
            || msg.contains("upgrade")
            || msg.contains("suspended");
        if looks_blocked {
            let why = if msg.is_empty() { format!("HTTP {status}") } else { msg };
            remember_blocked(&why);
            return Err(BalancesError::AccountBlocked(why));
        }
        return Err(BalancesError::FetchFailed(format!("HTTP {status}: {msg}")));
    }

    // A body carrying `message` instead of `result` is Moralis refusing (unknown chain,
    // too many balances, bad key) — a failure, not an empty wallet.
    if !d["result"].is_array() {
        let msg = d["message"].as_str().unwrap_or("").to_string();
        if msg.contains("chain must be") {
            return Err(BalancesError::UnsupportedChain);
        }
        return Err(BalancesError::FetchFailed(msg));
    }
    Ok(parse_balances(&d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_entry_is_dropped_and_amounts_scale() {
        let d = json!({"result": [
            // Moralis lists the native coin here; Blockscout's /tokens does not, and the
            // native balance is fetched separately — keeping this would double-count ETH.
            {"token_address": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "symbol": "ETH",
             "decimals": 18, "balance": "1000000000000000000", "usd_value": 3000.0, "native_token": true},
            {"token_address": "0xA0B86991C6218B36C1D19D4A2E9EB0CE3606EB48", "symbol": "usdc",
             "decimals": 6, "balance": "2500000", "usd_value": 2.5, "native_token": false},
            // Non-positive balances are dropped, matching the Blockscout branch.
            {"token_address": "0xdead", "symbol": "ZERO", "decimals": 18, "balance": "0", "native_token": false},
        ]});
        let out = parse_balances(&d);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["symbol"], "USDC");
        assert_eq!(out[0]["contract"], "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
        assert_eq!(out[0]["balance"], 2.5);
        assert_eq!(out[0]["usd"], 2.5);
    }

    #[test]
    fn chains_moralis_rejects_are_absent() {
        // Verified live: Moralis answers "chain must be a valid enum value" for these.
        for c in ["blast", "mantle", "sonic"] {
            assert!(chain_param(c).is_none(), "{c} must not be claimed — Moralis rejects it");
        }
        assert_eq!(chain_param("linea"), Some("0xe708"));
    }
}
