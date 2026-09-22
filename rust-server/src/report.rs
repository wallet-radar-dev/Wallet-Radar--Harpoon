//! Port of server/report.js `walletReport` — the analytical wallet report: it
//! synthesizes balances, exchange netflows, the smart-money score and the ML
//! prediction into an explainable rule-based forecast, a take-profit ladder, and
//! a Ukrainian-language summary.
//!
//! The fetch/timeout orchestration stays in main.rs; this is the pure synthesis,
//! golden-tested byte-for-byte against Node with `Date.now()` supplied as `now`.
//! Every quirk is preserved: the `f()` money formatter's toFixed tiers, the `sc`
//! scoring weights and `*1.3 / *0.7` score multipliers, the `≥3× → take` ladder,
//! `Math.min(92, …)` confidence, and the exact summary strings.

use crate::jsmath::{js_num, js_round, round_fixed, to_fixed};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// report.js STABLE — its own stablecoin set (note: has RLUSD, not USD1).
const STABLE: [&str; 16] = [
    "USDT", "USDC", "DAI", "BUSD", "FDUSD", "TUSD", "USDE", "PYUSD", "USDD", "GUSD", "USDP", "LUSD", "USDS", "FRAX",
    "CRVUSD", "RLUSD",
];

fn is_stable(sym: &str) -> bool {
    STABLE.contains(&sym)
}

/// report.js `f` — compact USD formatter with tiered toFixed precision.
fn f(n: f64) -> String {
    if n >= 1e9 {
        format!("${}B", to_fixed(n / 1e9, 2))
    } else if n >= 1e6 {
        format!("${}M", to_fixed(n / 1e6, 1))
    } else if n >= 1e3 {
        format!("${}k", to_fixed(n / 1e3, 0))
    } else {
        format!("${}", js_round(n) as i64)
    }
}

