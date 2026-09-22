//! Кеш ончейн-балансів усіх відстежуваних гаманців.
//!
//! НАВІЩО. Баланс одного гаманця — це живий обхід 15 мереж, 7-47 секунд. Фронт колись намагався
//! зібрати всі 507 сам, залпом із браузера: 6 з'єднань × хвилини, черга на години, і «Сумарна
//! вартість» показувала синтетику замість підсумку. Порахувати підсумок по 507 гаманцях може лише
//! той, хто має час і кеш, — тобто сервер. Воркер обходить гаманці повільно й по черзі, а фронт
//! читає готове.
//!
//! Тут — ЧИСТА частина: злиття свіжих результатів із попереднім файлом. Мережа й цикл — у main.rs
//! (balances_tick), щоб це можна було перевірити тестом без жодного запиту.

use serde_json::{json, Value};

/// Результат обходу одного гаманця. `total_usd == None` — ЗБІЙ (джерело не відповіло), а не нуль.
/// Різниця між «нуль на балансі» і «не змогли дізнатись» тут принципова: перше можна показати,
/// друге — ні.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub address: String,
    pub total_usd: Option<f64>,
    pub tokens: usize,
    pub chains: i64,
    pub source: String,
}

/// Зливає свіжі результати з попереднім станом кешу.
///
/// ДВА ПРАВИЛА, обидва оплачені багами:
///  1. Збій НЕ затирає відомий баланс — гаманець лишається з попереднім записом (і своїм старим
///     `updatedAt`, щоб було видно, що дані несвіжі). Записати нуль на місце знаного балансу —
///     та сама брехня, що й порожні кластери поверх непорожніх (d3b43e0).
///  2. Гаманець, якого вже немає в списку відстеження, зникає й з кешу — інакше файл ріс би вічно,
///     а «Сумарна вартість» рахувала б гаманці, які ніхто не дивиться.
pub fn merge(prev: &Value, fetched: &[Fetched], wallets: &[String], now: i64) -> Value {
    let prev_wallets = prev.get("wallets").and_then(Value::as_object);
    let mut out = serde_json::Map::new();

    for addr in wallets {
        let key = addr.to_lowercase();
        let fresh = fetched.iter().find(|f| f.address.to_lowercase() == key);

        let entry = match fresh {
            // Успіх — новий запис зі свіжою міткою часу.
            Some(f) if f.total_usd.is_some() => json!({
                "totalUsd": f.total_usd,
                "tokens": f.tokens,
                "chains": f.chains,
                "source": f.source,
                "updatedAt": now,
            }),
            // Збій або гаманець у цьому циклі не обходили — тримаємось за те, що знали.
            _ => match prev_wallets.and_then(|w| w.get(&key)).cloned() {
                Some(old) => old,
                None => continue, // ніколи не знали й зараз не дізнались — запису просто немає
            },
        };
        out.insert(key, entry);
    }

    let total: f64 = out.values().filter_map(|e| e["totalUsd"].as_f64()).sum();
    json!({
        "generated": now,
        "count": out.len(),          // скільки гаманців мають ВІДОМИЙ баланс
        "tracked": wallets.len(),    // скільки їх узагалі під наглядом — щоб покриття було видно
        "totalUsd": total,
        "wallets": Value::Object(out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(addr: &str, usd: f64) -> Fetched {
        Fetched { address: addr.into(), total_usd: Some(usd), tokens: 3, chains: 5, source: "evm".into() }
    }
    fn fail(addr: &str) -> Fetched {
        Fetched { address: addr.into(), total_usd: None, tokens: 0, chains: 0, source: "evm".into() }
    }

    #[test]
    fn merges_fresh_results_and_sums_only_known() {
        let prev = json!({});
        let wallets = vec!["0xAAA".to_string(), "0xBBB".to_string()];
        let got = merge(&prev, &[ok("0xaaa", 100.0), ok("0xbbb", 50.5)], &wallets, 1000);
        assert_eq!(got["count"], 2);
        assert_eq!(got["tracked"], 2);
        assert_eq!(got["totalUsd"], 150.5);
        assert_eq!(got["wallets"]["0xaaa"]["totalUsd"], 100.0);
        assert_eq!(got["wallets"]["0xaaa"]["updatedAt"], 1000);
    }

    /// Головне правило: збій джерела НЕ має права перетворити знаний баланс на нуль.
    #[test]
    fn failure_keeps_previous_value_and_its_stale_timestamp() {
        let prev = merge(&json!({}), &[ok("0xaaa", 100.0)], &["0xaaa".to_string()], 1000);
        let after = merge(&prev, &[fail("0xaaa")], &["0xaaa".to_string()], 2000);
        assert_eq!(after["wallets"]["0xaaa"]["totalUsd"], 100.0, "збій затер відомий баланс");
        assert_eq!(after["wallets"]["0xaaa"]["updatedAt"], 1000, "мітка часу має лишитись СТАРОЮ — дані несвіжі");
        assert_eq!(after["count"], 1);
        assert_eq!(after["totalUsd"], 100.0);
    }

    /// Гаманець, який у цьому циклі не обходили (черга не дійшла), теж не втрачає баланс.
    #[test]
    fn wallet_not_visited_this_cycle_keeps_its_value() {
        let prev = merge(&json!({}), &[ok("0xaaa", 10.0), ok("0xbbb", 20.0)], &["0xaaa".into(), "0xbbb".into()], 1000);
        let after = merge(&prev, &[ok("0xaaa", 11.0)], &["0xaaa".into(), "0xbbb".into()], 2000);
        assert_eq!(after["wallets"]["0xaaa"]["totalUsd"], 11.0);
        assert_eq!(after["wallets"]["0xbbb"]["totalUsd"], 20.0);
        assert_eq!(after["totalUsd"], 31.0);
    }

    #[test]
    fn wallet_removed_from_tracking_disappears_from_cache() {
        let prev = merge(&json!({}), &[ok("0xaaa", 10.0), ok("0xbbb", 20.0)], &["0xaaa".into(), "0xbbb".into()], 1000);
        let after = merge(&prev, &[], &["0xaaa".into()], 2000);
        assert!(after["wallets"]["0xbbb"].is_null(), "видалений гаманець лишився в кеші");
        assert_eq!(after["count"], 1);
        assert_eq!(after["totalUsd"], 10.0);
    }

    #[test]
    fn unknown_wallet_that_failed_has_no_entry_at_all() {
        let got = merge(&json!({}), &[fail("0xaaa")], &["0xaaa".to_string()], 1000);
        assert_eq!(got["count"], 0, "невідомий баланс не має вигадувати запис");
        assert_eq!(got["tracked"], 1);
        assert!(got["wallets"]["0xaaa"].is_null());
    }

    #[test]
    fn nul_balance_is_a_real_value_not_a_failure() {
        // Нуль на балансі — це факт, і його можна показати. Саме тому Fetched несе Option, а не 0.0.
        let got = merge(&json!({}), &[ok("0xaaa", 0.0)], &["0xaaa".to_string()], 1000);
        assert_eq!(got["count"], 1);
        assert_eq!(got["wallets"]["0xaaa"]["totalUsd"], 0.0);
    }
}
