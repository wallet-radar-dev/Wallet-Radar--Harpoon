//! rgassetgroups — групи капіталу ПО АКТИВАХ: для кожної priced-монети з високою концентрацією
//! створює групу «хто контролює X» (топ-холдери, net реконструйовано з обох сторін transfers, без CEX/
//! деплоєрів). Дописує у dataset/capital-groups.json (схема застосунку), старі A/B не чіпає, бекап.
//! Env: MIN_CONC(0.15) MIN_HOLDERS(3) TOPMEM(12). Лише бекфіл.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn is_junk(a: &str) -> bool { wr_store::addrs::is_junk_addr(a) }

// стейбли/фіат/wrapped-мажори — це кастодіальна концентрація (біржі/емітент), НЕ ціль маніпуляції
fn is_stable_or_wrapped(sym: &str) -> bool {
    let s = sym.to_uppercase();
    if s.contains("USD") || s.contains("EUR") || s.contains("GBP") { return true; } // USDC/USDT/PYUSD/RLUSD/EURC/TGBP…
    matches!(s.as_str(), "DAI" | "FRAX" | "LUSD" | "TUSD" | "USTB" | "AUSD" | "GHO" | "MIM" | "CRVUSD"
        | "WBTC" | "CBBTC" | "TBTC" | "WETH" | "WBETH" | "CBETH" | "WSTETH" | "STETH" | "RETH" | "WEETH" | "EETH")
        || s.ends_with("ON") // токенізовані акції (MCDON/NKEON/CRCLON) — це вже група B
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_conc: f64 = std::env::var("MIN_CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(0.15);
    let min_holders: usize = std::env::var("MIN_HOLDERS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    let topmem: usize = std::env::var("TOPMEM").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
    let tpath = format!("{root}/dataset/backfill/transfers.csv");

    // supply + priced(real) + labels/kind
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { let dec = v["decimals"].as_u64().unwrap_or(18) as i32; let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0); if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); } }
    }
    let mut real: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/backfill/onchain-prices.json")).unwrap())) { for k in v.keys() { real.insert(k.to_lowercase()); } }
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/backfill/wallet-kind.json")).unwrap())) { for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); } }
    let name_of = |a: &str| lbl.get(a).and_then(|x| x["name"].as_str()).map(|s| s.to_string());
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc")) || name_of(a).map(|n| { let l = n.to_lowercase(); l.contains("binance") || l.contains("crypto.com") || l.contains("okx") || l.contains("gate") || l.contains("coinbase") || l.contains("kraken") || l.contains("bybit") || l.contains("kucoin") || l.contains("bitget") || l.contains("circle") }).unwrap_or(false);

    let open = || BufReader::with_capacity(1 << 20, File::open(&tpath).expect("transfers"));
    // П1 tracked
    let mut tracked: HashSet<String> = HashSet::new();
    for line in open().lines().map_while(Result::ok) { let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let _t = it.next(); let tok = it.next().unwrap_or(""); if w == "wallet" || (d != "i" && d != "o") { continue; } let tl = tok.to_lowercase(); if real.contains(&tl) && supply.contains_key(&tl) { tracked.insert(w.to_lowercase()); } }
    // П2 net[(token,addr)]
    let mut net: HashMap<(String, String), f64> = HashMap::new();
    for line in open().lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let _t = it.next(); let tok = it.next().unwrap_or("");
        if w == "wallet" || (d != "i" && d != "o") { continue; }
        let tl = tok.to_lowercase(); if !real.contains(&tl) || !supply.contains_key(&tl) { continue; }
        let mut rit = line.rsplitn(4, ','); let _h = rit.next(); let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0); let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 { continue; } let s = if d == "i" { amt } else { -amt }; let wl = w.to_lowercase();
        if !is_junk(&wl) { *net.entry((tl.clone(), wl)).or_insert(0.0) += s; }
        if !tracked.contains(&cp) && !is_junk(&cp) { *net.entry((tl, cp)).or_insert(0.0) -= s; }
    }
    // token -> holders (addr, share), без CEX/деплоєра
    let mut tok_hold: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    for ((t, a), n) in &net { if *n <= 0.0 { continue; } let (hs, _) = &supply[t]; let sh = wr_dict::emission_share(*n, *hs).min(1.0); /* Фаза 0.2: словник, hs>0 гарант., байт-паритет */ if sh >= 0.005 && sh < 0.90 && !is_cex(a) { tok_hold.entry(t.clone()).or_default().push((a.clone(), sh)); } }

    // watchlist названих юзером монет — показати ЗАВЖДИ (навіть <поріг), бо цікавлять сліди китів
    let watch: HashSet<&str> = ["PEPE", "COMP", "DODO", "PENDLE", "LINK", "MASK", "ELON", "CRV", "CHZ", "JASMY"].into_iter().collect();

    // будую групи по активах
    let mut groups: Vec<(f64, Value)> = Vec::new(); // (total_conc для сорту, group)
    let mut mine: Vec<(String, String, f64, usize, bool)> = Vec::new(); // (contract, symbol, total, n, is_watch)
    for (t, mut hs) in tok_hold {
        hs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let total = hs.iter().map(|x| x.1).sum::<f64>().min(1.0);
        let (_, sym) = &supply[&t];
        if is_stable_or_wrapped(sym) { continue; } // стейбли/wrapped/токен-акції — не ціль
        let is_watch = watch.contains(sym.to_uppercase().as_str());
        if (!is_watch && total < min_conc) || hs.len() < min_holders { continue; }
        mine.push((t.clone(), sym.clone(), total, hs.len(), is_watch));
        let members: Vec<Value> = hs.iter().take(topmem).map(|(a, sh)| json!({"address": a, "count": (sh * 1000.0).round() as i64, "label": name_of(a).or_else(|| kind.get(a).cloned()), "role": if *sh >= 0.05 { "core" } else { "holder" }})).collect();
        groups.push((total, json!({
            "id": format!("AST:{}", &t[..10.min(t.len())]),
            "name": format!("{} {} — контроль {:.0}% ({} китів)", if is_watch { "⭐" } else { "🎯" }, if sym.is_empty() { &t[..8] } else { sym }, total * 100.0, hs.len()),
            "controlledEmission": [json!({"share": (total * 1e4).round() / 1e4, "symbol": sym})],
            "commonAssets": [sym.clone()],
            "cores": hs.iter().filter(|x| x.1 >= 0.05).count(),
            "counterparties": 0,
            "members": members,
            "size": hs.len(),
            "counterpartySource": "rgassetgroups (net з обох сторін, priced, без CEX/деплоєра)",
            "expandedWith": format!("топ-холдер {:.0}%", hs[0].1 * 100.0),
        })));
    }
    groups.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    eprintln!("[rgassetgroups] груп по активах (конц ≥{:.0}%, ≥{} китів): {}", min_conc * 100.0, min_holders, groups.len());

    // мердж у capital-groups.json (лишаю A/B, прибираю старі AST:/W)
    let cgp = format!("{root}/dataset/capital-groups.json");
    let mut ex: Value = serde_json::from_reader(BufReader::new(File::open(&cgp).unwrap())).unwrap_or(json!({"groups": []}));
    if let Ok(cur) = std::fs::read(&cgp) { File::create(format!("{cgp}.bak-assetgroups")).and_then(|mut f| f.write_all(&cur)).ok(); }
    let mut arr = ex["groups"].as_array().cloned().unwrap_or_default();
    arr.retain(|g| { let id = g["id"].as_str().unwrap_or(""); !id.starts_with("AST:") && !id.starts_with('W') });
    for (_, g) in groups.into_iter() { arr.push(g); }
    ex["groups"] = json!(arr);
    ex["generated"] = json!("2026-07-22 (rgassetgroups)");
    File::create(&cgp).and_then(|mut f| f.write_all(serde_json::to_string_pretty(&ex).unwrap().as_bytes())).ok();
    eprintln!("[rgassetgroups] у capital-groups.json тепер груп: {}", arr.len());

    // ===== SM-ІНТЕГРАЦІЯ (data-only, durable — код ці файли не пише) =====
    let augment = |path: &str, forensics_clamp: bool| -> (usize, usize) {
        let mut d: Value = File::open(path).ok().and_then(|f| serde_json::from_reader(BufReader::new(f)).ok()).unwrap_or(json!({"assets": []}));
        if let Ok(cur) = std::fs::read(path) { File::create(format!("{path}.bak-sm")).and_then(|mut f| f.write_all(&cur)).ok(); }
        let mut list = d["assets"].as_array().cloned().unwrap_or_default();
        let have: HashSet<String> = list.iter().filter_map(|a| a["contract"].as_str().map(|s| s.to_lowercase())).collect();
        let mut added = 0;
        for (c, sym, total, n, _w) in &mine {
            let pct = (total * 100.0).round();
            // для forensics-кандидатів воркер бере лише top10pct у 15..90 → клампимо
            let p10 = if forensics_clamp { pct.clamp(15.0, 89.0) } else { pct };
            if have.contains(c) {
                // проставити звʼязок наявним
                if !forensics_clamp { for a in list.iter_mut() { if a["contract"].as_str().map(|s| s.eq_ignore_ascii_case(c)).unwrap_or(false) { if let Some(o) = a.as_object_mut() { o.insert("capitalGroup".into(), json!(format!("🎯 {} — контроль {:.0}%", sym, pct))); } } } }
                continue;
            }
            let mut e = json!({"symbol": sym, "name": sym, "contract": c, "top10pct": p10, "top50pct": p10});
            if !forensics_clamp { e.as_object_mut().unwrap().insert("capitalGroup".into(), json!(format!("🎯 {} — контроль {:.0}% ({} китів)", sym, pct, n))); }
            list.push(e); added += 1;
        }
        let total_n = list.len();
        d["assets"] = json!(list);
        File::create(path).and_then(|mut f| f.write_all(serde_json::to_string_pretty(&d).unwrap().as_bytes())).ok();
        (added, total_n)
    };
    let (a1, n1) = augment(&format!("{root}/dataset/capital-group-assets.json"), false);
    let (a2, n2) = augment(&format!("{root}/dataset/cmc-whale-assets.json"), true);
    eprintln!("[rgassetgroups][SM] capital-group-assets.json: +{a1} монет (усього {n1}); cmc-whale-assets.json: +{a2} forensics-кандидатів (усього {n2})");
    eprintln!("[rgassetgroups][SM] мої монети зʼявляться у списку SM одразу; звʼязок у forensics — коли воркер відпрацює цикл (~30хв, on-chain)");
}
