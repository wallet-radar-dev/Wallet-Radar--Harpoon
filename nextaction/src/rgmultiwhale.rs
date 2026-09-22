//! rgmultiwhale — знаходить МУЛЬТИ-МОНЕТНИХ КИТІВ (=групи капіталу): адреси, що тримають ≥WHALE_PCT%
//! емісії у ≥MIN_COINS монетах. Реконструює net-баланси tracked-гаманців І контрагентів (кити поза
//! whitelist). Лише бекфіл (2 проходи). Env: WHALE_PCT(1.0), MIN_COINS(2), TOPN(40).

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn is_junk(a: &str) -> bool { wr_store::addrs::is_junk_addr(a) }
fn uf_find(uf: &mut [u32], x: u32) -> u32 { let mut r = x; while uf[r as usize] != r { r = uf[r as usize]; } let mut c = x; while uf[c as usize] != c { let n = uf[c as usize]; uf[c as usize] = r; c = n; } r }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let whale_pct: f64 = std::env::var("WHALE_PCT").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let max_pct: f64 = std::env::var("MAX_PCT").ok().and_then(|s| s.parse().ok()).unwrap_or(90.0); // ≥MAX% = деплоєр/скам-單холдер, не кит
    let min_coins: usize = std::env::var("MIN_COINS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let tpath = format!("{root}/dataset/backfill/transfers.csv");
    let open = || BufReader::with_capacity(1 << 20, File::open(&tpath).expect("transfers.csv"));

    // supply tokens -> (human_supply, symbol), індекс
    let mut sup: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv {
            let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
            let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw > 0.0 { sup.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); }
        }
    }
    // РЕАЛЬНІ токени = мають ончейн-своп-ціну (свопляться проти стейблів; скам-меми зазвичай ні)
    let priced_only = std::env::var("PRICED_ONLY").map(|v| v != "0").unwrap_or(true);
    let mut real: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/backfill/onchain-prices.json")).unwrap())) {
        for k in v.keys() { real.insert(k.to_lowercase()); }
    }
    eprintln!("[multiwhale] реальних (priced) токенів: {} (priced_only={})", real.len(), priced_only);
    // мітки
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/backfill/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let tag = |a: &str| -> String {
        if let Some(n) = lbl.get(a).and_then(|x| x["name"].as_str()) { return n.to_string(); }
        kind.get(a).cloned().unwrap_or_else(|| "EOA?".into())
    };

    // П1: tracked-гаманці (колонка wallet) для supply-токенів
    let mut tracked: HashSet<String> = HashSet::new();
    for line in open().lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _c = it.next(); let dir = it.next().unwrap_or(""); let _ts = it.next();
        let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        if sup.contains_key(&tok.to_lowercase()) { tracked.insert(w.to_lowercase()); }
    }
    eprintln!("[multiwhale] tracked-гаманців у supply-токенах: {}", tracked.len());

    // П2: net[(token, addr)]  (tracked зі своїх рядків; нетрековані контрагенти — з протилежної сторони)
    let mut net: HashMap<(String, String), f64> = HashMap::new();
    for line in open().lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _c = it.next(); let dir = it.next().unwrap_or(""); let _ts = it.next();
        let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        let tl = tok.to_lowercase();
        if !sup.contains_key(&tl) { continue; }
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 { continue; }
        let s = if dir == "i" { amt } else { -amt };
        let wl = w.to_lowercase();
        if !is_junk(&wl) { *net.entry((tl.clone(), wl)).or_insert(0.0) += s; }
        if !tracked.contains(&cp) && !is_junk(&cp) { *net.entry((tl, cp)).or_insert(0.0) -= s; }
    }
    eprintln!("[multiwhale] (token,addr) позицій: {}", net.len());

    // addr -> Vec<(token_contract, symbol, pct)>
    let mut whale: HashMap<String, Vec<(String, String, f64)>> = HashMap::new();
    for ((t, a), n) in &net {
        if *n <= 0.0 { continue; }
        if priced_only && !real.contains(t) { continue; }
        let (hs, sy) = &sup[t];
        // Фаза 0.2: частка емісії — зі СЛОВНИКА (hs>0 гарантовано завантажувачем) → байт-паритет.
        let pct = wr_dict::emission_share(*n, *hs).min(1.0) * 100.0;
        if pct >= whale_pct && pct <= max_pct { whale.entry(a.clone()).or_default().push((t.clone(), sy.clone(), pct)); }
    }
    let whales: Vec<(String, Vec<(String, String, f64)>)> = whale.into_iter().filter(|(_, v)| v.len() >= min_coins).collect();
    eprintln!("[multiwhale] мульти-монетних китів (≥{whale_pct}% у ≥{min_coins} монет): {}", whales.len());

    // === КЛАСТЕРИЗАЦІЯ у ГРУПИ КАПІТАЛУ: кити, що ділять ≥MIN_SHARED_TOK токенів (спільний кошик) ===
    let min_shared: usize = std::env::var("MIN_SHARED_TOK").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let n = whales.len() as u32;
    let mut uf: Vec<u32> = (0..n).collect();
    // token -> індекси китів
    let mut tok2w: HashMap<&str, Vec<u32>> = HashMap::new();
    for (i, (_, coins)) in whales.iter().enumerate() { for (t, _, _) in coins { tok2w.entry(t.as_str()).or_default().push(i as u32); } }
    // пара китів, що ділять токен -> к-сть спільних токенів
    let mut pair: HashMap<(u32, u32), u16> = HashMap::new();
    for ws in tok2w.values() {
        for a in 0..ws.len() { for b in a + 1..ws.len() { *pair.entry((ws[a].min(ws[b]), ws[a].max(ws[b]))).or_insert(0) += 1; } }
    }
    for ((a, b), c) in &pair { if *c as usize >= min_shared { let (ra, rb) = (uf_find(&mut uf, *a), uf_find(&mut uf, *b)); if ra != rb { uf[ra as usize] = rb; } } }
    let mut comp: HashMap<u32, Vec<u32>> = HashMap::new();
    for i in 0..n { comp.entry(uf_find(&mut uf, i)).or_default().push(i); }
    let mut groups: Vec<Vec<u32>> = comp.into_values().collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()));

    println!("\n===== ГРУПИ КАПІТАЛУ (мульти-монетні кити, зкластеризовані за спільним кошиком) =====");
    let mut out = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        // спільні токени групи (у ≥2 членів, або всі якщо 1 член)
        let mut tcount: HashMap<&str, (u16, &str)> = HashMap::new();
        for &wi in g { for (t, s, _) in &whales[wi as usize].1 { let e = tcount.entry(t.as_str()).or_insert((0, s.as_str())); e.0 += 1; } }
        let thresh = if g.len() >= 2 { 2 } else { 1 };
        let mut shared: Vec<(&str, &str)> = tcount.iter().filter(|(_, (c, _))| *c as usize >= thresh).map(|(t, (_, s))| (*t, *s)).collect();
        shared.sort();
        let members: Vec<&String> = g.iter().map(|&wi| &whales[wi as usize].0).collect();
        println!("Група {}: {} гаманців | кошик({}): {}", gi + 1, g.len(), shared.len(),
            shared.iter().take(12).map(|(_, s)| *s).collect::<Vec<_>>().join(","));
        for &wi in g { let (a, coins) = &whales[wi as usize]; let mut c = coins.clone(); c.sort_by(|x, y| y.2.partial_cmp(&x.2).unwrap());
            println!("   {} {}  {}", tag(a), a, c.iter().take(8).map(|(_, s, p)| format!("{}:{:.0}%", s, p)).collect::<Vec<_>>().join(" ")); }
        out.push(json!({"id": gi + 1, "n_wallets": g.len(),
            "members": members, "shared_tokens": shared.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
            "shared_symbols": shared.iter().map(|(_, s)| *s).collect::<Vec<_>>()}));
    }
    File::create(format!("{root}/dataset/backfill/rgmultiwhale-groups.json"))
        .and_then(|mut f| f.write_all(serde_json::to_string_pretty(&json!({"n_groups": groups.len(), "groups": out})).unwrap().as_bytes())).ok();
    eprintln!("[multiwhale] груп капіталу: {} -> rgmultiwhale-groups.json", groups.len());
}
