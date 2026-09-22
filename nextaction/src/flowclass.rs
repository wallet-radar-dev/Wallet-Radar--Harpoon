//! flowclass — розбирає вихідні потоки членів мережі на СПРАВЖНІ класи, а не «все на контракт = злив».
//!
//! Дефект, який лікуємо: і монітор, і статистика продажів вважали продажем БУДЬ-ЯКИЙ вихід токена
//! на контракт. Насправді там змішані: своп (реальний продаж), надання ліквідності, бридж на іншу
//! мережу і депозит/стейк. Перші три — різні наміри, і тільки перший є продажем.
//!
//! Метод: групуємо рядки transfers.csv за tx_hash (він у файлі є) і дивимось, що гаманець ОТРИМАВ
//! у тій самій транзакції:
//!   • призначення має мітку мосту                    → BRIDGE
//!   • у відповідь прийшов розписковий токен (LP/aToken/cToken) → LP (надання ліквідності)
//!   • у відповідь прийшов інший токен або ETH        → SWAP (продаж)
//!   • у відповідь нічого                             → DEPOSIT (депозит/стейк, не продаж)
//!   • призначення — біржа                            → CEX
//! Вихід: dataset/backfill/flow-classes.json {wallet:{swapUsd,lpUsd,bridgeUsd,depositUsd,cexUsd,counts}}
//! Args: ROOT [GROUP]. Env: CHAIN_FILTER(eth)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

