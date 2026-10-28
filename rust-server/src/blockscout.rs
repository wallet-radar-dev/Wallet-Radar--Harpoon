//! Port of the response-parsing layer of server/blockscout.js. The HTTP client,
//! request queue and pagination are network-bound and land with the record/replay
//! harness; these normalizers are pure functions of an API response and are
//! golden-tested against Node.

use crate::jsmath::js_num;
use chrono::DateTime;
use serde_json::{json, Value};

/// `Date.parse(s) || 0`. Blockscout timestamps are ISO8601 UTC; sub-millisecond
/// digits are truncated (not rounded), matching V8. Unparseable → 0.
pub fn date_parse_ms(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s).map(|d| d.timestamp_millis()).unwrap_or(0)
}

/// `Number(v || 0)` where the field is a Blockscout string. A missing/empty/zero
/// value yields `default`; a non-empty numeric string is parsed.
fn number_or(v: &Value, default: f64) -> f64 {
    match v {
        Value::String(s) if !s.is_empty() => s.parse().unwrap_or(0.0),
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => default,
    }
}

/// `it.token?.exchange_rate ? Number(it.token.exchange_rate) : 0` — 0 unless a
/// non-empty rate string is present.
fn rate_of(token: &Value) -> f64 {
    match &token["exchange_rate"] {
        Value::String(s) if !s.is_empty() => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn lower_str(v: &Value) -> String {
    v.as_str().unwrap_or("").to_lowercase()
}

/// `a?.x || a?.y || ""` over string fields.
fn or_str<'a>(v: &'a Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = v[*k].as_str() {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// Item mapping of `transfersRich` (blockscout.js:235-252). One response page in,
/// normalized transfers out.
pub fn parse_transfers_rich(d: &Value) -> Vec<Value> {
    let empty = vec![];
    d["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|it| {
            let dec = number_or(&it["total"]["decimals"], 18.0);
            let amount = number_or(&it["total"]["value"], 0.0) / 10f64.powf(dec);
            let rate = rate_of(&it["token"]);
            let token_addr = {
                let a = lower_str(&it["token"]["address"]);
                if a.is_empty() { lower_str(&it["token"]["address_hash"]) } else { a }
            };
            let asset = it["token"]["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or("?");
            // The tx hash is what separates a PURCHASE from an airdrop: in a swap the token
            // arrives and another (USDC/WETH) leaves in the SAME hash, while a free airdrop
            // has no counter-movement. Omitting it silently disabled classifyTokens' swap branch.
            let hash = {
                let h = lower_str(&it["transaction_hash"]);
                if h.is_empty() { lower_str(&it["tx_hash"]) } else { h }
            };
            json!({
                "from": lower_str(&it["from"]["hash"]),
                "to": lower_str(&it["to"]["hash"]),
                "fromName": or_str(&it["from"], &["name", "ens_domain_name"]),
                "toName": or_str(&it["to"], &["name", "ens_domain_name"]),
                "fromContract": it["from"]["is_contract"].as_bool().unwrap_or(false),
                "toContract": it["to"]["is_contract"].as_bool().unwrap_or(false),
                "asset": asset,
                "token": token_addr,
                "amount": js_num(amount),
                "usd": js_num(if rate != 0.0 { amount * rate } else { 0.0 }),
                "hash": hash,
                "ts": date_parse_ms(it["timestamp"].as_str().unwrap_or("")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_parse_matches_node() {
        assert_eq!(date_parse_ms("2023-01-15T12:34:56.000000Z"), 1673786096000);
        assert_eq!(date_parse_ms("2024-06-30T00:00:00Z"), 1719705600000);
        assert_eq!(date_parse_ms("2023-12-01T08:15:30.123456Z"), 1701418530123); // micros truncated
        assert_eq!(date_parse_ms("not-a-date"), 0);
        assert_eq!(date_parse_ms(""), 0);
    }

    #[test]
    fn transfers_rich_golden() {
        let input: Value = serde_json::from_str(&fixture!("bs-input.json")).unwrap();
        let want: Value = serde_json::from_str(&fixture!("bs-golden.json")).unwrap();
        let got = Value::Array(parse_transfers_rich(&input));
        assert_eq!(got, want);
    }
}
