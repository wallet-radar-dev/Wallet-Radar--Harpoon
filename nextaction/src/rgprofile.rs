//! rgprofile — рольовий+капітальний профіль УСІХ координованих груп (rolegroups-coordinated.json).
//! КАПІТАЛ = СТЕЙБЛ-ПОТІК (реальні гроші): stable_in / stable_out / net(=P&L) по членах групи
//! (стейбли за КОНТРАКТОМ, amount людський ≈$1). Плюс: активи, ролі ДИРИГЕНТ/PREPUMP/PUMP/MM/HOLDER
//! + ПРОКСІ (проміжні, junk off), і вторинно — піковий USD мемкойн-позицій (human×ціна зі свопів).
//! Лише бекфіл, без RPC. Args: ROOT. Env: GROUPS_FILE, TOPN.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

/// Стейбл-контракти — тег `rgprofile:stables` у базі.
fn stables_list() -> Vec<String> {
    let c = wr_store::open().expect("rgprofile: без сховища немає переліку стейблів");
    let mut q = c
        .prepare("SELECT address FROM wallet_tags WHERE tag='rgprofile:stables'")
        .expect("запит тегу");
    let v: Vec<String> =
        q.query_map([], |r| r.get(0)).expect("читання").filter_map(|r| r.ok()).collect();
    assert!(!v.is_empty(), "rgprofile: тег rgprofile:stables порожній");
    v
}



fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    era * 146097 + (yoe * 365 + yoe / 4 - yoe / 100 + (153 * mp + 2) / 5 + d - 1) - 719468
}
fn day_of(ts: &str) -> Option<i32> {
    Some(days_from_civil(ts.get(0..4)?.parse().ok()?, ts.get(5..7)?.parse().ok()?, ts.get(8..10)?.parse().ok()?) as i32)
}
fn is_junk(a: &str) -> bool {
    wr_store::addrs::is_junk_addr(a)
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let gfile = std::env::var("GROUPS_FILE").unwrap_or_else(|_| format!("{root}/dataset/backfill/rolegroups-coordinated.json"));
    let sl = stables_list();
    let stab: HashSet<&str> = sl.iter().map(String::as_str).collect();

    let gv: Value = serde_json::from_reader(BufReader::new(File::open(&gfile).expect("groups"))).expect("parse");
    let mut groups: Vec<(u64, Vec<String>)> = Vec::new();
    let mut w2g: HashMap<String, u64> = HashMap::new();
    for g in gv["groups"].as_array().cloned().unwrap_or_default() {
        let id = g["id"].as_u64().unwrap_or(0);
        let mem: Vec<String> = g["members"].as_array().or(g["top_cores"].as_array()).or(g["top_members"].as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
        for m in &mem { w2g.entry(m.clone()).or_insert(id); }
        groups.push((id, mem));
    }
    eprintln!("[rgprofile] груп: {}, seed: {}", groups.len(), w2g.len());

    // symbol + ЦІНА (human×price): price-файли + onchain-prices.json
    let mut price: HashMap<String, f64> = HashMap::new();
    let mut sym: HashMap<String, String> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { if let Some(s) = v["symbol"].as_str() { sym.entry(c.to_lowercase()).or_insert_with(|| s.to_string()); } }
    }
    if let Ok(rd) = std::fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            let Some(stem) = fname.strip_suffix(".json") else { continue };
            let Some((_c, tok)) = stem.split_once('_') else { continue };
            let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(match File::open(e.path()) { Ok(f) => f, Err(_) => continue })) else { continue };
            if let Some(s) = v["symbol"].as_str() { sym.entry(tok.to_lowercase()).or_insert_with(|| s.to_string()); }
            if let Some(last) = v["prices"].as_array().and_then(|a| a.last()) {
                let p = last["price"].as_f64().unwrap_or(0.0);
                if p > 0.0 { price.entry(tok.to_lowercase()).or_insert(p); }
            }
        }
    }
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/backfill/onchain-prices.json")).unwrap())) {
        for (t, o) in &v { if let Some(u) = o["usd"].as_f64() { price.entry(t.to_lowercase()).or_insert(u); } }
    }
    let sym_of = |t: &str| sym.get(t).cloned().unwrap_or_default();

    let mut mm: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/features.csv")) {
        for (i, l) in BufReader::new(f).lines().map_while(Result::ok).enumerate() {
            if i == 0 { continue; }
            let c: Vec<&str> = l.split(',').collect();
            if c.len() < 16 { continue; }
            if c[4].parse().unwrap_or(0.0) >= 0.4 && c[4].parse().unwrap_or(1.0) <= 0.6 && c[15].parse().unwrap_or(0.0) >= 5.0 && c[13].parse().unwrap_or(0.0) >= 30.0 { mm.insert(c[0].to_lowercase()); }
        }
    }
    let mut pump: HashMap<String, i32> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{root}/dataset/backfill/rolegroups-pumps.json")).unwrap())) {
        for p in v["pumps"].as_array().cloned().unwrap_or_default() {
            if let (Some(t), Some(w)) = (p["token"].as_str(), p["pump_week"].as_str()) { if let Some(d) = day_of(w) { pump.insert(t.to_lowercase(), d.div_euclid(7)); } }
        }
    }

    #[derive(Default)]
    struct Agg { pre_in: f64, out_pump: f64, first_in_wk: i32 }
    let mut agg: HashMap<(String, String), Agg> = HashMap::new();
    let mut events: HashMap<(String, String), Vec<(i32, f64)>> = HashMap::new();
    let mut nbr: HashMap<(u64, String), (bool, bool)> = HashMap::new();
    // КАПІТАЛ: стейбл-потік per group
    let mut s_in: HashMap<u64, f64> = HashMap::new();
    let mut s_out: HashMap<u64, f64> = HashMap::new();
    let f = File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv");
    for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _c = it.next();
        let dir = it.next().unwrap_or("");
        let ts = it.next().unwrap_or("");
        let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        let wl = w.to_lowercase();
        let Some(&gid) = w2g.get(&wl) else { continue };
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        let cp = rit.next().unwrap_or("").to_lowercase();
        if amt <= 0.0 { continue; }
        let tl = tok.to_lowercase();
        let Some(day) = day_of(ts) else { continue };
        // стейбл-потік (реальні гроші)
        if stab.contains(tl.as_str()) {
            if dir == "i" { *s_in.entry(gid).or_insert(0.0) += amt; } else { *s_out.entry(gid).or_insert(0.0) += amt; }
        }
        let week = day.div_euclid(7);
        let pw = pump.get(&tl).copied().unwrap_or(i32::MAX);
        let a = agg.entry((wl.clone(), tl.clone())).or_insert(Agg { first_in_wk: i32::MAX, ..Default::default() });
        if dir == "i" { if week < pw { a.pre_in += amt; } if week < a.first_in_wk { a.first_in_wk = week; } }
        else if week >= pw - 1 && week <= pw + 4 { a.out_pump += amt; }
        events.entry((wl.clone(), tl.clone())).or_default().push((day, if dir == "i" { amt } else { -amt }));
        if !w2g.contains_key(&cp) && !is_junk(&cp) {
            let e = nbr.entry((gid, cp)).or_insert((false, false));
            if dir == "o" { e.0 = true; } else { e.1 = true; }
        }
    }

    let mut peak: HashMap<(String, String), f64> = HashMap::new();
    for ((w, t), ev) in &mut events {
        ev.sort_by_key(|x| x.0);
        let (mut run, mut mx) = (0.0f64, 0.0f64);
        for (_d, s) in ev.iter() { run += s; if run > mx { mx = run; } }
        peak.insert((w.clone(), t.clone()), mx);
    }
    let role_of = |w: &str| -> (&'static str, f64) {
        let mut pre = 0.0; let mut ip = false; let mut iu = false;
        for ((sw, tk), a) in &agg {
            if sw != w || !pump.contains_key(tk) { continue; }
            pre += a.pre_in;
            let dumped = a.pre_in > 0.0 && a.out_pump >= 0.5 * a.pre_in && a.out_pump <= 3.0 * a.pre_in;
            if a.first_in_wk != i32::MAX && a.first_in_wk < pump[tk] { if dumped { ip = true; } } else { iu = true; }
        }
        (if mm.contains(w) { "MM" } else if ip { "PREPUMP" } else if iu { "PUMP" } else { "HOLDER" }, pre)
    };

    println!("\n===== ПРОФІЛЬ ГРУП: КАПІТАЛ = СТЕЙБЛ-ПОТІК (реальні гроші) =====");
    println!("{:>3} {:>5} {:>5} {:>6} {:>12} {:>12} {:>12} {:>10}  D/pp/pu/mm/h  топ-актив", "id", "гам", "прокс", "актив", "завів$", "вивів$", "net$(P&L)", "пік-ток$");
    let mut rows: Vec<(u64, usize, usize, usize, f64, f64, f64, f64, [u32; 5], String)> = Vec::new();
    for (gid, mem) in &groups {
        let memset: HashSet<&String> = mem.iter().collect();
        let mut census = [0u32; 5];
        let mut best: Option<(String, f64)> = None;
        let mut roles: HashMap<&String, &'static str> = HashMap::new();
        for w in mem {
            let (r, pre) = role_of(w);
            roles.insert(w, r);
            if best.as_ref().map(|(_, p)| pre > *p).unwrap_or(true) { best = Some((w.clone(), pre)); }
        }
        if let Some((lead, _)) = &best { roles.insert(mem.iter().find(|m| *m == lead).unwrap(), "ДИРИГЕНТ"); }
        for r in roles.values() { match *r { "ДИРИГЕНТ" => census[0] += 1, "PREPUMP" => census[1] += 1, "PUMP" => census[2] += 1, "MM" => census[3] += 1, _ => census[4] += 1 } }
        let proxies = nbr.iter().filter(|((g, _), (i, o))| g == gid && *i && *o).count();
        let mut assets: HashSet<&String> = HashSet::new();
        let mut peak_usd = 0.0;
        let mut top = String::new(); let mut topv = 0.0;
        for ((w, t), pk) in &peak {
            if !memset.contains(w) || *pk <= 0.0 || is_junk(t) || stab.contains(t.as_str()) { continue; }
            assets.insert(t);
            if let Some(pr) = price.get(t) { let v = pk * pr; peak_usd += v; if v > topv { topv = v; top = { let s = sym_of(t); if s.is_empty() { t[..10.min(t.len())].to_string() } else { s } }; } }
        }
        let si = s_in.get(gid).copied().unwrap_or(0.0);
        let so = s_out.get(gid).copied().unwrap_or(0.0);
        rows.push((*gid, mem.len() + proxies, proxies, assets.len(), si, so, si - so, peak_usd, census, top));
    }
    rows.sort_by(|a, b| (b.4 + b.5).partial_cmp(&(a.4 + a.5)).unwrap()); // за оборотом стейблів
    let topn: usize = std::env::var("TOPN").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    for (gid, tot, prox, na, si, so, net, pusd, c, top) in rows.iter().take(topn) {
        println!("{:>3} {:>5} {:>5} {:>6} {:>12.0} {:>12.0} {:>12.0} {:>10.0}  {}/{}/{}/{}/{:<3} {}", gid, tot, prox, na, si, so, net, pusd, c[0], c[1], c[2], c[3], c[4], top);
    }
    let (ti, to): (f64, f64) = (rows.iter().map(|r| r.4).sum(), rows.iter().map(|r| r.5).sum());
    println!("\nусього груп: {}; стейбл-оборот усіх: завів ${:.0} / вивів ${:.0} / net ${:.0}", rows.len(), ti, to, ti - to);
}
