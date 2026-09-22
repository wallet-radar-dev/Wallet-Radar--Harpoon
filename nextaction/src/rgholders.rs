//! rgholders — РЕАЛЬНА концентрація холдерів у монеті: реконструює net-баланси і tracked-гаманців,
//! І контрагентів (обидві сторони переказів) → ловить китів ПОЗА whitelist. Показує топ-холдерів
//! з % емісії + мітку (біржа/контракт/EOA). Лише бекфіл. Args: ROOT. Env: TOKENS=comma, LABELS.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn is_junk(a: &str) -> bool {
    wr_store::addrs::is_junk_addr(a)
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let tokens: HashSet<String> = std::env::var("TOKENS").expect("TOKENS").split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let labels: HashMap<String, String> = std::env::var("LABELS").unwrap_or_default().split(',')
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))).collect();

    // supply
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv {
            let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
            let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); }
        }
    }
    // мітки: labels-cache (імена) + wallet-kind (contract/exchange)
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/backfill/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let tag = |a: &str| -> String {
        if let Some(n) = lbl.get(a).and_then(|x| x["name"].as_str()) { return n.to_string(); }
        match kind.get(a).map(|s| s.as_str()) { Some("exchange") => "exchange".into(), Some("contract") => "contract".into(), Some("otc") => "otc".into(), Some("whale") => "whale-EOA".into(), _ => "EOA?".into() }
    };

    // збір рядків для цільових токенів
    let mut rows: HashMap<String, Vec<(String, f64, String)>> = HashMap::new(); // token -> (wallet, signed, cp)
    let f = File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv");
    for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _c = it.next();
        let dir = it.next().unwrap_or("");
        let _ts = it.next();
        let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        let tl = tok.to_lowercase();
        if !tokens.contains(&tl) { continue; }
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 { continue; }
        rows.entry(tl).or_default().push((w.to_lowercase(), if dir == "i" { amt } else { -amt }, cp));
    }

    for token in &tokens {
        let Some(rs) = rows.get(token) else { println!("\n{}: нема даних", token); continue };
        let sym = labels.get(token).cloned().unwrap_or_else(|| token[..10.min(token.len())].to_string());
        // tracked = гаманці з власними рядками
        let tracked: HashSet<&String> = rs.iter().map(|(w, _, _)| w).collect();
        // net: tracked зі своїх рядків; нетрековані контрагенти — з протилежної сторони
        let mut net: HashMap<String, f64> = HashMap::new();
        for (w, s, cp) in rs {
            *net.entry(w.clone()).or_insert(0.0) += s;
            if !tracked.contains(cp) && !is_junk(cp) { *net.entry(cp.clone()).or_insert(0.0) -= s; }
        }
        let (hs, ssym) = supply.get(token).cloned().unwrap_or((0.0, String::new()));
        let display = if sym.len() <= 8 { sym.clone() } else { ssym };
        // холдери (net>0), сорт
        let mut hold: Vec<(String, f64)> = net.into_iter().filter(|(a, n)| *n > 0.0 && !is_junk(a)).collect();
        hold.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let total: f64 = hold.iter().map(|x| x.1).sum();
        let top10: f64 = hold.iter().take(10).map(|x| x.1).sum();
        let conc = |v: f64| if hs > 0.0 { format!("{:.1}%", wr_dict::emission_share(v, hs).min(1.0) * 100.0) } else { "?".into() };
        println!("\n######### {} ({}) #########", display, &token[..12.min(token.len())]);
        println!("холдерів (tracked+контрагенти): {} | supply={:.3e}", hold.len(), hs);
        println!("🐋 РЕАЛЬНА концентрація: усі холдери={} емісії | топ-10={} | топ-1={}", conc(total), conc(top10), conc(hold.first().map(|x| x.1).unwrap_or(0.0)));
        println!("--- топ-15 холдерів (% емісії, мітка) ---");
        for (a, n) in hold.iter().take(15) {
            let t = tag(a);
            let is_tracked = if tracked.contains(a) { "" } else { " [поза-whitelist]" };
            println!("  {:>6}  {}  {}{}", conc(*n), a, t, is_tracked);
        }
    }
}