/// розписковий токен = підтвердження вкладу, а не куплений актив (евристика за символом)
fn is_receipt(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.starts_with("UNI-V2") || s.starts_with("SLP") || s.ends_with("-LP") || (s.ends_with("LP") && s.len() > 2)
        || s.starts_with("AETH") || s.starts_with("AARB") || s.starts_with("AOPT") || s.starts_with("APOL") || s.starts_with("ABAS")
        || s.starts_with("CUSD") || s.starts_with("CDAI") || s.starts_with("CETH") || s.starts_with("STK")
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let group = std::env::args().nth(2).unwrap_or_else(|| "NET".into());
    let chain_filter = std::env::var("CHAIN_FILTER").unwrap_or_else(|_| "eth".into());
    let bf = format!("{root}/dataset/backfill");

    let memb: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/group-membership.json")).expect("membership"))).unwrap();
    let members: HashSet<String> = memb["wallets"].as_object().cloned().unwrap_or_default().iter()
        .filter(|(_, x)| x["group"].as_str() == Some(group.as_str())).map(|(w, _)| w.to_lowercase()).collect();
    let px: HashMap<String, f64> = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/token-prices.json")).unwrap())).ok()
        .and_then(|v| v["tokens"].as_object().map(|m| m.iter().filter_map(|(k, x)| x["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect())).unwrap_or_default();
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let name_of = |a: &str| lbl.get(a).and_then(|x| x["name"].as_str()).unwrap_or("").to_lowercase();
    let is_bridge = |a: &str| { let n = name_of(a);
        ["bridge", "arbitrum", "optimism", "zksync", "polygon", "base:", "hop", "across", "orbiter", "stargate", "synapse", "celer", "wormhole", "gateway"]
            .iter().any(|t| n.contains(t)) };
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc"))
        || { let n = name_of(a); ["binance","coinbase","okx","kraken","bybit","kucoin","bitget","huobi","mexc","bitfinex","upbit","bithumb","paribu","gate","exchange","hot wallet","deposit"].iter().any(|c| n.contains(c)) };
    let is_contract = |a: &str| kind.get(a).map(|s| s == "contract").unwrap_or(false);
    // ВАЖЛИВО: своп «токен → ETH» не має вхідного ТОКЕНА в тій самій транзакції (ETH не ERC-20,
    // а внутрішні перекази у transfers.csv не збираються). Через це такі продажі спершу впали
    // в «депозити» і SWAP вийшов $9. Тому додатково розрізняємо за ПРИРОДОЮ призначення.
    let is_swap_venue = |a: &str| { let n = name_of(a);
        ["uniswap", "sushiswap", "1inch", "0x:", "paraswap", "curve", "balancer", "router", "pool",
         "cow", "kyber", "dodo", "pancake", "matcha", "odos", "swap", "aggregator", "metamask"]
            .iter().any(|t| n.contains(t)) };
    let is_stake_venue = |a: &str| { let n = name_of(a);
        ["aave", "lido", "compound", "rocket", "eigen", "stake", "vault", "farm", "gauge", "deposit contract", "morpho", "spark"]
            .iter().any(|t| n.contains(t)) };

    // збираємо рядки членів, згруповані за (гаманець, tx_hash)
    #[derive(Default)]
    struct Tx { out: Vec<(String, String, f64)>, // (токен, символ, usd)
                in_syms: Vec<String>, dests: Vec<String> }
    let mut txs: HashMap<(String, String), Tx> = HashMap::new();
    let mut rows = 0u64;
    for line in BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).expect("transfers")).lines().map_while(Result::ok) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 9 || f[0] == "wallet" { continue; }
        let (w, ch, d, tok, sym, cp, amt, hash) = (f[0].to_lowercase(), f[1], f[2], f[4].to_lowercase(), f[5].to_string(), f[6].to_lowercase(), f[7].parse::<f64>().unwrap_or(0.0), f[8].to_string());
        if ch != chain_filter || !members.contains(&w) { continue; }
        rows += 1;
        let e = txs.entry((w, hash)).or_default();
        if d == "o" { e.out.push((tok.clone(), sym, amt * px.get(&tok).copied().unwrap_or(0.0))); e.dests.push(cp); }
        else if d == "i" { e.in_syms.push(sym); }
    }
    eprintln!("[flowclass] рядків членів: {rows} · транзакцій: {}", txs.len());

    // класифікація
    let mut per: HashMap<String, [f64; 6]> = HashMap::new();     // swap, lp, bridge, deposit, cex, unknown
    let mut cnt: HashMap<String, [u32; 6]> = HashMap::new();
    let mut totals = [0.0f64; 6];
    for ((w, _h), t) in &txs {
        if t.out.is_empty() { continue; }
        let usd: f64 = t.out.iter().map(|o| o.2).sum();
        let got_receipt = t.in_syms.iter().any(|s| is_receipt(s));
        let got_any = !t.in_syms.is_empty();
        let idx = if t.dests.iter().any(|d| is_cex(d)) { 4 }
            else if t.dests.iter().any(|d| is_bridge(d)) { 2 }
            else if got_receipt { 1 }                                   // повернули розписку = вклад
            else if got_any { 0 }                                       // повернули інший токен = своп
            else if t.dests.iter().any(|d| is_swap_venue(d)) { 0 }      // біржовий майданчик = своп за ETH
            else if t.dests.iter().any(|d| is_stake_venue(d)) { 3 }     // протокол вкладу = депозит
            else if t.dests.iter().any(|d| is_contract(d)) { 5 }        // немічений контракт = невідомо
            else { continue };
        per.entry(w.clone()).or_insert([0.0; 6])[idx] += usd;
        cnt.entry(w.clone()).or_insert([0; 6])[idx] += 1;
        totals[idx] += usd;
    }

    let names = ["SWAP (продаж)", "LP (ліквідність)", "BRIDGE (міст)", "DEPOSIT (депозит/стейк)", "CEX (біржа)", "НЕВІДОМИЙ контракт"];
    let sum: f64 = totals.iter().sum();
    eprintln!("[flowclass] ===== ЩО НАСПРАВДІ У «ВИХОДІ НА КОНТРАКТ» =====");
    for (i, n) in names.iter().enumerate() {
        eprintln!("   {:<24} ${:>14.0} ({:.1}%)", n, totals[i], if sum > 0.0 { 100.0 * totals[i] / sum } else { 0.0 });
    }
    let dexlike = totals[0] + totals[1] + totals[2] + totals[3] + totals[5];
    eprintln!("[flowclass] раніше все це рахувалось як «злив у DEX»: ${:.0}; насправді продаж — ${:.0} ({:.1}%)",
        dexlike, totals[0], if dexlike > 0.0 { 100.0 * totals[0] / dexlike } else { 0.0 });

    let out: serde_json::Map<String, Value> = per.iter().map(|(w, v)| {
        let c = cnt.get(w).copied().unwrap_or([0; 6]);
        (w.clone(), json!({"swapUsd": v[0], "lpUsd": v[1], "bridgeUsd": v[2], "depositUsd": v[3], "cexUsd": v[4], "unknownUsd": v[5],
                           "swapN": c[0], "lpN": c[1], "bridgeN": c[2], "depositN": c[3], "cexN": c[4], "unknownN": c[5]}))
    }).collect();
    std::fs::write(format!("{bf}/flow-classes.json"), serde_json::to_string(&json!({
        "group": group, "wallets": out.len(),
        "totals": {"swapUsd": totals[0], "lpUsd": totals[1], "bridgeUsd": totals[2], "depositUsd": totals[3], "cexUsd": totals[4], "unknownUsd": totals[5]},
        "wallets_map": out})).unwrap()).ok();
    eprintln!("[flowclass] збережено flow-classes.json ({} гаманців)", per.len());
}
