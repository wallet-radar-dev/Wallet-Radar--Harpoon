//! Port of server/nonevm.js — non-EVM address typing and balances via free,
//! keyless explorers: Bitcoin (mempool.space), Tron (Tronscan), Solana (public
//! RPC). Shapes match `walletBalances` (`{ totalUsd, tokens, chain }`) so the
//! `/api/wallet-assets` route treats every chain uniformly.
//!
//! These hit third-party endpoints whose current-state responses drift, so they
//! are verified structurally rather than golden-tested; every JS quirk (rounding,
//! the `usd > 1 && usd < 1e9` gates, `?? 6` decimals, symbol slicing) is preserved.

use crate::jsmath::{js_num, js_round};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(9)).build().unwrap())
}

async fn get(url: &str) -> Option<Value> {
    let r = http().get(url).header("accept", "application/json").send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.json().await.ok()
}

/// `Number(v)`-style coercion: number as-is, decimal string parsed, else 0.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
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

/// nonevm.js `chainType` — classify an address by format. Probes in order:
/// EVM, Tron, Bitcoin, Solana; the first match wins.
pub fn chain_type(addr: &str) -> &'static str {
    fn base58(s: &str, min: usize, max: usize) -> bool {
        let n = s.chars().count();
        n >= min && n <= max && s.chars().all(|c| "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz".contains(c))
    }
    let is_hex40 = addr.len() == 42 && addr.starts_with("0x") && addr[2..].bytes().all(|b| b.is_ascii_hexdigit());
    if is_hex40 {
        return "evm";
    }
    // ^T[1-9A-HJ-NP-Za-km-z]{33}$ — Tron: 'T' + 33 base58 chars (34 total).
    if addr.starts_with('T') && addr.len() == 34 && base58(&addr[1..], 33, 33) {
        return "tron";
    }
    // ^(bc1|[13])[a-zA-HJ-NP-Z0-9]{20,70}$ — Bitcoin.
    if bitcoin_like(addr) {
        return "btc";
    }
    // ^[1-9A-HJ-NP-Za-km-z]{32,44}$ — Solana base58.
    if base58(addr, 32, 44) {
        return "solana";
    }
    "unknown"
}

/// `^(bc1|[13])[a-zA-HJ-NP-Z0-9]{20,70}$` — a bech32 or base58 BTC address.
fn bitcoin_like(addr: &str) -> bool {
    let rest = if let Some(r) = addr.strip_prefix("bc1") {
        r
    } else if addr.starts_with('1') || addr.starts_with('3') {
        &addr[1..]
    } else {
        return false;
    };
    // [a-zA-HJ-NP-Z0-9]{20,70}: all lowercase + digits, uppercase minus I and O.
    let n = rest.chars().count();
    n >= 20 && n <= 70 && rest.chars().all(|c| c.is_ascii_alphanumeric() && !matches!(c, 'I' | 'O'))
}

/// nonevm.js `btcBalance` — spendable BTC via mempool.space, priced by DefiLlama.
async fn btc_balance(addr: &str) -> Value {
    let d = match get(&format!("https://mempool.space/api/address/{addr}")).await {
        Some(d) => d,
        None => return json!({"totalUsd": 0, "tokens": [], "chain": "bitcoin"}),
    };
    let sats = num(&d["chain_stats"]["funded_txo_sum"]) - num(&d["chain_stats"]["spent_txo_sum"]);
    let btc = sats / 1e8;
    let px = crate::prices::native_price_usd("bitcoin").await;
    let usd = js_round(btc * px);
    let tokens = if usd > 0.0 {
        vec![json!({"symbol": "BTC", "name": "Bitcoin", "balance": js_num(btc), "valueUsd": js_num(usd), "chains": 1})]
    } else {
        vec![]
    };
    json!({"totalUsd": js_num(usd), "tokens": tokens, "chain": "bitcoin"})
}

