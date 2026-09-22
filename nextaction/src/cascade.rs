//! cascade: по КОЖНОМУ активу (eth, з ціною+supply) визначає координований КАСКАД ролей
//! і частку емісії, яку каскад контролює. Ролі (per wallet×token, памп-місяць P=пік ціни):
//!   holder/диригент  — зайшов до P, net>0, не злив у памп (тримає емісію);
//!   prepump          — зайшов до P, злив у вікні [P-1,P+1];
//!   pump             — зайшов у P.. (наздоганяв);
//!   mm (маркетмейкер, wallet-level, перекриває) — in_ratio 0.4-0.6 + часта дво-стороння активність.
//! Гаманці НЕЗВ'ЯЗАНІ — група = роль-склад на активі. Лише whale-EOA (kind), контракти виключені.
//! Вихід: dataset/backfill/cascades.json (per-token) + wallet-roles census. Без RPC.
//! Env: MIN_RISE(2.0) PUMP_WIN(1) MM_INLO(0.4) MM_INHI(0.6) MM_FREQ(5) MM_DAYS(30) MIN_CASCADE(0.005).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

fn envf(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }
fn envi(k: &str, d: i64) -> i64 { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }

fn mi_ts(ts: &str) -> Option<i64> {
    let y: i64 = ts.get(0..4)?.parse().ok()?;
    let m: i64 = ts.get(5..7)?.parse().ok()?;
    Some(y * 12 + (m - 1))
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_rise = envf("MIN_RISE", 2.0);
    let win = envi("PUMP_WIN", 1);
    let mm_lo = envf("MM_INLO", 0.4);
    let mm_hi = envf("MM_INHI", 0.6);
    let mm_freq = envf("MM_FREQ", 5.0);
    let mm_days = envf("MM_DAYS", 30.0);
    let min_cascade = envf("MIN_CASCADE", 0.005);

    // 1) supply: contract -> (human_supply, symbol)
    let sv: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(
        File::open(format!("{root}/dataset/token-supply.json")).expect("supply"))).expect("parse");
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    for (c, v) in &sv {
        let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
        let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if raw <= 0.0 { continue; }
        supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string()));
    }

    // 2) памп-місяць P + rise з prices (лише eth)
    let mut pump: HashMap<String, (i64, f64)> = HashMap::new(); // contract -> (pump_mi, rise)
    if let Ok(rd) = fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            let Some(stem) = fname.strip_suffix(".json") else { continue };
            let Some((c, a)) = stem.split_once('_') else { continue };
            if c != "eth" { continue; }
            let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(File::open(e.path()).unwrap())) else { continue };
            let Some(arr) = v["prices"].as_array() else { continue };
            let mut mon: HashMap<i64, f64> = HashMap::new(); // month_idx -> max price
            for p in arr {
                let ts = p["timestamp"].as_i64().unwrap_or(0);
                let price = p["price"].as_f64().unwrap_or(0.0);
                let (yy, mm) = ymd(ts);
                let mi = yy as i64 * 12 + (mm as i64 - 1);
                let e = mon.entry(mi).or_insert(0.0);
                if price > *e { *e = price; }
            }
            if mon.len() < 3 { continue; }
            let peak = mon.iter().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap();
            let base = mon.values().cloned().fold(f64::INFINITY, f64::min);
            let rise = if base > 0.0 { *peak.1 / base } else { 0.0 };
            pump.insert(a.to_lowercase(), (*peak.0, rise));
        }
    }
    eprintln!("[cascade] токенів supply={} з памп-вікном={}", supply.len(), pump.len());

    // 3) whale-EOA
    let mut whales: HashMap<String, ()> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(
        File::open(format!("{root}/dataset/backfill/wallet-kind.json")).expect("kind"))) {
        for (a, k) in v { if k.as_str() == Some("whale") { whales.insert(a.to_lowercase(), ()); } }
    }
    eprintln!("[cascade] whale-EOA: {}", whales.len());

    // 4) MM-набір із features.csv (wallet-level)
    let mut mm: HashMap<String, ()> = HashMap::new();
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/features.csv")) {
        for (i, l) in BufReader::new(f).lines().map_while(Result::ok).enumerate() {
            if i == 0 { continue; }
            let c: Vec<&str> = l.split(',').collect();
            if c.len() < 16 { continue; }
            let w = c[0].to_lowercase();
            if !whales.contains_key(&w) { continue; }
            let in_ratio: f64 = c[4].parse().unwrap_or(0.0);
            let freq: f64 = c[15].parse().unwrap_or(0.0);
            let days: f64 = c[13].parse().unwrap_or(0.0);
            if in_ratio >= mm_lo && in_ratio <= mm_hi && freq >= mm_freq && days >= mm_days {
                mm.insert(w, ());
            }
        }
    }
    eprintln!("[cascade] маркетмейкерів (features): {}", mm.len());

    // 5) прохід transfers (eth, whale, token з памп-вікном): per (w,tok) агрегати
    #[derive(Default)]
    struct Pos { first_in: i64, sum_in: f64, sum_out: f64, out_pump: f64 }
    let mut pos: HashMap<(String, String), Pos> = HashMap::new();
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/transfers.csv")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let mut fr = line.split(',');
            let w = fr.next().unwrap_or("");
            let chain = fr.next().unwrap_or("");
            if chain != "eth" { continue; }
            let dir = fr.next().unwrap_or("");
            let ts = fr.next().unwrap_or("");
            let tok = fr.next().unwrap_or("");
            if dir != "i" && dir != "o" { continue; }
            let wl = w.to_lowercase();
            if !whales.contains_key(&wl) { continue; }
            let tl = tok.to_lowercase();
            let Some(&(pm, _rise)) = pump.get(&tl) else { continue };
            if !supply.contains_key(&tl) { continue; }
            let Some(mi) = mi_ts(ts) else { continue };
            let mut rr = line.rsplitn(3, ',');
            let _h = rr.next();
            let amt: f64 = rr.next().unwrap_or("0").parse().unwrap_or(0.0);
            if amt == 0.0 { continue; }
            let p = pos.entry((wl, tl)).or_insert(Pos { first_in: i64::MAX, ..Default::default() });
            if dir == "i" {
                p.sum_in += amt;
                if mi < p.first_in { p.first_in = mi; }
            } else {
                p.sum_out += amt;
                if (mi - pm).abs() <= win { p.out_pump += amt; }
            }
        }
    }
    eprintln!("[cascade] (whale,token) позицій: {}", pos.len());

    // 6) роль per (w,tok) + агрегація по токену
    #[derive(Default)]
    struct Tok {
        holder_net: f64, prepump_net: f64,
        holders: Vec<(String, f64)>, prepumps: Vec<(String, f64)>,
        n_holder: u32, n_prepump: u32, n_pump: u32, n_mm: u32,
    }
    let mut toks: HashMap<String, Tok> = HashMap::new();
    let mut role_census: HashMap<&str, u32> = HashMap::new();
    for ((w, tk), p) in &pos {
        let (pm, _rise) = pump[tk];
        let (hs, _sym) = &supply[tk];
        let net = (p.sum_in - p.sum_out).max(0.0);
        // Фаза 0.2: частка емісії — зі СЛОВНИКА (hs>0 гарантовано) → байт-паритет.
        let share = wr_dict::emission_share(net, *hs).min(1.0);
        let t = toks.entry(tk.clone()).or_default();
        let role = if mm.contains_key(w) {
            "mm"
        } else if p.first_in != i64::MAX && p.first_in < pm {
            // зайшов ДО пампу
            let dumped = p.sum_in > 0.0 && p.out_pump / p.sum_in >= 0.5;
            if net > 0.0 && !dumped { "holder" } else { "prepump" }
        } else {
            "pump"
        };
        *role_census.entry(role).or_insert(0) += 1;
        match role {
            "holder" => { t.holder_net += net; t.n_holder += 1; if share > 0.0 { t.holders.push((w.clone(), share)); } }
            "prepump" => { t.prepump_net += net; t.n_prepump += 1; if share > 0.0 { t.prepumps.push((w.clone(), share)); } }
            "pump" => { t.n_pump += 1; }
            _ => { t.n_mm += 1; }
        }
    }

    // 7) зібрати per-token каскади
    let mut rows: Vec<Value> = Vec::new();
    for (tk, t) in &toks {
        let (hs, sym) = supply[tk].clone();
        let (pm, rise) = pump[tk];
        if rise < min_rise { continue; }
        let cascade_share = wr_dict::emission_share(t.holder_net + t.prepump_net, hs).min(1.0);
        if cascade_share < min_cascade { continue; }
        let mut hs_top = t.holders.clone(); hs_top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let mut pp = t.prepumps.clone(); pp.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let holders_full: Vec<Value> = hs_top.iter().map(|(w, s)| json!({"w": w, "share": (s*1e6).round()/1e6})).collect();
        let prepumps_full: Vec<Value> = pp.iter().map(|(w, s)| json!({"w": w, "share": (s*1e6).round()/1e6})).collect();
        rows.push(json!({
            "symbol": sym, "contract": tk, "supply": hs,
            "pump_month": mi_str(pm), "rise": (rise*10.0).round()/10.0,
            "cascade_emission_share": (cascade_share*1e6).round()/1e6,
            "holder_share": ((t.holder_net/hs).min(1.0)*1e6).round()/1e6,
            "roles": {"holder": t.n_holder, "prepump": t.n_prepump, "pump": t.n_pump, "mm": t.n_mm},
            "holders": holders_full, "prepumps": prepumps_full
        }));
    }
    rows.sort_by(|a, b| b["cascade_emission_share"].as_f64().unwrap().partial_cmp(&a["cascade_emission_share"].as_f64().unwrap()).unwrap());
    fs::write(format!("{root}/dataset/backfill/cascades.json"),
        serde_json::to_string_pretty(&json!({
            "params": {"MIN_RISE": min_rise, "PUMP_WIN": win, "MM": {"in":[mm_lo,mm_hi],"freq":mm_freq,"days":mm_days}},
            "role_census": {"holder": role_census.get("holder").copied().unwrap_or(0),
                "prepump": role_census.get("prepump").copied().unwrap_or(0),
                "pump": role_census.get("pump").copied().unwrap_or(0),
                "mm": role_census.get("mm").copied().unwrap_or(0)},
            "cascades": rows
        })).unwrap()).ok();

    // 8) звіт
    let rc = |k: &str| role_census.get(k).copied().unwrap_or(0);
    eprintln!("\n=== РОЛЬОВИЙ ПЕРЕПИС (whale×token призначень) ===");
    eprintln!("  holder={} prepump={} pump={} mm={}", rc("holder"), rc("prepump"), rc("pump"), rc("mm"));
    eprintln!("\n=== ТОП-30 КАСКАДІВ за часткою емісії (holder+prepump / supply) ===");
    eprintln!("{:<10} {:>8} {:>8} {:>7} {:>4}/{:<4}/{:<4}/{:<3}  pump", "token", "casc%", "hold%", "rise", "H", "P", "pu", "mm");
    for r in rows.iter().take(30) {
        let ro = &r["roles"];
        eprintln!("{:<10} {:>7.2}% {:>7.2}% {:>6.1}x {:>4}/{:<4}/{:<4}/{:<3}  {}",
            r["symbol"].as_str().unwrap(),
            r["cascade_emission_share"].as_f64().unwrap()*100.0,
            r["holder_share"].as_f64().unwrap()*100.0,
            r["rise"].as_f64().unwrap(),
            ro["holder"].as_u64().unwrap(), ro["prepump"].as_u64().unwrap(),
            ro["pump"].as_u64().unwrap(), ro["mm"].as_u64().unwrap(),
            r["pump_month"].as_str().unwrap());
    }
    eprintln!("\nкаскадів (>{:.1}%): {}; вихід: dataset/backfill/cascades.json", min_cascade*100.0, rows.len());
}

// unix ts -> (year, month) UTC, без chrono
fn ymd(ts: i64) -> (i32, u32) {
    let days = ts.div_euclid(86400);
    let mut y = 1970i32;
    let mut d = days;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let dy = if leap { 366 } else { 365 };
        if d >= dy { d -= dy; y += 1; } else { break; }
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let ml = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0usize;
    while m < 12 && d >= ml[m] { d -= ml[m]; m += 1; }
    (y, (m + 1) as u32)
}

fn mi_str(mi: i64) -> String { format!("{:04}-{:02}", mi.div_euclid(12), mi.rem_euclid(12) + 1) }