/// JS `String(number)` for the values interpolated here (integers and ≤2-dp
/// ratios): whole numbers print without a decimal point.
fn numstr(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

fn obj(fields: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in fields {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// report.js `walletReport` synthesis. `score`/`cond` may be `Null`; `bal`/`nf`/`ml`
/// carry their JS post-timeout defaults (`{totalUsd,tokens}` / `{netflows}` /
/// `{available:false}`). `now` is `Date.now()`.
pub fn build_report(address: &str, bal: &Value, nf: &Value, score: &Value, ml: &Value, cond: &Value, now: i64) -> Value {
    let empty = vec![];
    let tokens = bal["tokens"].as_array().unwrap_or(&empty);
    let netflows = nf["netflows"].as_array().unwrap_or(&empty);

    // ── take-profit ladder ──
    // curBySym: symbol → current USD for non-stable holdings (last write wins).
    let mut cur_by_sym: HashMap<String, f64> = HashMap::new();
    for tk in tokens {
        if let Some(sym) = tk["symbol"].as_str() {
            if !sym.is_empty() && !is_stable(sym) {
                cur_by_sym.insert(sym.to_string(), tk["valueUsd"].as_f64().unwrap_or(0.0));
            }
        }
    }
    let open_positions = if score.is_object() { score["openPositions"].as_array().cloned().unwrap_or_default() } else { vec![] };
    let mut take_profit: Vec<Value> = Vec::new();
    for op in &open_positions {
        let sym = op["sym"].as_str().unwrap_or("");
        let cost = op["costUsd"].as_f64().unwrap_or(0.0);
        let current = cur_by_sym.get(sym).copied().unwrap_or(0.0);
        // `!currentUsd || op.costUsd < 100` — missing/zero current, or tiny cost.
        if current == 0.0 || cost < 100.0 {
            continue;
        }
        let multiple = round_fixed(current / cost, 2);
        let (tier, sell_pct) = if multiple >= 3.0 {
            ("take", js_round((cost / current) * 100.0).min(90.0))
        } else if multiple >= 2.0 {
            ("approach", 0.0)
        } else {
            ("hold", 0.0)
        };
        take_profit.push(obj(vec![
            ("sym", Value::String(sym.to_string())),
            ("costUsd", op["costUsd"].clone()),
            ("currentUsd", js_num(js_round(current))),
            ("multiple", js_num(multiple)),
            ("tier", Value::String(tier.to_string())),
            ("sellPct", js_num(sell_pct)),
        ]));
    }
    take_profit.sort_by(|a, b| {
        b["multiple"].as_f64().unwrap_or(0.0).partial_cmp(&a["multiple"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let tp_ready: Vec<&Value> = take_profit.iter().filter(|x| x["tier"] == "take").collect();

    // ── explainable forecast ──
    let mut reasons: Vec<Value> = Vec::new();
    let mut sc = 0.0f64;
    for x in netflows {
        let asset = x["asset"].as_str().unwrap_or("");
        let net = x["net"].as_f64().unwrap_or(0.0);
        let amt = f(net.abs());
        if is_stable(asset) {
            if net > 0.0 {
                sc -= 8.0;
                reasons.push(obj(vec![("sign", json!("neutral")), ("text", json!(format!("накопичує стейбли {asset} {amt} — де-ризик/готовність купувати")))]));
            } else {
                sc += 6.0;
                reasons.push(obj(vec![("sign", json!("bull")), ("text", json!(format!("витрачає стейбли {asset} {amt} — можливі покупки")))]));
            }
        } else if net > 0.0 {
            sc += 16.0;
            reasons.push(obj(vec![("sign", json!("bull")), ("text", json!(format!("накопичує {asset} {amt} (виводить з біржі)")))]));
        } else {
            sc -= 16.0;
            reasons.push(obj(vec![("sign", json!("bear")), ("text", json!(format!("скидає {asset} {amt} на біржу")))]));
        }
    }

    // smScore = score?.score ?? null
    let sm_score: Option<f64> = if score.is_object() { score["score"].as_f64() } else { None };
    if let Some(s) = sm_score {
        if s >= 65.0 {
            sc *= 1.3;
            reasons.push(obj(vec![("sign", json!("info")), ("text", json!(format!("високий smart-money скоринг {} → сигналу можна довіряти більше", numstr(s))))]));
        } else if s <= 40.0 {
            sc *= 0.7;
            reasons.push(obj(vec![("sign", json!("info")), ("text", json!(format!("низький скоринг {} → сигнал слабший", numstr(s))))]));
        }
    }

    // ml?.available && typeof ml.prob === "number"
    let ml_available = ml["available"].as_bool().unwrap_or(false);
    let ml_prob: Option<f64> = if ml_available && ml["prob"].is_number() { ml["prob"].as_f64() } else { None };
    if let Some(p) = ml_prob {
        if p >= 60.0 {
            reasons.push(obj(vec![("sign", json!("bull")), ("text", json!(format!("ML: ймовірно smart money ({}%)", numstr(p))))]));
        } else if p <= 35.0 {
            reasons.push(obj(vec![("sign", json!("bear")), ("text", json!(format!("ML: радше не smart money ({}%)", numstr(p))))]));
        }
    }

    let sc = js_round(sc);
    let direction = if sc >= 20.0 { "bullish" } else if sc <= -20.0 { "bearish" } else { "neutral" };
    let sm_ge65 = sm_score.map(|s| s >= 65.0).unwrap_or(false);
    let confidence = (40.0 + sc.abs() + if sm_ge65 { 10.0 } else { 0.0 }).min(92.0);

    // ── summary ──
    let verdict = match direction {
        "bullish" => "📈 Накопичення / сила",
        "bearish" => "📉 Роздача / вихід",
        _ => "➖ Нейтрально",
    };
    let top: Vec<String> = tokens.iter().take(5).map(|t| format!("{} {}", t["symbol"].as_str().unwrap_or(""), f(t["valueUsd"].as_f64().unwrap_or(0.0)))).collect();
    let mut summary: Vec<Value> = Vec::new();
    summary.push(json!(format!(
        "Обсяг гаманця: {} у {} активах{}.",
        f(bal["totalUsd"].as_f64().unwrap_or(0.0)),
        tokens.len(),
        if top.is_empty() { String::new() } else { format!(" (топ: {})", top.join(", ")) }
    )));
    if cond.is_object() {
        summary.push(json!(format!(
            "🎼 Це ДИРИГЕНТ — координує {} активів, фінансує {} холдерів.",
            numstr(cond["assetCount"].as_f64().unwrap_or(0.0)),
            numstr(cond["holderCount"].as_f64().unwrap_or(0.0))
        )));
    }
    if let Some(s) = sm_score {
        let roi = if score.is_object() && !score["roi"].is_null() { format!(", ROI {}%", numstr(score["roi"].as_f64().unwrap_or(0.0))) } else { String::new() };
        let wr = if score.is_object() && !score["winRate"].is_null() { format!(", win-rate {}%", numstr(score["winRate"].as_f64().unwrap_or(0.0))) } else { String::new() };
        summary.push(json!(format!("Smart-money скоринг: {}/100{}{}.", numstr(s), roi, wr)));
    } else {
        summary.push(json!("Скоринг недоступний (мало історії)."));
    }
    if !netflows.is_empty() {
        let flows: Vec<String> = netflows.iter().take(3).map(|x| {
            let net = x["net"].as_f64().unwrap_or(0.0);
            format!("{} {}{}", x["asset"].as_str().unwrap_or(""), if net >= 0.0 { "+" } else { "\u{2212}" }, f(net.abs()))
        }).collect();
        summary.push(json!(format!("Чистий потік на біржах: {}.", flows.join(", "))));
    } else {
        summary.push(json!("Прямих рухів на біржах не виявлено (можливо, діє через суб-гаманці)."));
    }
    if !tp_ready.is_empty() {
        let parts: Vec<String> = tp_ready.iter().map(|x| format!("{} {}x — продати ~{}%", x["sym"].as_str().unwrap_or(""), numstr(x["multiple"].as_f64().unwrap_or(0.0)), numstr(x["sellPct"].as_f64().unwrap_or(0.0)))).collect();
        summary.push(json!(format!("💰 Фіксація тіла: {} (правило +200%).", parts.join(", "))));
    }
    summary.push(json!(format!("Прогноз: {} (впевненість {}%).", verdict, numstr(confidence))));

    // ── result ──
    let holdings: Vec<Value> = tokens.iter().take(12).cloned().collect();
    let conductor = if cond.is_object() {
        obj(vec![("assetCount", cond["assetCount"].clone()), ("holderCount", cond["holderCount"].clone()), ("assets", cond["assets"].clone())])
    } else {
        Value::Null
    };
    let get = |k: &str| if score.is_object() { score[k].clone() } else { Value::Null };

    obj(vec![
        ("address", Value::String(address.to_string())),
        ("generated", js_num(now as f64)),
        ("volume", bal["totalUsd"].clone()),
        ("assetCount", js_num(tokens.len() as f64)),
        ("holdings", Value::Array(holdings)),
        ("netflows", Value::Array(netflows.clone())),
        ("score", sm_score.map(js_num).unwrap_or(Value::Null)),
        ("roi", nullish(get("roi"))),
        ("winRate", nullish(get("winRate"))),
        ("realizedUsd", nullish(get("realizedUsd"))),
        ("ml", if ml_available { ml["prob"].clone() } else { Value::Null }),
        ("isConductor", json!(cond.is_object())),
        ("conductor", conductor),
        ("takeProfit", Value::Array(take_profit)),
        ("forecast", obj(vec![
            ("direction", Value::String(direction.to_string())),
            ("score", js_num(sc)),
            ("confidence", js_num(confidence)),
            ("reasons", Value::Array(reasons)),
        ])),
        ("verdict", Value::String(verdict.to_string())),
        ("summary", Value::Array(summary)),
    ])
}

/// `x ?? null` — undefined/null → Null, else the value.
fn nullish(v: Value) -> Value {
    if v.is_null() {
        Value::Null
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_report_matches_node() {
        let inputs: Value = serde_json::from_str(include_str!("../tests/fixtures/report-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/report-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let got = build_report(
                c["address"].as_str().unwrap(),
                &c["bal"],
                &c["nf"],
                &c["score"],
                &c["ml"],
                &c["cond"],
                c["now"].as_i64().unwrap(),
            );
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
