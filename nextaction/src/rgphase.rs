//! rgphase — детекція фази накопичення→роздачі по кожному priced-альту.
//! Помісячна крива сукупного net-потоку НЕ-біржових холдерів (own-rows) → пік, поточний % від піку,
//! напрям останніх 3 міс → фаза (НАКОПИЧУЄ / ТРИМАЄ / ПОЧАВ РОЗДАВАТИ / РОЗДАЄ). Симетрія до
//! «продали BTC на піку» — але для альтів, і НАПЕРЕД. Дописує phase у capital-groups.json (CAP:*).
//! Env: MIN_HOLDERS(50) DROP_START(0.95) DROP_HARD(0.80). Лише бекфіл.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn is_stable(sym: &str) -> bool { let s = sym.to_uppercase(); s.contains("USD") || s.contains("EUR") || s.contains("GBP") || matches!(s.as_str(), "DAI"|"FRAX"|"LUSD"|"TUSD"|"USTB"|"AUSD"|"GHO"|"MIM"|"WBTC"|"CBBTC"|"TBTC"|"WETH"|"WBETH"|"CBETH"|"WSTETH"|"STETH"|"RETH"|"WEETH"|"EETH") || s.ends_with("ON") }
fn ym_from_iso(ts: &str) -> i32 { // "2026-07-08T..." -> індекс місяця, 0 = 2020-01
    if ts.len() < 7 { return i32::MIN; }
    let y: i32 = ts[0..4].parse().unwrap_or(0); let m: i32 = ts[5..7].parse().unwrap_or(0);
    if y < 2015 || m < 1 || m > 12 { return i32::MIN; }
    (y - 2020) * 12 + (m - 1)
}
fn ym_str(i: i32) -> String { let y = 2020 + i.div_euclid(12); let m = i.rem_euclid(12) + 1; format!("{y}-{m:02}") }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_holders: usize = std::env::var("MIN_HOLDERS").ok().and_then(|s| s.parse().ok()).unwrap_or(50);
    let drop_start: f64 = std::env::var("DROP_START").ok().and_then(|s| s.parse().ok()).unwrap_or(0.95);
    let drop_hard: f64 = std::env::var("DROP_HARD").ok().and_then(|s| s.parse().ok()).unwrap_or(0.80);
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
    let is_cex = |a: &str| matches!(kind.get(a).map(|s| s.as_str()), Some("exchange") | Some("otc")) || lbl.get(a).and_then(|x| x["name"].as_str()).map(|n| { let l = n.to_lowercase(); ["binance","crypto.com","okx","gate","coinbase","kraken","bybit","kucoin","bitget","circle","bridge","galaxy","falconx","wintermute","cumberland","tether"].iter().any(|c| l.contains(c)) }).unwrap_or(false);

    // помісячний net-потік не-біржових own-rows + к-сть унікальних холдерів на токен
    let mut mflow: HashMap<(String, i32), f64> = HashMap::new();
    let mut holders: HashMap<String, HashSet<String>> = HashMap::new();
    let tpath = format!("{bf}/transfers.csv");
    for line in BufReader::with_capacity(1 << 20, File::open(&tpath).expect("transfers")).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ','); let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or("");
        if w == "wallet" || (d != "i" && d != "o") { continue; }
        let tl = tok.to_lowercase(); if !priced_alt(&tl) { continue; }
        let wl = w.to_lowercase(); if is_cex(&wl) { continue; }
        let amt: f64 = line.rsplitn(4, ',').nth(1).unwrap_or("0").parse().unwrap_or(0.0); if amt <= 0.0 { continue; }
        let mi = ym_from_iso(ts); if mi == i32::MIN { continue; }
        let s = if d == "i" { amt } else { -amt };
        *mflow.entry((tl.clone(), mi)).or_insert(0.0) += s;
        holders.entry(tl).or_default().insert(wl);
    }

    // на кожен токен: кумулятивна крива → пік, поточний, фаза
    let mut out: Vec<Value> = Vec::new();
    for (t, (hsupply, sym)) in &supply {
        if !priced_alt(t) { continue; }
        let hc = holders.get(t).map(|h| h.len()).unwrap_or(0); if hc < min_holders { continue; }
        let mut months: Vec<(i32, f64)> = mflow.iter().filter(|((tk, _), _)| tk == t).map(|((_, m), v)| (*m, *v)).collect();
        if months.len() < 3 { continue; }
        months.sort_by_key(|x| x.0);
        // кумулятив
        let mut cum = 0.0; let mut curve: Vec<(i32, f64)> = Vec::new();
        for (m, v) in &months { cum += v; curve.push((*m, cum)); }
        let (peak_idx, peak_val) = curve.iter().cloned().fold((0i32, f64::MIN), |(pi, pv), (m, c)| if c > pv { (m, c) } else { (pi, pv) });
        if peak_val <= 0.0 { continue; }
        let (last_m, cur) = *curve.last().unwrap();
        let from_peak = cur / peak_val;
        // потік за останні 3 міс (діапазон [last_m-2, last_m])
        let recent: f64 = months.iter().filter(|(m, _)| *m > last_m - 3).map(|(_, v)| v).sum();
        let months_since_peak = last_m - peak_idx;
        let phase = if from_peak <= drop_hard { "РОЗДАЄ" }
            else if from_peak <= drop_start && recent < 0.0 { "ПОЧАВ РОЗДАВАТИ" }
            else if recent > 0.0 && from_peak >= 0.98 { "НАКОПИЧУЄ" }
            else { "ТРИМАЄ" };
        out.push(json!({
            "symbol": sym, "contract": t,
            "holders": hc,
            "peakMonth": ym_str(peak_idx),
            // Фаза 0.2: частка емісії — зі СЛОВНИКА (hsupply>0 гарант.) → байт-паритет.
            "curEmis": ((wr_dict::emission_share(cur, *hsupply)).max(0.0) * 1e4).round() / 1e4,
            "peakEmis": ((wr_dict::emission_share(peak_val, *hsupply)) * 1e4).round() / 1e4,
            "fromPeak": (from_peak * 1e3).round() / 1e3,
            "recent3moPctOfPeak": ((recent / peak_val) * 1e3).round() / 1e3,
            "monthsSincePeak": months_since_peak,
            "phase": phase,
        }));
    }
    // сорт: спершу хто РОЗДАЄ (найнижчий fromPeak)
    let ord = |p: &str| match p { "РОЗДАЄ" => 0, "ПОЧАВ РОЗДАВАТИ" => 1, "ТРИМАЄ" => 2, _ => 3 };
    out.sort_by(|a, b| ord(a["phase"].as_str().unwrap()).cmp(&ord(b["phase"].as_str().unwrap())).then(a["fromPeak"].as_f64().unwrap().partial_cmp(&b["fromPeak"].as_f64().unwrap()).unwrap()));

    let mut cnt: HashMap<&str, usize> = HashMap::new();
    for g in &out { *cnt.entry(g["phase"].as_str().unwrap()).or_insert(0) += 1; }
    eprintln!("[rgphase] альтів проаналізовано: {}", out.len());
    eprintln!("[rgphase] РОЗДАЄ: {} · ПОЧАВ РОЗДАВАТИ: {} · ТРИМАЄ: {} · НАКОПИЧУЄ: {}", cnt.get("РОЗДАЄ").unwrap_or(&0), cnt.get("ПОЧАВ РОЗДАВАТИ").unwrap_or(&0), cnt.get("ТРИМАЄ").unwrap_or(&0), cnt.get("НАКОПИЧУЄ").unwrap_or(&0));
    for g in out.iter().take(25) { eprintln!("   {:<8} {:<16} пік {} → зараз {:.0}% піку (спад {} міс), 3міс {:+.0}%", g["symbol"].as_str().unwrap(), g["phase"].as_str().unwrap(), g["peakMonth"].as_str().unwrap(), g["fromPeak"].as_f64().unwrap() * 100.0, g["monthsSincePeak"].as_i64().unwrap(), g["recent3moPctOfPeak"].as_f64().unwrap() * 100.0); }

    nextaction::safewrite::save_json_or_die("rgphase", &format!("{bf}/rgphase.json"), &json!({"assets": out}));
    // дописую phase у capital-groups.json (CAP:* за символом)
    let phase_by_sym: HashMap<String, Value> = out.iter().map(|g| (g["symbol"].as_str().unwrap().to_string(), g.clone())).collect();
    let cgp = format!("{root}/dataset/capital-groups.json");
    let mut cg: Value = serde_json::from_reader(BufReader::new(File::open(&cgp).unwrap())).unwrap();
    if let Ok(cur) = std::fs::read(&cgp) { File::create(format!("{cgp}.bak-phase")).and_then(|mut f| f.write_all(&cur)).ok(); }
    let mut annotated = 0;
    if let Some(arr) = cg["groups"].as_array_mut() {
        for g in arr.iter_mut() {
            let id = g["id"].as_str().unwrap_or("").to_string();
            if !id.starts_with("CAP:") { continue; }
            let sym = g["commonAssets"].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).unwrap_or("");
            if let Some(p) = phase_by_sym.get(sym) {
                // назву НЕ чіпаємо — маркер назви ставить rgflow (realPhase). Тут лише поля позиційної фази.
                if let Some(o) = g.as_object_mut() {
                    o.insert("posPhase".into(), p["phase"].clone());
                    o.insert("peakMonth".into(), p["peakMonth"].clone());
                    o.insert("fromPeak".into(), p["fromPeak"].clone());
                    annotated += 1;
                }
            }
        }
    }
    nextaction::safewrite::save_json_or_die("rgphase", &cgp, &cg);
    eprintln!("[rgphase] анотовано фазою груп CAP: {}", annotated);
}
