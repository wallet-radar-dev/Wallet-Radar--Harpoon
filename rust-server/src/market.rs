//! Port of server/market.js — the macro layer: Fear & Greed, global market cap and
//! BTC/ETH dominance, DeFi TVL. All keyless. Feeds the risk-on/risk-off regime and
//! the BTC→ETH rotation call that the institutional strategy leans on.
//!
//! The rotation needs HISTORY: it records dominance every call and reads the 3-day
//! delta back out, so the answer only warms up once the series has points in it
//! (`haveHistory`). That is why `history.rs` had to exist first.

use crate::jsmath::{js_num, round_fixed};
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// market.js `jget` — any failure (network, non-2xx, bad JSON) is just `null`.
async fn jget(url: &str) -> Value {
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("wallet-radar/rust")
        .build();
    let Ok(c) = c else { return Value::Null };
    match c.get(url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// `a ?? b` — take `a` unless it is null/absent.
fn coalesce(a: &Value, b: &Value) -> Value {
    if a.is_null() { b.clone() } else { a.clone() }
}

/// market.js `computeRotation` — dominance trend over ~3 days plus on-chain
/// confirmation from the coordinated whale moves.
fn compute_rotation(btc_dom: &Value, eth_dom: &Value, moves: &Value, now: i64) -> Value {
    let (bd, ed) = (btc_dom.as_f64(), eth_dom.as_f64());
    // `(btcDom && ethDom) ? +(ethDom / btcDom).toFixed(4) : null` — a zero is falsy
    // in JS, so it lands in the null branch too.
    let eth_btc_ratio = match (bd, ed) {
        (Some(b), Some(e)) if b != 0.0 && e != 0.0 => json!(js_num(round_fixed(e / b, 4))),
        _ => Value::Null,
    };
    if bd.is_some() && ed.is_some() {
        crate::history::record("market", &json!({"btcDom": btc_dom, "ethDom": eth_dom}), now);
        crate::history::persist();
    }

    let since = now - 3 * 24 * 3600 * 1000;
    let d_btc = crate::history::delta("market", "btcDom", since);
    let d_eth = crate::history::delta("market", "ethDom", since);
    let btc_change = d_btc.as_ref().map_or(Value::Null, |d| d["diff"].clone());
    let eth_change = d_eth.as_ref().map_or(Value::Null, |d| d["diff"].clone());

    // On-chain: accumulation vs distribution among the coordinated whale moves.
    let mut onchain = Value::Null;
    if let Some(list) = moves["moves"].as_array() {
        if !list.is_empty() {
            let count = |dir: &str| list.iter().filter(|m| m["direction"].as_str() == Some(dir)).count();
            let (acc, dist) = (count("accumulate"), count("distribute"));
            let bias = if acc > dist { "accumulate" } else if dist > acc { "distribute" } else { "mixed" };
            onchain = json!({"accumulate": acc, "distribute": dist, "bias": bias});
        }
    }

    let mut phase = "neutral";
    if let (Some(b), Some(e)) = (btc_change.as_f64(), eth_change.as_f64()) {
        if b <= -0.3 && e >= 0.2 {
            phase = "btc_to_eth";
        } else if b >= 0.3 && e <= -0.2 {
            phase = "eth_to_btc";
        }
    }
    json!({
        "ethBtcRatio": eth_btc_ratio,
        "btcDomChange": btc_change,
        "ethDomChange": eth_change,
        "phase": phase,
        "onchain": onchain,
        "haveHistory": !btc_change.is_null(),
    })
}

/// market.js `compute` — the four keyless sources, then the regime score.
async fn compute(moves: &Value, now: i64) -> Value {
    let (fng, cp, cg, chains) = tokio::join!(
        jget("https://api.alternative.me/fng/?limit=2"),
        jget("https://api.coinpaprika.com/v1/global"),
        jget("https://api.coingecko.com/api/v3/global"),
        jget("https://api.llama.fi/v2/chains"),
    );

    let f0 = &fng["data"][0];
    let f1 = &fng["data"][1];
    // `f0 ? Number(f0.value) : null` — the value arrives as a string.
    // `Number(f0.value)` — a JS number, so an integral one prints as 26, not 26.0.
    let num_of = |v: &Value| v["value"].as_str().and_then(|s| s.parse::<f64>().ok()).map(|n| json!(js_num(n))).unwrap_or(Value::Null);
    let fear_value = if f0.is_object() { num_of(f0) } else { Value::Null };
    let fear_prev = if f1.is_object() { num_of(f1) } else { Value::Null };
    let fear_class = f0["value_classification"].clone();

    let g = &cg["data"];
    let mcap = coalesce(&cp["market_cap_usd"], &g["total_market_cap"]["usd"]);
    let btc_dom = coalesce(&cp["bitcoin_dominance_percentage"], &g["market_cap_percentage"]["btc"]);
    let eth_dom = g["market_cap_percentage"]["eth"].clone();
    let change24 = coalesce(&cp["market_cap_change_24h"], &g["market_cap_change_percentage_24h_usd"]);
    let tvl = match chains.as_array() {
        Some(a) => json!(js_num(a.iter().map(|x| x["tvl"].as_f64().unwrap_or(0.0)).sum::<f64>())),
        None => Value::Null,
    };

    // Contrarian Fear & Greed: fear is an entry zone, greed calls for caution.
    let mut score = 0i32;
    let mut reasons: Vec<String> = Vec::new();
    if let Some(f) = fear_value.as_f64() {
        if f <= 25.0 {
            score += 1;
            reasons.push(format!("Екстремальний страх ({}) — історично зона накопичення", fmt_num(f)));
        } else if f >= 75.0 {
            score -= 1;
            reasons.push(format!("Екстремальна жадібність ({}) — обережність, фіксація", fmt_num(f)));
        } else {
            reasons.push(format!("Настрій нейтральний ({}/100)", fmt_num(f)));
        }
    }
    if let Some(c) = change24.as_f64() {
        if c <= -3.0 {
            score -= 1;
            reasons.push(format!("Ринок {}% за 24г", round_fixed(c, 1)));
        } else if c >= 3.0 {
            score += 1;
            reasons.push(format!("Ринок +{}% за 24г", round_fixed(c, 1)));
        }
    }
    if let (Some(f), Some(p)) = (fear_value.as_f64(), fear_prev.as_f64()) {
        if f > p + 4.0 {
            reasons.push("Настрій покращується".into());
        } else if f < p - 4.0 {
            reasons.push("Настрій погіршується".into());
        }
    }
    if let Some(b) = btc_dom.as_f64() {
        if b >= 55.0 {
            reasons.push(format!("Висока домінація BTC ({}%) — альти слабші, перевага ядру", round_fixed(b, 1)));
        }
    }

    let rotation = compute_rotation(&btc_dom, &eth_dom, moves, now);
    match rotation["phase"].as_str() {
        Some("btc_to_eth") => reasons.push(format!(
            "Ротація BTC→ETH: домінація BTC {}пп, ETH +{}пп за 3д — капітал тече в ETH/альти",
            rotation["btcDomChange"], rotation["ethDomChange"]
        )),
        Some("eth_to_btc") => reasons.push("Ротація ETH→BTC: капітал повертається в BTC (risk-off)".into()),
        _ => {}
    }
    match rotation["onchain"]["bias"].as_str() {
        Some("accumulate") => reasons.push(format!(
            "Ончейн: кити накопичують ({} vs {} розподіл)",
            rotation["onchain"]["accumulate"], rotation["onchain"]["distribute"]
        )),
        Some("distribute") => reasons.push(format!(
            "Ончейн: кити розподіляють ({} vs {} накопичення)",
            rotation["onchain"]["distribute"], rotation["onchain"]["accumulate"]
        )),
        _ => {}
    }

    let regime = if score >= 1 { "accumulate" } else if score <= -1 { "derisk" } else { "neutral" };
    let tilt = match regime {
        "accumulate" => "Макро сприяє розгортанню капіталу — можна збільшувати сателіт (альфу).",
        "derisk" => "Макро перегріте — зменшити сателіт, нарощувати стейбл-буфер.",
        _ => "Макро нейтральне — тримати цільові ваги профілю.",
    };

    json!({
        "fearValue": fear_value, "fearPrev": fear_prev, "fearClass": fear_class,
        "mcap": mcap, "btcDom": btc_dom, "ethDom": eth_dom, "change24": change24, "tvl": tvl,
        "regime": regime, "tilt": tilt, "rotation": rotation, "reasons": reasons, "ts": now,
    })
}

/// JS prints an integral f64 without a trailing ".0" — `${25}` is "25", not "25.0".
fn fmt_num(f: f64) -> String {
    json!(js_num(f)).to_string()
}

/// market.js `getMarketContext` — `cached("market:context", 30min, compute)`.
pub async fn get_market_context(moves: &Value, now: i64) -> Value {
    static CACHE: OnceLock<Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, v)) = &*cache.lock().unwrap() {
        if at.elapsed() < Duration::from_secs(30 * 60) {
            return v.clone();
        }
    }
    let v = compute(moves, now).await;
    *cache.lock().unwrap() = Some((Instant::now(), v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_reads_the_three_day_dominance_trend() {
        let _g = crate::history::reset_for_test();
        // A cold history: the first call records but has nothing to compare against.
        let now = 10_000_000_000;
        let r = compute_rotation(&json!(58.0), &json!(17.0), &json!({}), now);
        assert_eq!(r["haveHistory"], json!(false));
        assert_eq!(r["phase"], json!("neutral"));
        // (17/58).toFixed(4)
        assert_eq!(r["ethBtcRatio"], json!(0.2931));
        assert_eq!(r["onchain"], Value::Null);

        // A later point, 2 days on, with BTC dominance down and ETH up → rotation.
        // It has to land INSIDE the 3-day window: `delta` compares the newest point
        // against the oldest one still in the window, so a point 4 days back would
        // be skipped and the only candidate left would be the newest itself → null.
        let later = now + 2 * 24 * 3600 * 1000;
        let moves = json!({"moves": [{"direction": "accumulate"}, {"direction": "accumulate"}, {"direction": "distribute"}]});
        let r2 = compute_rotation(&json!(57.0), &json!(17.5), &moves, later);
        assert_eq!(r2["haveHistory"], json!(true));
        assert_eq!(r2["btcDomChange"], json!(-1)); // js_num: an integral f64 prints as -1, not -1.0
        assert_eq!(r2["ethDomChange"], json!(0.5));
        assert_eq!(r2["phase"], json!("btc_to_eth"));
        assert_eq!(r2["onchain"], json!({"accumulate": 2, "distribute": 1, "bias": "accumulate"}));
    }

    #[test]
    fn a_missing_dominance_pair_yields_no_ratio() {
        let _g = crate::history::reset_for_test();
        let r = compute_rotation(&Value::Null, &json!(17.0), &json!({"moves": []}), 1);
        assert_eq!(r["ethBtcRatio"], Value::Null);
        assert_eq!(r["onchain"], Value::Null); // empty moves list → stays null
    }
}
