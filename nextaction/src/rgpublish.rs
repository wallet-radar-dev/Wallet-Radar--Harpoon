//! rgpublish — публікує знайдені групи капіталу в UI застосунку: конвертує rgmultiwhale-groups.json
//! у схему dataset/capital-groups.json (яку віддає /api/capital-groups passthrough) і ДОПИСУЄ мої групи
//! до наявних (бекап capital-groups.json.bak-publish). controlledEmission рахує з transfers (net членів /
//! supply). Лише бекфіл. Args: ROOT.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());

    // supply
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv {
            let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
            let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); }
        }
    }
    // реальні (priced) токени — лише їх у controlledEmission
    let mut real: HashSet<String> = HashSet::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/backfill/onchain-prices.json")).unwrap())) {
        for k in v.keys() { real.insert(k.to_lowercase()); }
    }
    // мої групи
    let gv: Value = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/backfill/rgmultiwhale-groups.json")).unwrap())).expect("rgmultiwhale-groups.json");
    let mygroups = gv["groups"].as_array().cloned().unwrap_or_default();
    // усі члени
    let mut all_members: HashSet<String> = HashSet::new();
    for g in &mygroups { for m in g["members"].as_array().unwrap_or(&vec![]) { if let Some(a) = m.as_str() { all_members.insert(a.to_lowercase()); } } }

    // net[(member, token)] з transfers (own rows + контрагентські для нетрекованих)
    let mut tracked: HashSet<String> = HashSet::new();
    let tpath = format!("{root}/dataset/backfill/transfers.csv");
    for line in BufReader::with_capacity(1 << 20, File::open(&tpath).expect("transfers")).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _c = it.next(); let dir = it.next().unwrap_or(""); let _ts = it.next(); let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        if supply.contains_key(&tok.to_lowercase()) { tracked.insert(w.to_lowercase()); }
    }
    let mut net: HashMap<(String, String), f64> = HashMap::new();
    for line in BufReader::with_capacity(1 << 20, File::open(&tpath).expect("transfers")).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _c = it.next(); let dir = it.next().unwrap_or(""); let _ts = it.next(); let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        let tl = tok.to_lowercase();
        if !supply.contains_key(&tl) { continue; }
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next(); let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0); let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 { continue; }
        let s = if dir == "i" { amt } else { -amt };
        let wl = w.to_lowercase();
        if all_members.contains(&wl) { *net.entry((wl, tl.clone())).or_insert(0.0) += s; }
        if all_members.contains(&cp) && !tracked.contains(&cp) { *net.entry((cp, tl)).or_insert(0.0) -= s; }
    }

    // формую групи у схемі UI
    let mut out_groups: Vec<Value> = Vec::new();
    for (gi, g) in mygroups.iter().enumerate() {
        let members: Vec<String> = g["members"].as_array().unwrap_or(&vec![]).iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect();
        if members.len() < 2 { continue; } // одиночний кит — не група
        let mset: HashSet<&String> = members.iter().collect();
        // controlledEmission = сума net членів / supply per токен
        let mut tok_hold: HashMap<String, f64> = HashMap::new();
        for ((w, t), n) in &net { if mset.contains(w) && *n > 0.0 { *tok_hold.entry(t.clone()).or_insert(0.0) += n; } }
        let mut emis: Vec<Value> = tok_hold.iter().filter_map(|(t, held)| {
            if !real.contains(t) { return None; } // лише реальні (свопні) токени — без скам
            let (hs, sym) = &supply[t];
            // Фаза 0.2: частка емісії — зі СЛОВНИКА (hs>0 гарант.) → байт-паритет.
            let share = wr_dict::emission_share(*held, *hs).min(1.0);
            if share >= 0.02 && share < 0.90 { Some(json!({"share": (share * 1e4).round() / 1e4, "symbol": sym})) } else { None }
        }).collect();
        emis.sort_by(|a, b| b["share"].as_f64().unwrap().partial_cmp(&a["share"].as_f64().unwrap()).unwrap());
        let common: Vec<String> = emis.iter().take(25).map(|e| e["symbol"].as_str().unwrap().to_string()).collect();
        let top3: Vec<String> = emis.iter().take(3).map(|e| format!("{} {:.0}%", e["symbol"].as_str().unwrap(), e["share"].as_f64().unwrap() * 100.0)).collect();
        // members = МАСИВ обʼєктів (як група A); cores/counterparties = ЧИСЛА
        let members_arr: Vec<Value> = members.iter().map(|a| json!({"address": a, "count": 1, "label": null, "role": "core"})).collect();
        out_groups.push(json!({
            "id": format!("W{}", gi + 1),
            "name": format!("Мульти-монетний десек #{}: {}", gi + 1, top3.join(", ")),
            "controlledEmission": emis,
            "commonAssets": common,
            "cores": members.len(),
            "counterparties": 0,
            "members": members_arr,
            "size": members.len(),
            "counterpartySource": "rgmultiwhale (net з обох сторін transfers, priced-only)",
            "expandedWith": "rgfunding (спільний газ-пеймастер)",
        }));
    }

    // мерджу з наявним capital-groups.json (старі лишаю)
    let cgp = format!("{root}/dataset/capital-groups.json");
    let mut existing: Value = serde_json::from_reader(BufReader::new(File::open(&cgp).unwrap())).unwrap_or(json!({"groups": []}));
    // бекап
    if let Ok(cur) = std::fs::read(&cgp) { File::create(format!("{cgp}.bak-publish")).and_then(|mut f| f.write_all(&cur)).ok(); }
    let mut groups = existing["groups"].as_array().cloned().unwrap_or_default();
    // прибрати попередні мої (W*) щоб не дублювати
    groups.retain(|g| !g["id"].as_str().map(|s| s.starts_with('W')).unwrap_or(false));
    let added = out_groups.len();
    groups.extend(out_groups);
    existing["groups"] = json!(groups);
    existing["generated"] = json!("2026-07-22 (rgpublish)");
    File::create(&cgp).and_then(|mut f| f.write_all(serde_json::to_string_pretty(&existing).unwrap().as_bytes())).ok();
    eprintln!("[rgpublish] додано {} моїх груп у capital-groups.json (усього {}); бекап .bak-publish", added, groups.len());
}
