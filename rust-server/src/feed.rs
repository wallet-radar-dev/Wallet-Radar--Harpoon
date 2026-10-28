// Pure kernel of `POST /webhook/moralis` event mapping (server/feed.js
// moralisWebhookEvents): map a Moralis stream webhook body into our event objects
// for tracked wallets (ERC-20 + native), valued with the static price tables. The
// tracked-wallet list and the Date.now() ts fallback are injected (the store read
// + clock are the route's job). Golden-tested byte-for-byte.

use crate::jsmath::{js_num, js_round};
use serde_json::{json, Value};

// feed.js static tables.
fn chain_by_id(id: &str) -> Option<&'static str> {
    match id {
        "0x1" => Some("eth"),
        "0xa4b1" => Some("arbitrum"),
        "0x2105" => Some("base"),
        "0x89" => Some("polygon"),
        "0xa" => Some("optimism"),
        "0x38" => Some("bnb"),
        "0xa86a" => Some("avalanche"),
        _ => None,
    }
}
pub fn net_symbol(chain: &str) -> &'static str {
    match chain {
        "eth" | "arbitrum" | "base" | "optimism" => "ETH",
        "polygon" => "MATIC",
        "bnb" => "BNB",
        "solana" => "SOL",
        "bitcoin" => "BTC",
        "tron" => "TRX",
        "dogecoin" => "DOGE",
        "sonic" => "S",
        "mantle" => "MNT",
        "avalanche" => "AVAX",
        "flare" => "FLR",
        _ => "ETH",
    }
}
fn price(sym: &str) -> f64 {
    match sym {
        "ETH" => 1665.0,
        "BTC" => 62400.0,
        "SOL" => 69.0,
        "BNB" => 576.0,
        "MATIC" => 0.5,
        "TRX" => 0.33,
        "DOGE" => 0.08,
        "AVAX" => 6.4,
        "MNT" => 0.51,
        "FLR" => 0.007,
        "S" => 0.5,
        _ => 0.0,
    }
}
fn is_stable(sym: &str) -> bool {
    matches!(sym, "USDT" | "USDC" | "DAI" | "BUSD" | "FDUSD" | "TUSD" | "USD1")
}

// JS Number(v).
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
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}
// first truthy (non-empty) string among the keys, else "".
fn or_str<'a>(obj: &'a Value, keys: &[&str]) -> &'a str {
    for k in keys {
        if let Some(s) = obj[*k].as_str() {
            if !s.is_empty() {
                return s;
            }
        }
    }
    ""
}

/// Node має ДВА різні entityOf, і порт узяв не той. `entities.js` (→ entities.rs) віддає
/// Option/null — його користувачі перевіряють на null самі. Але стрічку подій обслуговує ЛОКАЛЬНИЙ
/// entityOf у feed.js:163, і він не вміє null: невідомий контрагент — це {name:"", type:"unknown"}.
/// Порт підключив сюди перший, тож None протік у JSON як `null`; фронт читає
/// `ev.counterpartyEntity.type` (render.ts) і падав на першій же події з невідомим контрагентом —
/// тобто майже завжди. Контракт відновлено: entity_json НІКОЛИ не повертає null.
///
/// Багатші імена з ENTITIES ("Binance" замість Node-івського "CEX") лишаємо — це свідоме
/// збагачення з порту entities (11ec0a7). KNOWN_CEX тут як запобіжник: одна з шести адрес
/// (0xa52d…f771d9) у ENTITIES відсутня, і без цієї гілки вона втратила б бейдж біржі, який Node їй ставив.
fn entity_json(cp: &str) -> Value {
    if let Some(e) = crate::entities::entity_of(cp) {
        return json!({"name": e.name, "type": e.kind});
    }
    if in_known_cex(cp) {
        return json!({"name": "CEX", "type": "cex"});
    }
    json!({"name": "", "type": "unknown"})
}

