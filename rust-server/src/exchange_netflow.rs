// Pure kernel of `GET /api/exchange-netflow` (server/index.js:1065): given a
// token's global transfers and its current price, aggregate flow to/from
// exchanges into an accumulation/distribution verdict. Deterministic — the two
// live fetches (priceChart, tokenTransfersGlobal) happen in the route (main.rs);
// this half is golden-tested byte-for-byte against the Node route body.
//
// `is_cex_addr` ports index.js:1064 `isCexAddr` with its exact CEX_ADDRS
// (index.js:1032), the cex-class entries of KNOWN (index.js:326), and CEX_RE
// (index.js:1063).

use crate::jsmath::{js_num, js_round};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::OnceLock;

/// index.js:1032 `CEX_ADDRS` — hot-wallet addresses (already lowercase).

// The last three CEX_ADDRS rows in the JS Set (Crypto.com / Robinhood / Gemini)
// — kept separate only to stay within a fixed-size array above without a magic
// recount; folded into the same lookup set below.

/// index.js:326 `KNOWN` entries whose `cls === "cex"` (the burn 0x…dead is `cls
/// "burn"` and is deliberately excluded). Two of these (0x47ac…503, 0x2faf…ad2 /
/// FTX) are NOT in CEX_ADDRS, so `isCexAddr` needs them from here.


/// Три колишні масиви (CEX_ADDRS 35 + EXTRA 3 + KNOWN_CEX 9) — тепер три теги
/// у базі. Об'єднання лишилось тим самим, що й було: саме ці три, не більше.
fn in_cex_set(addr: &str) -> bool {
    let s = wr_store::snapshot();
    ["exchange_netflow:cex_addrs", "exchange_netflow:cex_addrs_extra", "exchange_netflow:known_cex"]
        .iter()
        .any(|t| s.wallet_has_tag("eth", addr, t))
}

/// index.js:1063 `CEX_RE` (case-insensitive).
fn cex_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)binance|coinbase|okx|kraken|bybit|huobi|kucoin|gate|bitfinex|gemini|bitstamp|mexc|bitget|crypto\.com|robinhood|deposit|exchange|hot ?wallet").unwrap()
    })
}

/// index.js:1064 `isCexAddr(addr, name)`.
pub fn is_cex_addr(addr: &str, name: &str) -> bool {
    let a = addr.to_lowercase();
    in_cex_set(&a) || cex_re().is_match(name)
}

/// One row of `bs.tokenTransfersGlobal` output.
#[derive(Deserialize, Default)]
pub struct Transfer {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default, rename = "fromName")]
    pub from_name: String,
    #[serde(default, rename = "toName")]
    pub to_name: String,
    #[serde(default)]
    pub amount: f64,
    #[serde(default)]
    pub ts: f64,
}

/// The aggregation kernel — verbatim port of the index.js:1065 route body after
/// the two fetches: classify each transfer's ends, sum directional flow, value
/// it at `px`, and derive the verdict.
pub fn exchange_netflow(contract: &str, px: f64, transfers: &[Transfer]) -> Value {
    let (mut in_amt, mut out_amt) = (0.0f64, 0.0f64);
    let (mut in_n, mut out_n) = (0i64, 0i64);
    let mut min_ts = f64::INFINITY;
    let mut max_ts = 0.0f64;
    for t in transfers {
        if t.ts != 0.0 {
            // if (t.ts) — truthy
            if t.ts < min_ts {
                min_ts = t.ts;
            }
            if t.ts > max_ts {
                max_ts = t.ts;
            }
        }
        let to_cex = is_cex_addr(&t.to, &t.to_name);
        let from_cex = is_cex_addr(&t.from, &t.from_name);
        if to_cex && !from_cex {
            in_amt += t.amount;
            in_n += 1;
        } else if from_cex && !to_cex {
            out_amt += t.amount;
            out_n += 1;
        }
    }
    let in_usd = js_round(in_amt * px);
    let out_usd = js_round(out_amt * px);
    let net_usd = out_usd - in_usd;
    let total = in_usd + out_usd;
    let verdict = if total < 1.0 || (in_n + out_n) < 3 {
        "neutral"
    } else if net_usd > 0.12 * total {
        "accumulation"
    } else if net_usd < -0.12 * total {
        "distribution"
    } else {
        "neutral"
    };
    json!({
        "contract": contract,
        "price": js_num(px),
        "inUsd": js_num(in_usd),
        "outUsd": js_num(out_usd),
        "netUsd": js_num(net_usd),
        "inCount": in_n,
        "outCount": out_n,
        "transfers": transfers.len(),
        "windowFrom": if min_ts.is_infinite() { Value::Null } else { js_num(min_ts) },
        "windowTo": if max_ts == 0.0 { Value::Null } else { js_num(max_ts) },
        "verdict": verdict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchange_netflow_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("exnetflow-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("exnetflow-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let transfers: Vec<Transfer> = serde_json::from_value(c["transfers"].clone()).unwrap();
            let got = exchange_netflow(c["contract"].as_str().unwrap(), c["px"].as_f64().unwrap(), &transfers);
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
