//! rgsync (#3) — синхронність ВХОДУ. Для кожної CAP-групи: місяць першого входу кожного члена в її альт →
//! домінантна «хвиля входу» (пік-місяць + частка) + к-сть різних місяців. Навіть якщо альт набирається
//! роками, дом. хвиля = координований захід. Анотує групи entryWave/entryWaveShare + пише rgsync.json.
//! Env: —. Лише бекфіл.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn is_stable(sym: &str) -> bool { let s = sym.to_uppercase(); s.contains("USD") || s.contains("EUR") || s.contains("GBP") || matches!(s.as_str(), "DAI"|"FRAX"|"LUSD"|"TUSD"|"USTB"|"AUSD"|"GHO"|"MIM"|"WBTC"|"CBBTC"|"TBTC"|"WETH"|"WBETH"|"CBETH"|"WSTETH"|"STETH"|"RETH"|"WEETH"|"EETH") || s.ends_with("ON") }
fn ym_from_iso(ts: &str) -> Option<String> { if ts.len() < 7 { return None; } let (y, m) = (&ts[0..4], &ts[5..7]); if y.parse::<i32>().is_ok() && m.parse::<i32>().is_ok() { Some(format!("{y}-{m}")) } else { None } }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    // supply + priced -> sym2contract
    let mut supply: HashMap<String, String> = HashMap::new(); // contract -> symbol
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { supply.insert(c.to_lowercase(), v["symbol"].as_str().unwrap_or("").to_string()); }
    }
    let mut real: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())) { for k in v.keys() { real.insert(k.to_lowercase()); } }
    let mut sym2c: HashMap<String, String> = HashMap::new();
    for (c, s) in &supply { if real.contains(c) && !is_stable(s) { sym2c.entry(s.to_uppercase()).or_insert_with(|| c.clone()); } }

    // CAP-групи: gid -> (symbol, contract, members)
    let cgp = format!("{root}/dataset/capital-groups.json");
    let cg: Value = serde_json::from_reader(BufReader::new(File::open(&cgp).unwrap())).unwrap();
    let groups = cg["groups"].as_array().cloned().unwrap_or_default();
    let mut gi: Vec<usize> = Vec::new(); let mut gc: Vec<String> = Vec::new();
    let mut gmem: Vec<HashSet<String>> = Vec::new();
    let mut members: HashSet<String> = HashSet::new();
    for (i, g) in groups.iter().enumerate() {
        if !g["id"].as_str().unwrap_or("").starts_with("CAP:") { continue; }
        let sym = g["commonAssets"].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).unwrap_or("").to_uppercase();
        let Some(c) = sym2c.get(&sym) else { continue };
        gi.push(i); gc.push(c.clone());
        let mut ms = HashSet::new();
        for m in g["members"].as_array().unwrap_or(&vec![]) { if let Some(a) = m["address"].as_str() { let a = a.to_lowercase(); ms.insert(a.clone()); members.insert(a); } }
        gmem.push(ms);
    }
    eprintln!("[rgsync] CAP-груп: {}, унікальних членів: {}", gi.len(), members.len());

    // перший вхід (i) кожного члена у КОЖЕН priced-токен: firstentry[(wallet,token)] = min ts
    let mut fe: HashMap<(String, String), String> = HashMap::new();
    for line in BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).unwrap()).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or("");
        if d != "i" { continue; }
        let wl = w.to_lowercase(); if !members.contains(&wl) { continue; }
        let tl = tok.to_lowercase(); if !real.contains(&tl) { continue; }
        let e = fe.entry((wl, tl)).or_insert_with(|| String::from("~"));
        if ts < e.as_str() { *e = ts.to_string(); }
    }

    // на кожну групу: гістограма місяців першого входу членів у її альт
    let mut out: Vec<Value> = Vec::new();
    let mut annot: HashMap<usize, (String, f64, usize)> = HashMap::new();
    for k in 0..gi.len() {
        let c = &gc[k];
        let mut hist: HashMap<String, usize> = HashMap::new();
        let mut total = 0;
        for w in &gmem[k] { if let Some(ts) = fe.get(&(w.clone(), c.clone())) { if let Some(mo) = ym_from_iso(ts) { *hist.entry(mo).or_insert(0) += 1; total += 1; } } }
        if total < 5 { continue; }
        let (wave, wc) = hist.iter().max_by_key(|(_, c)| **c).map(|(m, c)| (m.clone(), *c)).unwrap();
        let share = wc as f64 / total as f64;
        let nmonths = hist.len();
        let sym = supply.get(c).cloned().unwrap_or_default();
        annot.insert(gi[k], (wave.clone(), share, nmonths));
        out.push(json!({"symbol": sym, "entries": total, "wave": wave, "waveShare": (share * 1e3).round() / 1e3, "distinctMonths": nmonths}));
    }
    out.sort_by(|a, b| b["waveShare"].as_f64().unwrap().partial_cmp(&a["waveShare"].as_f64().unwrap()).unwrap());
    eprintln!("[rgsync] груп з ≥5 датованими входами: {}", out.len());
    eprintln!("[rgsync] найкоординованіші хвилі входу (частка членів в один місяць):");
    for g in out.iter().take(20) { eprintln!("   {:<8} хвиля {} = {:.0}% входів ({} входів, {} різних міс)", g["symbol"].as_str().unwrap(), g["wave"].as_str().unwrap(), g["waveShare"].as_f64().unwrap() * 100.0, g["entries"].as_i64().unwrap(), g["distinctMonths"].as_i64().unwrap()); }

    // анотую групи
    let mut cg2: Value = serde_json::from_reader(BufReader::new(File::open(&cgp).unwrap())).unwrap();
    if let Ok(cur) = std::fs::read(&cgp) { File::create(format!("{cgp}.bak-sync")).and_then(|mut f| f.write_all(&cur)).ok(); }
    let mut n = 0;
    if let Some(arr) = cg2["groups"].as_array_mut() {
        for (ai, (wave, share, nm)) in &annot {
            if let Some(o) = arr[*ai].as_object_mut() { o.insert("entryWave".into(), json!(wave)); o.insert("entryWaveShare".into(), json!((share * 1e3).round() / 1e3)); o.insert("entryMonths".into(), json!(nm)); n += 1; }
        }
    }
    nextaction::safewrite::save_json_or_die("rgsync", &cgp, &cg2);
    nextaction::safewrite::save_json_or_die("rgsync", &format!("{bf}/rgsync.json"), &json!({"groups": out}));
    eprintln!("[rgsync] анотовано груп: {}", n);
}