/// feed.js:158 KNOWN_CEX — набір біржових адрес САМОЇ СТРІЧКИ, окремий від ENTITIES.
/// Переїхав у базу під тегом `feed:known_cex`. Окремість збережено: у стрічці
/// 6 адрес, і розширити їх до спільного переліку означало б змінити те,
/// які контрагенти підписуються як біржа.
fn in_known_cex(addr: &str) -> bool {
    wr_store::snapshot().wallet_has_tag("eth", addr, "feed:known_cex")
}


/// moralisWebhookEvents(body). `wallets` is store.getWallets(); `now` is the
/// Date.now() fallback used only when block.timestamp is missing.
pub fn moralis_webhook_events(body: &Value, wallets: &[Value], now: i64) -> Vec<Value> {
    let chain_id = body["chainId"].as_str().map(|s| s.to_lowercase()).unwrap_or_default();
    let Some(chain) = chain_by_id(&chain_id) else { return vec![] };

    // byAddr: valid-address wallets, lowercased → (id)
    let addr_re = regex::Regex::new(r"^0x[0-9a-fA-F]{40}$").unwrap();
    let mut by_addr: std::collections::HashMap<String, &Value> = std::collections::HashMap::new();
    for w in wallets {
        if let Some(a) = w["address"].as_str() {
            if addr_re.is_match(a) {
                by_addr.insert(a.to_lowercase(), w);
            }
        }
    }
    if by_addr.is_empty() {
        return vec![];
    }

    let ts_raw = num(&body["block"]["timestamp"]);
    let ts = if ts_raw != 0.0 && !ts_raw.is_nan() { ts_raw * 1000.0 } else { now as f64 };
    let block_num = {
        let b = num(&body["block"]["number"]);
        if b.is_nan() { 0.0 } else { b }
    };

    let mut out: Vec<Value> = Vec::new();
    let mut push = |id: &str, w: &Value, is_in: bool, cp: &str, asset: &str, amount: f64, value_usd: f64| {
        let tx_hash = id.split(':').next().unwrap_or("");
        out.push(json!({
            "id": id,
            "walletId": w["id"],
            "network": chain,
            "kind": if is_in { "in" } else { "out" },
            "asset": asset,
            "amount": js_num(amount),
            "valueUsd": js_num(value_usd),
            "counterparty": cp,
            "counterpartyEntity": entity_json(cp),
            "timestamp": js_num(ts),
            "txHash": tx_hash,
            "blockNumber": js_num(block_num),
            "feeUsd": 0,
            "status": "confirmed",
        }));
    };

    // ERC-20
    if let Some(trs) = body["erc20Transfers"].as_array() {
        for tr in trs {
            if truthy(&tr["possibleSpam"]) {
                continue;
            }
            let tx_hash = or_str(tr, &["transactionHash", "transaction_hash"]);
            if tx_hash.is_empty() {
                continue;
            }
            let from = or_str(tr, &["from", "fromAddress"]).to_lowercase();
            let to = or_str(tr, &["to", "toAddress"]).to_lowercase();
            // logIndex ?? log_index ?? 0
            let li = if !tr["logIndex"].is_null() {
                num(&tr["logIndex"])
            } else if !tr["log_index"].is_null() {
                num(&tr["log_index"])
            } else {
                0.0
            };
            let sym = {
                let s = or_str(tr, &["tokenSymbol", "token_symbol"]);
                if s.is_empty() { "?" } else { s }
            };
            // amount = Number(valueWithDecimals ?? value/10^(tokenDecimals||18)) || 0
            let amount = {
                let a = if !tr["valueWithDecimals"].is_null() {
                    num(&tr["valueWithDecimals"])
                } else {
                    let dec = if truthy(&tr["tokenDecimals"]) { num(&tr["tokenDecimals"]) } else { 18.0 };
                    num(&tr["value"]) / 10f64.powf(dec)
                };
                if a.is_nan() { 0.0 } else { a }
            };
            let rate = if is_stable(sym) { 1.0 } else { price(sym) };
            let value_usd = js_round(amount * rate);
            let id = format!("{tx_hash}:{chain}:{}", li as i64);
            if let Some(w) = by_addr.get(&from) {
                push(&id, w, false, &to, sym, amount, value_usd);
            }
            if let Some(w) = by_addr.get(&to) {
                push(&id, w, true, &from, sym, amount, value_usd);
            }
        }
    }

    // native
    let nsym = net_symbol(chain);
    if let Some(txs) = body["txs"].as_array() {
        for tx in txs {
            let tx_hash = tx["hash"].as_str().unwrap_or("");
            // Number(tx.value ?? "0")/1e18 — absent/null value ?? "0" = 0
            let amount = if tx["value"].is_null() { 0.0 } else { num(&tx["value"]) / 1e18 };
            if tx_hash.is_empty() || !(amount > 0.0) {
                continue;
            }
            let from = or_str(tx, &["fromAddress", "from_address"]).to_lowercase();
            let to = or_str(tx, &["toAddress", "to_address"]).to_lowercase();
            let value_usd = js_round(amount * price(nsym));
            let id = format!("{tx_hash}:{chain}:nat");
            if let Some(w) = by_addr.get(&from) {
                push(&id, w, false, &to, nsym, amount, value_usd);
            }
            if let Some(w) = by_addr.get(&to) {
                push(&id, w, true, &from, nsym, amount, value_usd);
            }
        }
    }

    out
}

