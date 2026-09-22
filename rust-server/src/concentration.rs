//! Port of the pure assembly of the `/api/concentration` route (server/index.js):
//! given token metadata, the top holders, and their attribution metas, builds the
//! ranked holder table with cumulative shares, per-category breakdown, reach
//! thresholds, and the derived scores. Concentration is real; entry/fearGreed/
//! behaviour are seeded heuristics (`seededServer`), deterministic per contract.
//!
//! The three Blockscout fetches (tokenInfo / tokenHolders / addressMeta) live in
//! `bs.rs`; this is the deterministic remainder, so the whole route is
//! byte-for-byte reproducible.

use crate::jsmath::{js_num, js_round, round_fixed, seeded_server};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

/// index.js KNOWN — мітки бірж і burn-адрес. Таблиця переїхала у SQLite (`wallets`).
///
/// Повертає (мітка, вид). Вид беремо з бази як є: колишній перелік знав лише
/// "cex" і "burn", база знає ширше — але жоден із кодів нижче не звіряється
/// з рядком дослівно, тож розширення безпечне.
fn known(addr: &str) -> Option<(String, String)> {
    let s = wr_store::snapshot();
    let a = addr.to_lowercase();
    let kind = s.wallet_kind("eth", &a)?;
    // Гаманці під наглядом (`eoa`) міткою біржі НЕ є — інакше вони випали б
    // із підрахунку концентрації, заради якого цей модуль існує.
    if matches!(kind, "eoa" | "unknown") {
        return None;
    }
    let name = s.wallet_label("eth", &a).unwrap_or(kind).to_string();
    Some((name, kind.to_string()))
}

struct Cats {
    cex: Regex,
    mm: Regex,
    fund: Regex,
    burn: Regex,
}

fn cats() -> &'static Cats {
    static C: OnceLock<Cats> = OnceLock::new();
    C.get_or_init(|| Cats {
        cex: Regex::new(r"binance|coinbase|okx|kraken|bybit|huobi|kucoin|gate|bitfinex|gemini|bitstamp|mexc|bitget|upbit|htx|crypto\.com|exchange").unwrap(),
        mm: Regex::new(r"wintermute|\bjump\b|gsr|\bdwf\b|cumberland|amber|b2c2|flow traders|market ?maker").unwrap(),
        fund: Regex::new(r"a16z|paradigm|polychain|pantera|\bfund\b|capital|ventures|treasury|foundation|\blabs\b|\bdao\b").unwrap(),
        burn: Regex::new(r"burn|null|dead").unwrap(),
    })
}

/// `^0x0{40}$` — the all-zero null address.
fn is_null_addr(a: &str) -> bool {
    a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b == b'0')
}

/// index.js `holderCategory` — cex / mm / fund / burn / contract / whale / retail.
fn holder_category(name: &str, is_contract: bool, share: f64, addr: &str) -> &'static str {
    let a = addr.to_lowercase();
    if let Some((_, cls)) = known(&a) {
        // Вид приходить із бази як String. Зіставляємо на &'static str, щоб не
        // тягнути час життя назовні; невідомий вид НЕ ковтаємо мовчки —
        // хай далі класифікує за назвою, як і для адрес без мітки.
        match cls.as_str() {
            "cex" => return "cex",
            "burn" => return "burn",
            "bridge" => return "bridge",
            "defi" | "issuer" | "pool" => return "contract",
            _ => {}
        }
    }
    let s = name.to_lowercase();
    let c = cats();
    if c.cex.is_match(&s) {
        "cex"
    } else if c.mm.is_match(&s) {
        "mm"
    } else if c.fund.is_match(&s) {
        "fund"
    } else if c.burn.is_match(&s) || is_null_addr(&a) {
        "burn"
    } else if is_contract {
        "contract"
    } else if share >= 1.0 {
        "whale"
    } else {
        "retail"
    }
}

