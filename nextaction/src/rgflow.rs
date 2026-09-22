//! rgflow — ТОЧНИЙ скан відтоку китової мережі: куди йдуть монети, коли позиція падає?
//!   → біржа (CEX) = ПРОДАЖ · → контракт (DEX/DeFi) = своп/продаж · → інший кит = внутрішній ·
//!   → новий EOA = РОТАЦІЯ (все ще в мережі, НЕ продаж!). Розрізняє реальний продаж від ротації.
//! Збирає свіжі гаманці-ротації (отримали від ≥2 китів) = розширення мережі. Вікно RECENT (ост. міс).
//! Анотує групи sell/rotate + realPhase; пише rgflow.json (+ network-expansion). Env: RECENT_MONTHS(3).
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn is_junk(a: &str) -> bool { a.is_empty() || wr_store::addrs::is_junk_addr(a) }
fn is_stable(sym: &str) -> bool { let s = sym.to_uppercase(); s.contains("USD") || s.contains("EUR") || s.contains("GBP") || matches!(s.as_str(), "DAI"|"FRAX"|"LUSD"|"TUSD"|"USTB"|"AUSD"|"GHO"|"MIM"|"WBTC"|"CBBTC"|"TBTC"|"WETH"|"WBETH"|"CBETH"|"WSTETH"|"STETH"|"RETH"|"WEETH"|"EETH") || s.ends_with("ON") }
fn ym(ts: &str) -> i32 { if ts.len() < 7 { return i32::MIN; } let y: i32 = ts[0..4].parse().unwrap_or(0); let m: i32 = ts[5..7].parse().unwrap_or(0); if y < 2015 || m < 1 || m > 12 { return i32::MIN; } (y - 2020) * 12 + (m - 1) }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let recent_months: i32 = std::env::var("RECENT_MONTHS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let bf = format!("{root}/dataset/backfill");

    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { let dec = v["decimals"].as_u64().unwrap_or(18) as i32; let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0); if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); } }
    }
    let mut real: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())) { for k in v.keys() { real.insert(k.to_lowercase()); } }
    let priced_alt = |t: &str| real.contains(t) && supply.get(t).map(|(_, s)| !is_stable(s)).unwrap_or(false);
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) { for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); } }
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc")) || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase(); ["binance","crypto.com","okx","gate","coinbase","kraken","bybit","kucoin","bitget","circle","bridge","galaxy","falconx","wintermute","cumberland","tether","bitfinex","huobi","mexc","gemini","korbit","upbit","bithumb","uphold","bittrex","poloniex","paxos","robinhood","deposit funder"].iter().any(|c| l.contains(c)) }).unwrap_or(false);
    let is_contract = |a: &str| kind.get(a).map(|s| s == "contract").unwrap_or(false);

    let tpath = format!("{bf}/transfers.csv");
    let open = || BufReader::with_capacity(1 << 20, File::open(&tpath).expect("transfers"));
    // ПАС 1: tracked + net -> whale set W (net>0 холдери priced-альтів, non-cex, <90%)
    let mut tracked: HashSet<String> = HashSet::new();
    let mut maxm = i32::MIN;
    for line in open().lines().map_while(Result::ok) { let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or(""); if w == "wallet" || (d != "i" && d != "o") { continue; } let mi = ym(ts); if mi != i32::MIN && mi > maxm { maxm = mi; } if priced_alt(&tok.to_lowercase()) { tracked.insert(w.to_lowercase()); } }
    let recent_from = maxm - (recent_months - 1);
    let mut net: HashMap<(String, String), f64> = HashMap::new();
    for line in open().lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let _t = it.next(); let tok = it.next().unwrap_or("");
        if w == "wallet" || (d != "i" && d != "o") { continue; }
        let tl = tok.to_lowercase(); if !priced_alt(&tl) { continue; }
        let mut rit = line.rsplitn(4, ','); let _h = rit.next(); let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0); let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 { continue; } let s = if d == "i" { amt } else { -amt }; let wl = w.to_lowercase();
        if !is_junk(&wl) { *net.entry((tl.clone(), wl)).or_insert(0.0) += s; }
        if !tracked.contains(&cp) && !is_junk(&cp) { *net.entry((tl, cp)).or_insert(0.0) -= s; }
    }
    // whale set + холдери на токен
    let mut whales: HashSet<String> = HashSet::new();
    let mut tok_holders: HashMap<String, HashSet<String>> = HashMap::new();
    for ((t, a), n) in &net { if *n <= 0.0 { continue; } let (hs, _) = &supply[t]; let sh = n / hs; if sh >= 0.0005 && sh < 0.90 && !is_cex(a) { whales.insert(a.clone()); tok_holders.entry(t.clone()).or_default().insert(a.clone()); } }
    eprintln!("[rgflow] китів (net>0 холдерів): {}, максимальний місяць даних: {}", whales.len(), { let y = 2020 + maxm.div_euclid(12); let m = maxm.rem_euclid(12) + 1; format!("{y}-{m:02}") });

    // ПАС 3: класифікація ВІДТОКУ (dir=o) від китів по priced-альтах
    // out[token] = [cex, dex, internal, rotate]  (token-units), + recent-версія
    let mut out_all: HashMap<String, [f64; 4]> = HashMap::new();
    let mut out_rec: HashMap<String, [f64; 4]> = HashMap::new();
    // кандидати-ротації: новий EOA -> {кити-відправники}, {токени}
    let mut rot_senders: HashMap<String, HashSet<String>> = HashMap::new();
    for line in open().lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or("");
        if d != "o" { continue; }
        let wl = w.to_lowercase(); if !whales.contains(&wl) { continue; }
        let tl = tok.to_lowercase(); if !priced_alt(&tl) { continue; }
        let mut rit = line.rsplitn(4, ','); let _h = rit.next(); let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0); let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 || is_junk(&cp) || cp == wl { continue; }
        let idx = if is_cex(&cp) { 0 } else if is_contract(&cp) { 1 } else if whales.contains(&cp) { 2 } else { 3 };
        out_all.entry(tl.clone()).or_insert([0.0; 4])[idx] += amt;
        let mi = ym(ts);
        if mi != i32::MIN && mi >= recent_from { out_rec.entry(tl.clone()).or_insert([0.0; 4])[idx] += amt; }
        if idx == 3 { rot_senders.entry(cp).or_default().insert(wl); } // новий EOA отримав від кита
    }

    // мережеве розширення: свіжі EOA, що отримали від ≥2 китів (ротація-ціль) і ще не в whales
    let mut expansion: Vec<(String, usize)> = rot_senders.iter().filter(|(a, s)| s.len() >= 2 && !whales.contains(*a) && !is_cex(a) && !is_contract(a)).map(|(a, s)| (a.clone(), s.len())).collect();
    expansion.sort_by(|a, b| b.1.cmp(&a.1));
    eprintln!("[rgflow] свіжих гаманців-ротацій (від ≥2 китів, поза whales): {} → розширення мережі", expansion.len());

    // на кожну priced-групу: розклад відтоку + реальна фаза
    let cgp = format!("{root}/dataset/capital-groups.json");
    let cg: Value = serde_json::from_reader(BufReader::new(File::open(&cgp).unwrap())).unwrap();
    let cap_syms: HashSet<String> = cg["groups"].as_array().map(|a| a.iter().filter(|g| g["id"].as_str().unwrap_or("").starts_with("CAP:")).filter_map(|g| g["commonAssets"][0].as_str().map(|s| s.to_uppercase())).collect()).unwrap_or_default();
    let mut rows: Vec<Value> = Vec::new();
    for (t, sym) in supply.iter().map(|(t, (_, s))| (t.clone(), s.clone())).collect::<Vec<_>>() {
        if !priced_alt(&t) { continue; }
        let Some(oa) = out_all.get(&t) else { continue };
        let tot: f64 = oa.iter().sum(); if tot <= 0.0 { continue; }
        let held: f64 = tok_holders.get(&t).map(|hs| hs.iter().map(|a| net.get(&(t.clone(), a.clone())).copied().unwrap_or(0.0).max(0.0)).sum()).unwrap_or(0.0);
        let orr = out_rec.get(&t).copied().unwrap_or([0.0; 4]);
        let rec_tot: f64 = orr.iter().sum();
        let sold = oa[0] + oa[1]; let rotate = oa[3]; let internal = oa[2];
        let rec_sold = orr[0] + orr[1];
        let sell_pct = sold / tot; let rot_pct = rotate / tot;
        // РЕАЛЬНА фаза: дивимось лише продаж (біржа+DEX) відносно утримання
        let rec_sold_vs_held = if held > 0.0 { rec_sold / held } else { 0.0 };
        let phase = if rec_sold_vs_held >= 0.05 { "РОЗДАЄ (біржа/DEX)" }
            else if rec_tot > 0.0 && orr[3] / rec_tot.max(1.0) >= 0.5 && orr[3] > held * 0.03 { "РОТАЦІЯ (нові гаманці)" }
            else { "ТРИМАЄ/НАКОПИЧУЄ" };
        rows.push(json!({
            "symbol": sym, "contract": t, "inCap": cap_syms.contains(&sym.to_uppercase()),
            "outTotal": tot.round(), "heldNet": held.round(),
            "toCexPct": (oa[0] / tot * 1e3).round() / 1e3, "toDexPct": (oa[1] / tot * 1e3).round() / 1e3,
            "internalPct": (internal / tot * 1e3).round() / 1e3, "rotatePct": (rot_pct * 1e3).round() / 1e3,
            "sellPct": (sell_pct * 1e3).round() / 1e3,
            "recentSoldVsHeld": (rec_sold_vs_held * 1e3).round() / 1e3,
            "realPhase": phase,
        }));
    }
    rows.sort_by(|a, b| b["recentSoldVsHeld"].as_f64().unwrap().partial_cmp(&a["recentSoldVsHeld"].as_f64().unwrap()).unwrap());
    let n_sell = rows.iter().filter(|r| r["realPhase"] == "РОЗДАЄ (біржа/DEX)").count();
    let n_rot = rows.iter().filter(|r| r["realPhase"] == "РОТАЦІЯ (нові гаманці)").count();
    eprintln!("[rgflow] альтів: {} · РЕАЛЬНО РОЗДАЮТЬ (біржа/DEX): {} · РОТУЮТЬ у нові гаманці: {}", rows.len(), n_sell, n_rot);
    eprintln!("[rgflow] --- ТОП за реальним продажем (ост. {} міс, % від утримання) ---", recent_months);
    for r in rows.iter().filter(|r| r["inCap"].as_bool().unwrap_or(false)).take(22) {
        eprintln!("   {:<8} {:<22} продаж:CEX {:.0}%/DEX {:.0}% · ротація {:.0}% · внутр {:.0}% · ост.продаж {:.1}% утрим",
            r["symbol"].as_str().unwrap(), r["realPhase"].as_str().unwrap(),
            r["toCexPct"].as_f64().unwrap() * 100.0, r["toDexPct"].as_f64().unwrap() * 100.0,
            r["rotatePct"].as_f64().unwrap() * 100.0, r["internalPct"].as_f64().unwrap() * 100.0,
            r["recentSoldVsHeld"].as_f64().unwrap() * 100.0);
    }

    nextaction::safewrite::save_json_or_die("rgflow", &format!("{bf}/rgflow.json"), &json!({
        "generated": "2026-07-23", "recentMonths": recent_months, "whales": whales.len(),
        "assets": rows,
        "networkExpansion": expansion.iter().take(500).map(|(a, n)| json!({"addr": a, "fromWhales": n})).collect::<Vec<_>>(),
    }));
    // анотую CAP-групи
    let bym: HashMap<String, Value> = rows.iter().map(|r| (r["symbol"].as_str().unwrap().to_uppercase(), r.clone())).collect();
    let mut cg2 = cg.clone();
    if let Ok(cur) = std::fs::read(&cgp) { File::create(format!("{cgp}.bak-flow")).and_then(|mut f| f.write_all(&cur)).ok(); }
    let mut nn = 0;
    if let Some(arr) = cg2["groups"].as_array_mut() {
        for g in arr.iter_mut() {
            if !g["id"].as_str().unwrap_or("").starts_with("CAP:") { continue; }
            let sym = g["commonAssets"][0].as_str().unwrap_or("").to_uppercase();
            if let Some(r) = bym.get(&sym) {
                let ph = r["realPhase"].as_str().unwrap_or("");
                let mark = if ph.starts_with("РОЗДАЄ") { "🔴" } else if ph.starts_with("РОТАЦІЯ") { "🟠" } else { "🟢" };
                let raw = g["name"].as_str().unwrap_or("").to_string();
                let base = if raw.starts_with('🔴') || raw.starts_with('🟠') || raw.starts_with('🟢') || raw.starts_with('⚪') {
                    raw.splitn(2, "· ").nth(1).unwrap_or(&raw).to_string()
                } else { raw };
                if let Some(o) = g.as_object_mut() {
                    o.insert("sellPct".into(), r["sellPct"].clone()); o.insert("rotatePct".into(), r["rotatePct"].clone());
                    o.insert("toCexPct".into(), r["toCexPct"].clone()); o.insert("realPhase".into(), r["realPhase"].clone());
                    o.insert("name".into(), json!(format!("{} {} · {}", mark, ph, base)));
                    nn += 1;
                }
            }
        }
    }
    nextaction::safewrite::save_json_or_die("rgflow", &cgp, &cg2);
    eprintln!("[rgflow] анотовано CAP-груп реальним відтоком: {}; мережеве розширення у rgflow.json", nn);
}