// String(number) for the id — integer without a trailing ".0".
fn num_str(v: &Value) -> String {
    if let Some(i) = v.as_i64() {
        i.to_string()
    } else if let Some(f) = v.as_f64() {
        if f.fract() == 0.0 { (f as i64).to_string() } else { f.to_string() }
    } else {
        v.as_str().unwrap_or("").to_string()
    }
}

/// feed.js `toEvent(wallet, chain, tr)` — an ERC-20 transfer (from bs::token_transfers)
/// into an event object. Deterministic; golden-tested.
pub fn to_event(wallet: &Value, chain: &str, tr: &Value) -> Value {
    let w = wallet["address"].as_str().unwrap_or("").to_lowercase();
    let to = tr["to"].as_str().unwrap_or("");
    let is_in = to == w;
    let cp = if is_in { tr["from"].as_str().unwrap_or("") } else { to };
    json!({
        "id": format!("{}:{}:{}", tr["txHash"].as_str().unwrap_or(""), chain, num_str(&tr["logIndex"])),
        "walletId": wallet["id"],
        "network": chain,
        "kind": if is_in { "in" } else { "out" },
        "asset": tr["asset"],
        "amount": tr["amount"],
        "valueUsd": tr["valueUsd"],
        "counterparty": cp,
        "counterpartyEntity": entity_json(cp),
        "timestamp": tr["ts"],
        "txHash": tr["txHash"],
        "blockNumber": tr["block"],
        "feeUsd": 0,
        "status": "confirmed",
    })
}

