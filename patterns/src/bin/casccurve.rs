//! casccurve — КРИВА ЧИСТОЇ ПОЗИЦІЇ китів у ЧАСІ (фундамент стратегії Каскад).
//! Проблема totals: distributed=out/in не відрізняє РОЗДАЧУ від CHURN. Рішення — реконструкція
//! net-позиції групи в часі: набрали до ПІКУ, далі роздають. distributed% = (пік−поточне)/пік.
//! Юніти amount скорочуються (це відношення) → надійно без знання масштабу.
//! Кити = члени капітал-груп (group-membership.json) МІНУС INFRA і ММ-churn (mm-wallets.txt).
//! Токени — лише ті, де відома supply (tokenpnl) — щоб обмежити обсяг.
//! Вхід: transfers.csv (wallet,chain,dir,ts,token,symbol,cp,amount,hash) + group-membership + mm-wallets + tokenpnl.
//! Вихід: dataset/backfill/cascade-curves.json. Args: ROOT.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::fs::File;

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");

    // кити: group-membership мінус INFRA; + мітка групи
    let mut whale: HashMap<String, String> = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(format!("{bf}/group-membership.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let Some(o) = v["wallets"].as_object() {
                for (a, x) in o {
                    let g = x["group"].as_str().unwrap_or("");
                    if !g.is_empty() && g != "INFRA" && g != "OTHER" { whale.insert(a.to_lowercase(), g.to_string()); }
                }
            }
        }
    }
    // виключити ММ-churn
    if let Ok(s) = std::fs::read_to_string(format!("{bf}/mm-wallets.txt")) {
        for l in s.lines() { whale.remove(&l.trim().to_lowercase()); }
    }
    eprintln!("[casccurve] китів (не-INFRA, не-ММ): {}", whale.len());

    // токени з відомою supply + symbol/price (з tokenpnl)
    let mut supply: HashMap<String, (f64, String, f64)> = HashMap::new(); // token -> (supply, sym, priceNow)
    if let Ok(f) = File::open(format!("{bf}/tokenpnl.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(d) = serde_json::from_str::<Value>(&line) {
                let t = d["contract"].as_str().or_else(|| d["token"].as_str()).unwrap_or("").to_lowercase();
                let s = d["supply"].as_f64().unwrap_or(0.0);
                if !t.is_empty() && s > 0.0 {
                    supply.insert(t, (s, d["symbol"].as_str().unwrap_or("").to_string(), d["priceNow"].as_f64().unwrap_or(0.0)));
                }
            }
        }
    }
    let toks: HashSet<String> = supply.keys().cloned().collect();
    eprintln!("[casccurve] токенів із supply: {}", toks.len());

    // стрім transfers: збираємо (ts, signed) по токену для китів
    let mut ev: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    let f = File::open(format!("{bf}/transfers.csv")).expect("нема transfers.csv");
    let mut rows: u64 = 0;
    for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
        rows += 1;
        if rows % 3_000_000 == 0 { eprintln!("[casccurve] {rows} рядків…"); }
        let mut it = line.splitn(9, ',');
        let w = it.next().unwrap_or("");
        if !whale.contains_key(w) { continue }
        let _chain = it.next();
        let dir = it.next().unwrap_or("");
        let ts = it.next().unwrap_or("").to_string();
        let tok = it.next().unwrap_or("").to_lowercase();
        if !toks.contains(&tok) { continue }
        let _sym = it.next(); let _cp = it.next();
        let amt: f64 = it.next().unwrap_or("0").parse().unwrap_or(0.0);
        if amt == 0.0 { continue }
        let signed = if dir == "i" { amt } else { -amt };
        ev.entry(tok).or_default().push((ts, signed));
    }
    eprintln!("[casccurve] оброблено {rows} рядків; токенів із подіями: {}", ev.len());

    // по токену: пік net, поточне, distributed%
    let mut out: Vec<Value> = Vec::new();
    for (tok, mut e) in ev {
        if e.len() < 5 { continue }
        e.sort_by(|a, b| a.0.cmp(&b.0)); // ISO ts сортується лексично
        let (mut cum, mut peak, mut peak_ts) = (0.0f64, 0.0f64, String::new());
        for (ts, s) in &e { cum += s; if cum > peak { peak = cum; peak_ts = ts.clone(); } }
        let current = cum;
        if peak <= 0.0 { continue }
        let distributed = ((peak - current) / peak * 100.0).clamp(0.0, 100.0);
        let (sup, sym, price) = supply.get(&tok).cloned().unwrap_or((0.0, String::new(), 0.0));
        // emission% від піку (пік_net/supply) — орієнтовно, юніти amount можуть різнитись від supply → познач suspect
        let peak_emis = if sup > 0.0 { 100.0 * peak / sup } else { -1.0 };
        out.push(json!({
            "token": tok, "symbol": sym, "priceNow": price, "supply": sup,
            "peakNet": peak, "currentNet": current, "distributedPct": (distributed*100.0).round()/100.0,
            "peakEmisPct": (peak_emis*100.0).round()/100.0, "peakDate": &peak_ts[..10.min(peak_ts.len())],
            "lastDate": &e.last().unwrap().0[..10.min(e.last().unwrap().0.len())], "nEvents": e.len(),
        }));
    }
    out.sort_by(|a, b| b["distributedPct"].as_f64().unwrap().partial_cmp(&a["distributedPct"].as_f64().unwrap()).unwrap());
    let n = out.len();
    std::fs::write(format!("{bf}/cascade-curves.json"),
        serde_json::to_string(&json!({"generated_rows": rows, "tokens": out})).unwrap()).ok();
    eprintln!("[casccurve] збережено cascade-curves.json ({n} токенів)");
}
