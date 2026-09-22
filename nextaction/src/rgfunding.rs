//! rgfunding — газ/перше фінансування набору гаманців АБО (GROUPS_FILE) зшивання ГРУП за СПІЛЬНИМ
//! ГАЗ-ФАНДЕРОМ (пеймастером). Режим WALLETS: перший вхідний ETH + спільні джерела. Режим GROUPS_FILE:
//! читає rgmultiwhale-groups.json, для кожного члена знаходить газ-фандера, і зливає групи, чиї члени
//! фінансовані тим самим пеймастером → мета-операції. Лише бекфіл. Env: WALLETS | GROUPS_FILE.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    era * 146097 + (yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + d - 1) - 719468
}
fn date(ts: &str) -> Option<(i32, String)> {
    let y: i64 = ts.get(0..4)?.parse().ok()?; let m: i64 = ts.get(5..7)?.parse().ok()?; let d: i64 = ts.get(8..10)?.parse().ok()?;
    Some((days_from_civil(y, m, d) as i32, format!("{:04}-{:02}-{:02}", y, m, d)))
}
fn is_junk(a: &str) -> bool { wr_store::addrs::is_junk_addr(a) }
fn uf_find(uf: &mut [u32], x: u32) -> u32 { let mut r = x; while uf[r as usize] != r { r = uf[r as usize]; } let mut c = x; while uf[c as usize] != c { let n = uf[c as usize]; uf[c as usize] = r; c = n; } r }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let name = |a: &str| lbl.get(a).and_then(|x| x["name"].as_str()).map(|s| s.to_string());
    let lab = |a: &str| match name(a) { Some(n) => format!("{} ({})", a, n), None => a.to_string() };

    // режим: GROUPS_FILE (зшивання груп) або WALLETS (один набір)
    let gfile = std::env::var("GROUPS_FILE").ok();
    let mut groups: Vec<(u64, Vec<String>)> = Vec::new();
    let mut m2g: HashMap<String, u32> = HashMap::new(); // member -> індекс групи
    let mut wallets: HashSet<String> = HashSet::new();
    if let Some(gf) = &gfile {
        let gv: Value = serde_json::from_reader(BufReader::new(File::open(gf).expect("groups"))).expect("parse");
        for (gi, g) in gv["groups"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let mem: Vec<String> = g["members"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
            for m in &mem { wallets.insert(m.clone()); m2g.entry(m.clone()).or_insert(gi as u32); }
            groups.push((g["id"].as_u64().unwrap_or(gi as u64 + 1), mem));
        }
        eprintln!("[rgfunding] груп: {}, членів: {}", groups.len(), wallets.len());
    } else {
        wallets = std::env::var("WALLETS").expect("WALLETS or GROUPS_FILE").split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    }

    // прохід transfers: перший вхідний ETH (газ-фандер) per член
    let mut eth: HashMap<String, (i32, String, String)> = HashMap::new(); // wallet -> (day, date, funder)
    let f = File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv");
    for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or(""); let _c = it.next(); let dir = it.next().unwrap_or(""); let ts = it.next().unwrap_or("");
        let _tok = it.next(); let rest = it.next().unwrap_or("");
        if dir != "i" { continue; }
        let wl = w.to_lowercase();
        if !wallets.contains(&wl) { continue; }
        if rest.split(',').next().unwrap_or("") != "ETH" { continue; }
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next(); let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0); let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 || is_junk(&cp) { continue; }
        let Some((day, ds)) = date(ts) else { continue };
        let e = eth.entry(wl).or_insert((i32::MAX, String::new(), String::new()));
        if day < e.0 { *e = (day, ds, cp); }
    }

    // funder -> члени
    let mut fund_mem: HashMap<String, Vec<String>> = HashMap::new();
    for (w, (_, _, cp)) in &eth { fund_mem.entry(cp.clone()).or_default().push(w.clone()); }

    if gfile.is_none() {
        // режим WALLETS: друк per-wallet + спільні джерела
        let mut ws: Vec<&String> = wallets.iter().collect(); ws.sort();
        for w in &ws { match eth.get(*w) { Some((_, d, cp)) => println!("{}  перший ГАЗ: {} від {}", w, d, lab(cp)), None => println!("{}  газ: —", w) } }
        println!("\n=== СПІЛЬНІ ДЖЕРЕЛА ГАЗУ (≥2 членів) ===");
        let mut sv: Vec<(&String, &Vec<String>)> = fund_mem.iter().filter(|(_, v)| v.len() >= 2).collect();
        sv.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        for (cp, m) in sv { println!("  {} → {} членів", lab(cp), m.len()); }
        return;
    }

    // === режим GROUPS_FILE: зшивання груп за спільним газ-фандером (пеймастером) ===
    let n = groups.len() as u32;
    let mut uf: Vec<u32> = (0..n).collect();
    let mut paymasters: Vec<(String, HashSet<u32>, usize)> = Vec::new(); // funder, групи, к-сть членів
    for (cp, mem) in &fund_mem {
        if mem.len() < 2 { continue; }
        let gset: HashSet<u32> = mem.iter().filter_map(|m| m2g.get(m).copied()).collect();
        // union усіх груп, що ділять цей пеймастер
        let gv: Vec<u32> = gset.iter().copied().collect();
        for i in 1..gv.len() { let (a, b) = (uf_find(&mut uf, gv[0]), uf_find(&mut uf, gv[i])); if a != b { uf[a as usize] = b; } }
        if gset.len() >= 1 { paymasters.push((cp.clone(), gset, mem.len())); }
    }
    // мета-групи
    let mut meta: HashMap<u32, Vec<u32>> = HashMap::new();
    for i in 0..n { meta.entry(uf_find(&mut uf, i)).or_default().push(i); }
    let mut metas: Vec<Vec<u32>> = meta.into_values().collect();
    metas.sort_by(|a, b| b.len().cmp(&a.len()));

    eprintln!("[rgfunding] пеймастерів (газ ≥2 членів): {}", paymasters.len());
    println!("\n===== МЕТА-ОПЕРАЦІЇ (групи зшиті за СПІЛЬНИМ ГАЗ-ПЕЙМАСТЕРОМ) =====");
    let mut out = Vec::new();
    for (mi, gs) in metas.iter().enumerate() {
        // пеймастери цієї мета-групи
        let gset: HashSet<u32> = gs.iter().copied().collect();
        let mut pms: Vec<(&String, usize)> = paymasters.iter().filter(|(_, g, _)| g.iter().any(|x| gset.contains(x))).map(|(c, _, n)| (c, *n)).collect();
        pms.sort_by(|a, b| b.1.cmp(&a.1)); pms.dedup_by(|a, b| a.0 == b.0);
        let members: Vec<&String> = gs.iter().flat_map(|&gi| groups[gi as usize].1.iter()).collect();
        if gs.len() >= 2 || !pms.is_empty() {
            println!("Мета-{}: {} під-груп, {} гаманців | пеймастери: {}", mi + 1, gs.len(), members.len(),
                pms.iter().take(3).map(|(c, k)| format!("{}(×{})", lab(c), k)).collect::<Vec<_>>().join(", "));
            for &gi in gs.iter().take(6) { println!("     під-група {}: {} гам", groups[gi as usize].0, groups[gi as usize].1.len()); }
        }
        out.push(json!({"id": mi + 1, "n_subgroups": gs.len(), "n_wallets": members.len(),
            "paymasters": pms.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            "members": members.iter().take(60).collect::<Vec<_>>()}));
    }
    File::create(format!("{root}/dataset/backfill/rgmeta-groups.json"))
        .and_then(|mut f| f.write_all(serde_json::to_string_pretty(&json!({"n_meta": metas.len(), "groups": out})).unwrap().as_bytes())).ok();
    eprintln!("[rgfunding] мета-операцій: {} -> rgmeta-groups.json", metas.len());
}