/// feed.js pollBlockscoutOnce inline `mk(t)` — a native transfer into an event,
/// valued at `amount × px`. Deterministic; golden-tested.
pub fn native_event(wallet: &Value, chain: &str, tr: &Value, px: f64, sym: &str) -> Value {
    let w = wallet["address"].as_str().unwrap_or("").to_lowercase();
    let to = tr["to"].as_str().unwrap_or("");
    let is_in = to == w;
    let cp = if is_in { tr["from"].as_str().unwrap_or("") } else { to };
    let amount = tr["amount"].as_f64().unwrap_or(0.0);
    json!({
        "id": format!("{}:{}:nat", tr["txHash"].as_str().unwrap_or(""), chain),
        "walletId": wallet["id"],
        "network": chain,
        "kind": if is_in { "in" } else { "out" },
        "asset": sym,
        "amount": tr["amount"],
        "valueUsd": js_num(js_round(amount * px)),
        "counterparty": cp,
        "counterpartyEntity": entity_json(cp),
        "timestamp": tr["ts"],
        "txHash": tr["txHash"],
        "blockNumber": tr["block"],
        "feeUsd": 0,
        "status": "confirmed",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_event_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("toevent-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("toevent-golden.json")).unwrap();
        for (i, c) in inputs["token"].as_array().unwrap().iter().enumerate() {
            let got = to_event(&c["wallet"], c["chain"].as_str().unwrap(), &c["tr"]);
            assert_eq!(got, golden["token"][i], "token case {i}");
        }
        for (i, c) in inputs["native"].as_array().unwrap().iter().enumerate() {
            let got = native_event(&c["wallet"], c["chain"].as_str().unwrap(), &c["tr"], c["px"].as_f64().unwrap(), c["sym"].as_str().unwrap());
            assert_eq!(got, golden["native"][i], "native case {i}");
        }
    }

    #[test]
    fn moralis_webhook_events_matches_node() {
        let inputs: Value = serde_json::from_str(&fixture!("moralis-input.json")).unwrap();
        let golden: Value = serde_json::from_str(&fixture!("moralis-golden.json")).unwrap();
        for (i, c) in inputs.as_array().unwrap().iter().enumerate() {
            let empty = vec![];
            let wallets = c["wallets"].as_array().unwrap_or(&empty);
            let got = moralis_webhook_events(&c["body"], wallets, c["now"].as_i64().unwrap());
            assert_eq!(Value::Array(got), golden[i], "case {i}");
        }
    }

    /// Контракт feed.js:163: у стрічці counterpartyEntity — ЗАВЖДИ обʼєкт із полем `type`, ніколи
    /// null. Фронт читає `ev.counterpartyEntity.type` без перевірки, і на null падав рендер УСІЄЇ
    /// стрічки. Голий golden цього не ловив: його зняли з Rust-виходу, тож він null і благословив.
    /// Тому тут не знімок, а сам інваріант — його не можна «перегенерувати» в неправильний.
    #[test]
    fn counterparty_entity_is_never_null() {
        let cases = [
            ("0x1111111111111111111111111111111111111111", "unknown"), // ніде не відома
            ("0x0000000000000000000000000000000000000000", "unknown"), // нульова адреса
            ("0xf977814e90da44bfa03b6295a0616a897441acec", "cex"),     // є і в ENTITIES, і в KNOWN_CEX
            ("0xa52d5c2ce7128941a1632554bcd154c567f771d9", "cex"),     // ЛИШЕ в KNOWN_CEX (не в ENTITIES)
            ("0xF977814E90DA44BFA03B6295A0616A897441ACEC", "cex"),     // регістр не має значення
        ];
        for (cp, want) in cases {
            let v = entity_json(cp);
            assert!(v.is_object(), "entity_json({cp}) = {v}, а має бути обʼєкт (null ламає фронт)");
            assert!(v["name"].is_string(), "entity_json({cp}): поле name зникло");
            assert_eq!(v["type"], want, "entity_json({cp}): не той тип");
        }
    }

    /// Ті самі граблі з іншого боку: жодна подія у фікстурах не сміє мати null у counterpartyEntity.
    /// Якщо колись хтось перезніме goldens зі зламаного виходу — впаде тут, а не в браузері користувача.
    #[test]
    fn goldens_have_no_null_entity() {
        let toev: Value = serde_json::from_str(&fixture!("toevent-golden.json")).unwrap();
        let mor: Value = serde_json::from_str(&fixture!("moralis-golden.json")).unwrap();
        let mut checked = 0;
        let mut check = |ev: &Value, whose: &str| {
            assert!(
                ev["counterpartyEntity"].is_object(),
                "{whose}: counterpartyEntity = {} — фікстуру зняли зі зламаного виходу",
                ev["counterpartyEntity"]
            );
            checked += 1;
        };
        for key in ["token", "native"] {
            for ev in toev[key].as_array().unwrap() {
                check(ev, "toevent-golden");
            }
        }
        for case in mor.as_array().unwrap() {
            for ev in case.as_array().unwrap() {
                check(ev, "moralis-golden");
            }
        }
        assert!(checked > 0, "фікстури порожні — тест нічого не перевірив");
    }
}