/// nonevm.js `tronBalance` — TRX + TRC-20 holdings via Tronscan.
async fn tron_balance(addr: &str) -> Value {
    let d = match get(&format!("https://apilist.tronscanapi.com/api/account?address={addr}")).await {
        Some(d) => d,
        None => return json!({"totalUsd": 0, "tokens": [], "chain": "tron"}),
    };
    let mut out: Vec<Value> = Vec::new();
    let trx = num(&d["balance"]) / 1e6;
    if trx > 0.0 {
        let px = crate::prices::native_price_usd("tron").await;
        if trx * px > 0.0 {
            out.push(json!({"symbol": "TRX", "name": "TRON", "balance": js_num(trx), "valueUsd": js_num(js_round(trx * px)), "chains": 1}));
        }
    }
    for t in d["trc20token_balances"].as_array().unwrap_or(&vec![]) {
        // Number(t.tokenDecimal ?? 6) — nullish: only null/absent falls back to 6.
        let dec = if t["tokenDecimal"].is_null() { 6.0 } else { num(&t["tokenDecimal"]) };
        let bal = num(&t["balance"]) / 10f64.powf(dec);
        let price = if truthy(&t["tokenPriceInUsd"]) { num(&t["tokenPriceInUsd"]) } else { 0.0 };
        let usd = bal * price;
        if usd > 1.0 && usd < 1e9 {
            let symbol: String =
                t["tokenAbbr"].as_str().filter(|s| !s.is_empty()).unwrap_or("?").to_uppercase().chars().take(12).collect();
            let name = t["tokenName"].as_str().unwrap_or("");
            out.push(json!({"symbol": symbol, "name": name, "balance": js_num(bal), "valueUsd": js_num(js_round(usd)), "chains": 1}));
        }
    }
    out.sort_by(|a, b| b["valueUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["valueUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = out.iter().map(|t| t["valueUsd"].as_f64().unwrap_or(0.0)).sum();
    json!({"totalUsd": js_num(total), "tokens": out, "chain": "tron"})
}

/// One Solana JSON-RPC call, returning `result` (or Null).
async fn sol_rpc(method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let resp = match http().post("https://api.mainnet-beta.solana.com").header("content-type", "application/json").json(&body).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Value::Null,
    };
    let d: Value = resp.json().await.unwrap_or(Value::Null);
    d["result"].clone()
}

/// nonevm.js `solanaBalance` — SOL + SPL tokens via public RPC, SPL priced by
/// DefiLlama. Best-effort; capped at 30 tokens.
async fn solana_balance(addr: &str) -> Value {
    let mut out: Vec<Value> = Vec::new();
    let bal_r = sol_rpc("getBalance", json!([addr])).await;
    let sol = if !bal_r["value"].is_null() { num(&bal_r["value"]) / 1e9 } else { 0.0 };
    if sol > 0.0 {
        let px = crate::prices::native_price_usd("solana").await;
        if sol * px > 0.0 {
            out.push(json!({"symbol": "SOL", "name": "Solana", "balance": js_num(sol), "valueUsd": js_num(js_round(sol * px)), "chains": 1}));
        }
    }
    let accs = sol_rpc(
        "getTokenAccountsByOwner",
        json!([addr, {"programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"}, {"encoding": "jsonParsed"}]),
    )
    .await;
    let mut holds: Vec<(String, f64)> = Vec::new();
    for a in accs["value"].as_array().unwrap_or(&vec![]) {
        let info = &a["account"]["data"]["parsed"]["info"];
        let amt = info["tokenAmount"]["uiAmount"].as_f64().unwrap_or(0.0);
        if amt > 0.0 {
            if let Some(mint) = info["mint"].as_str().filter(|s| !s.is_empty()) {
                holds.push((mint.to_string(), amt));
            }
        }
    }
    if !holds.is_empty() {
        let items: Vec<(String, String)> = holds.iter().map(|(m, _)| ("solana".to_string(), m.clone())).collect();
        let px = crate::prices::token_prices_current(&items).await;
        for (mint, amount) in &holds {
            let usd = amount * px.get(&format!("solana:{mint}")).copied().unwrap_or(0.0);
            if usd > 1.0 && usd < 1e9 {
                let sym: String = mint.chars().take(4).collect::<String>() + "…";
                out.push(json!({"symbol": sym, "name": "SPL", "balance": js_num(*amount), "valueUsd": js_num(js_round(usd)), "chains": 1}));
            }
        }
    }
    out.sort_by(|a, b| b["valueUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["valueUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(30);
    let total: f64 = out.iter().map(|t| t["valueUsd"].as_f64().unwrap_or(0.0)).sum();
    json!({"totalUsd": js_num(total), "tokens": out, "chain": "solana"})
}

/// nonevm.js `nonEvmBalances` — dispatch by address type.
///
/// nonevm.js:75/83/98 кешують кожен ланцюг по 5 хвилин (`bal:btc:` / `bal:tron:` / `bal:sol:`) —
/// порт цього не переніс. Джерела тут безключові й лімітовані за IP (mempool.space, tronscan,
/// публічний Solana RPC), тобто без кешу ми самі виробляємо ті 429, від яких потім страждаємо.
pub async fn non_evm_balances(addr: &str) -> Value {
    let ty = chain_type(addr);
    let ck = format!("bal:{ty}:{addr}");
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(300)) {
        return v;
    }
    let v = match ty {
        "btc" => btc_balance(addr).await,
        "tron" => tron_balance(addr).await,
        "solana" => solana_balance(addr).await,
        _ => return json!({"totalUsd": 0, "tokens": [], "chain": ty}),
    };
    // Нульовий баланс кешуємо (це валідна відповідь), а от порожній об'єкт без chain — ні:
    // він означав би, що джерело не відповіло.
    if v["chain"].is_string() {
        crate::bs::cache_value(&ck, &v);
    }
    v
}

// ── Counterparties (for /api/wallet-graph) — first-seen order like a JS Map ──

/// Ordered counterparty accumulator: bump `addr` (name kept from first insert).
fn cp_bump(cp: &mut Vec<(String, i64, String)>, addr: &str, name: &str) {
    if let Some(e) = cp.iter_mut().find(|(a, _, _)| a == addr) {
        e.1 += 1;
    } else {
        cp.push((addr.to_string(), 1, name.to_string()));
    }
}

fn cp_to_json(cp: &[(String, i64, String)]) -> Vec<Value> {
    cp.iter().map(|(a, c, n)| json!({"addr": a, "count": c, "name": n, "contract": false})).collect()
}

/// nonevm.js `btcCounterparties` — mempool.space tx graph. `None` on fetch error.
pub async fn btc_counterparties(address: &str) -> Option<(Vec<Value>, i64, &'static str)> {
    let txs = get(&format!("https://mempool.space/api/address/{address}/txs")).await?;
    let arr = txs.as_array().cloned().unwrap_or_default();
    let mut cp: Vec<(String, i64, String)> = Vec::new();
    for tx in &arr {
        let ins: Vec<&str> = tx["vin"].as_array().map(|a| a.iter().filter_map(|v| v["prevout"]["scriptpubkey_address"].as_str()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
        let outs: Vec<&str> = tx["vout"].as_array().map(|a| a.iter().filter_map(|o| o["scriptpubkey_address"].as_str()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
        let sent = ins.contains(&address);
        for o in if sent { &outs } else { &ins } {
            if !o.is_empty() && *o != address {
                cp_bump(&mut cp, o, "");
            }
        }
    }
    Some((cp_to_json(&cp), arr.len() as i64, "bitcoin"))
}

/// nonevm.js `tronCounterparties` — TronGrid TRC20 transfers. `None` on error.
pub async fn tron_counterparties(address: &str) -> Option<(Vec<Value>, i64, &'static str)> {
    let d = get(&format!("https://api.trongrid.io/v1/accounts/{address}/transactions/trc20?limit=50")).await?;
    let items = d["data"].as_array().cloned().unwrap_or_default();
    let mut cp: Vec<(String, i64, String)> = Vec::new();
    for it in &items {
        let from = it["from"].as_str().unwrap_or("");
        let to = it["to"].as_str().unwrap_or("");
        let other = if from == address { to } else { from };
        if !other.is_empty() && other != address {
            cp_bump(&mut cp, other, it["token_info"]["symbol"].as_str().unwrap_or(""));
        }
    }
    Some((cp_to_json(&cp), items.len() as i64, "tron"))
}

/// nonevm.js `solanaCounterparties` — signatures → tx account keys (best-effort).
/// `None` only when the first RPC (getSignaturesForAddress) fails, matching JS.
pub async fn solana_counterparties(address: &str) -> Option<(Vec<Value>, i64, &'static str)> {
    let sigs_r = sol_rpc("getSignaturesForAddress", json!([address, {"limit": 20}])).await;
    if sigs_r.is_null() {
        return None; // JS: catch → return null
    }
    let sigs = sigs_r.as_array().cloned().unwrap_or_default();
    let mut cp: Vec<(String, i64, String)> = Vec::new();
    let mut total_tx = 0i64;
    for s in sigs.iter().take(12) {
        let sig = s["signature"].as_str().unwrap_or("");
        let tx = sol_rpc("getTransaction", json!([sig, {"maxSupportedTransactionVersion": 0, "encoding": "jsonParsed"}])).await;
        if tx.is_null() {
            continue; // JS try/catch skip
        }
        total_tx += 1;
        for k in tx["transaction"]["message"]["accountKeys"].as_array().unwrap_or(&vec![]) {
            let pk = if k.is_string() { k.as_str().unwrap_or("") } else { k["pubkey"].as_str().unwrap_or("") };
            if !pk.is_empty() && pk != address {
                cp_bump(&mut cp, pk, "");
            }
        }
    }
    Some((cp_to_json(&cp), total_tx, "solana"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_type_classifies() {
        assert_eq!(chain_type("0xab5801a7d398351b8be11c439e05c5b3259aec9b"), "evm");
        assert_eq!(chain_type("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t"), "tron"); // 34 chars, T-prefixed
        assert_eq!(chain_type("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"), "btc");
        assert_eq!(chain_type("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"), "btc"); // legacy P2PKH
        assert_eq!(chain_type("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"), "solana");
        assert_eq!(chain_type("nonsense"), "unknown");
    }
}