/// Builds the `/api/concentration` response. `contract` is the trimmed original
/// (case preserved for `token.contract`; lowercased for the score seed).
/// `top_holders` is `tokenHolders(...).holders`; `metas[i]` is holder `i`'s
/// addressMeta (or `Null`), for the top 30 only.
pub fn build(contract: &str, info: &Value, top_holders: &[Value], metas: &[Value]) -> Value {
    let meta_n = top_holders.len().min(30);
    let mut cum = 0.0;
    let mut holders: Vec<Value> = Vec::with_capacity(top_holders.len());
    let mut shares: Vec<f64> = Vec::with_capacity(top_holders.len());
    let mut cums: Vec<f64> = Vec::with_capacity(top_holders.len());

    for (i, h) in top_holders.iter().enumerate() {
        let addr = h["address"].as_str().unwrap_or("").to_lowercase();
        let share = round_fixed(num_or0(&h["share"]), 2);
        cum += share;
        let cum_r = round_fixed(cum, 2);

        let m = if i < meta_n { metas.get(i).filter(|v| !v.is_null()) } else { None };
        // name = KNOWN[addr]?.name || m?.name || ""
        let known_name = known(&addr).map(|k| k.0).filter(|s| !s.is_empty());
        let meta_name =
            m.and_then(|mm| mm["name"].as_str()).filter(|s| !s.is_empty()).map(str::to_string);
        let name = known_name.or(meta_name).unwrap_or_default();
        let is_contract = m.map(|mm| mm["isContract"].as_bool().unwrap_or(false)).unwrap_or(false);
        let cat = holder_category(&name, is_contract, share, &addr);

        holders.push(json!({
            "rank": i + 1,
            "address": addr,
            "share": js_num(share),
            "cum": js_num(cum_r),
            "cls": cat,
            "label": name,
            "category": cat,
        }));
        shares.push(share);
        cums.push(cum_r);
    }

    // breakdown by category, in first-seen order.
    let mut breakdown: Map<String, Value> = Map::new();
    for h in &holders {
        let cat = h["category"].as_str().unwrap_or("");
        let share = h["share"].as_f64().unwrap_or(0.0);
        match breakdown.get_mut(cat) {
            Some(b) => {
                let o = b.as_object_mut().unwrap();
                o["count"] = json!(o["count"].as_u64().unwrap() + 1);
                o["sharePct"] = json!(o["sharePct"].as_f64().unwrap() + share);
            }
            None => {
                breakdown.insert(cat.to_string(), json!({"count": 1, "sharePct": share}));
            }
        }
    }
    for (_, b) in breakdown.iter_mut() {
        let o = b.as_object_mut().unwrap();
        o["sharePct"] = js_num(round_fixed(o["sharePct"].as_f64().unwrap(), 2));
    }

    let reach = |p: f64| -> Value {
        match cums.iter().position(|&c| c >= p) {
            Some(i) => json!(i + 1),
            None => Value::Null,
        }
    };
    let top10 = round_fixed(shares.iter().take(10).sum::<f64>(), 1);

    let seed = contract.to_lowercase();
    let mut rnd = seeded_server(&seed);
    let entry = js_round((100.0 - top10) * 0.6 + rnd() * 40.0).clamp(1.0, 99.0);
    let fear_greed = js_round(rnd() * 100.0);
    let behaviour = js_round(rnd() * 100.0);

    json!({
        "token": {
            "name": info["name"], "symbol": info["symbol"],
            "holdersCount": info["holdersCount"], "contract": contract,
        },
        "holders": holders,
        "breakdown": Value::Object(breakdown),
        "reach": { "p40": reach(40.0), "p60": reach(60.0), "p70": reach(70.0) },
        "topCumNote": js_num(cums.last().copied().unwrap_or(0.0)),
        "scores": {
            "top10": js_num(top10),
            "entry": js_num(entry),
            "fearGreed": js_num(fear_greed),
            "behaviour": js_num(behaviour),
        },
    })
}

/// `+(Number(v) || 0).toFixed(...)`'s inner `Number(v) || 0`.
fn num_or0(v: &Value) -> f64 {
    let n = match v {
        Value::Number(x) => x.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => s.trim().parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    };
    if n == 0.0 || n.is_nan() { 0.0 } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concentration_build_matches_node() {
        let input: Value = serde_json::from_str(include_str!("../tests/fixtures/conc-input.json")).unwrap();
        let golden: Value = serde_json::from_str(include_str!("../tests/fixtures/conc-golden.json")).unwrap();
        for (i, c) in input.as_array().unwrap().iter().enumerate() {
            let got = build(
                c["contract"].as_str().unwrap(),
                &c["info"],
                c["holders"].as_array().unwrap(),
                c["metas"].as_array().unwrap(),
            );
            assert_eq!(got, golden[i], "case {i}");
        }
    }
}
