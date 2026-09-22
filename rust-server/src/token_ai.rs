//! Port of server/token-ai.js — the "ask about a token" assistant. No LLM: it maps
//! the question onto one of six intents by keyword (uk/ru/en) and hands back the
//! matching slice of `computeAssetFlow`. The client renders the prose via i18n.

use serde_json::{json, Value};

/// token-ai.js INTENTS — first match wins, so the order is load-bearing.
const INTENTS: [(&str, &[&str]); 6] = [
    ("liquidity", &["ліквід", "ликвид", "liquid", "обсяг", "объём", "volume"]),
    ("pump", &["памп", "pump", "росте", "растёт", "вистрел", "выстрел", "signal", "сигнал"]),
    ("phase", &["накопич", "накопл", "accumul", "розпод", "распредел", "distribut", "фаз", "phase"]),
    ("entry", &["вхід", "вход", "entry", "прибут", "profit", "профит", "куп", "buy", "ціна", "цена", "price"]),
    ("leaders", &["хто", "кто", "who", "холдер", "holder", "топ", "top", "лідер", "leader", "кит", "whale"]),
    ("sentiment", &["настр", "sentiment", "соц", "social", "твіт", "твит", "reddit"]),
];

/// `q.toLowerCase()`, then the first intent with a substring hit; "summary" if none.
pub fn classify(q: &str) -> &'static str {
    let ql = q.to_lowercase();
    for (name, words) in INTENTS {
        if words.iter().any(|w| ql.contains(w)) {
            return name;
        }
    }
    "summary"
}

/// token-ai.js `answerToken` — reshape an already-computed asset-flow value.
/// `?? null` vs `|| null` is preserved: `currentPrice`/`marketCap` keep a 0, while
/// the object fields collapse a falsy value to null.
pub fn answer(d: &Value, q: &str) -> Value {
    let or_null = |v: &Value| if crate::bs::truthy(v) { v.clone() } else { Value::Null };
    json!({
        "symbol": d["token"]["symbol"].as_str().unwrap_or(""),
        "intent": classify(q),
        "currentPrice": nullish(&d["currentPrice"]),
        "marketCap": nullish(&d["marketCap"]),
        // `d.avgEntry ? d.avgEntry.price : null`
        "avgEntry": if crate::bs::truthy(&d["avgEntry"]) { d["avgEntry"]["price"].clone() } else { Value::Null },
        "whalePnl": or_null(&d["whalePnl"]),
        "liquidity": or_null(&d["liquidity"]),
        "pumpSignal": or_null(&d["pumpSignal"]),
        "leaderboard": d["leaderboard"].as_array().map_or_else(|| json!([]), |a| json!(a.iter().take(5).collect::<Vec<_>>())),
    })
}

/// `x ?? null` — only undefined/null collapse; 0 and "" survive.
fn nullish(v: &Value) -> Value {
    if v.is_null() {
        Value::Null
    } else {
        v.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_classification_matches_node() {
        assert_eq!(classify("яка ліквідність?"), "liquidity");
        assert_eq!(classify("Це ПАМП?"), "pump"); // case-insensitive
        assert_eq!(classify("фаза накопичення?"), "phase");
        assert_eq!(classify("яка ціна входу"), "entry");
        assert_eq!(classify("хто тримає"), "leaders");
        assert_eq!(classify("який настрій"), "sentiment");
        assert_eq!(classify(""), "summary");
        assert_eq!(classify("розкажи про токен"), "summary");
        // First match wins: "обсяг" (liquidity) precedes "сигнал" (pump).
        assert_eq!(classify("обсяг і сигнал"), "liquidity");
    }

    #[test]
    fn answer_shapes_and_nullifies_like_node() {
        let d = json!({
            "token": {"symbol": "UNI"},
            "currentPrice": 0,           // ?? null keeps a real 0
            "marketCap": null,
            "avgEntry": {"price": 4.2},
            "whalePnl": 0,               // || null collapses falsy → null
            "liquidity": {"tier": "deep"},
            "pumpSignal": null,
            "leaderboard": [1, 2, 3, 4, 5, 6, 7],
        });
        let a = answer(&d, "хто тримає");
        assert_eq!(a["symbol"], json!("UNI"));
        assert_eq!(a["intent"], json!("leaders"));
        assert_eq!(a["currentPrice"], json!(0));
        assert_eq!(a["marketCap"], Value::Null);
        assert_eq!(a["avgEntry"], json!(4.2));
        assert_eq!(a["whalePnl"], Value::Null);
        assert_eq!(a["liquidity"], json!({"tier": "deep"}));
        assert_eq!(a["leaderboard"].as_array().unwrap().len(), 5);

        // No avgEntry at all → null, not a missing key.
        let a2 = answer(&json!({"token": {}, "leaderboard": []}), "");
        assert_eq!(a2["avgEntry"], Value::Null);
        assert_eq!(a2["symbol"], json!(""));
        assert_eq!(a2["intent"], json!("summary"));
    }
}
