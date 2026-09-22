//! cascbt — ЧЕСНИЙ БЕКТЕСТ стратегії Каскад: чи distributed% передбачає форвардну ціну?
//! Гіпотеза: мало роздано (кити тримають) → росте далі; багато роздано (≥47%) → розворот/падіння.
//! Метод: реконструюю net-криву китів У ЧАСІ; у кожен день d рахую distributed_до_d=(пік_до_d−net_d)/пік_до_d,
//! пару з форвардною дохідністю ret(d→d+H). Агрегую по бакетах distributed + AUC(distributed→падіння).
//! ЛИШЕ priced-токени (token-prices-hourly). Це РЕАЛЬНА цифра, не обіцянка.
//! Вхід: transfers.csv + group-membership + mm-wallets + token-prices-hourly. Args: ROOT. Env: HZ(7,30 днів).
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::fs::File;

/// днів від епохи для (y,m,d) — алгоритм Хіннанта
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
/// "2026-07-08T…" -> день від епохи
fn ts_day(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' { return None }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let m: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    Some(days_from_civil(y, m, d))
}

fn auc(mut s: Vec<(f64, bool)>) -> f64 {
    s.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (mut rs, mut i) = (0.0f64, 0usize);
    while i < s.len() {
        let mut j = i; while j + 1 < s.len() && s[j + 1].0 == s[i].0 { j += 1; }
        let ar = (i + j) as f64 / 2.0 + 1.0;
        for k in i..=j { if s[k].1 { rs += ar; } }
        i = j + 1;
    }
    let np = s.iter().filter(|x| x.1).count(); let nn = s.len() - np;
    if np == 0 || nn == 0 { return 0.5 }
    (rs - (np * (np + 1)) as f64 / 2.0) / (np * nn) as f64
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let horizons: Vec<i64> = std::env::var("HZ").unwrap_or_else(|_| "7,30".into())
        .split(',').filter_map(|s| s.trim().parse().ok()).collect();

    // кити
    let mut whale: HashSet<String> = HashSet::new();
    if let Ok(v) = std::fs::read_to_string(format!("{bf}/group-membership.json")).ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok()).ok_or(()) {
        if let Some(o) = v["wallets"].as_object() {
            for (a, x) in o { let g = x["group"].as_str().unwrap_or("");
                if !g.is_empty() && g != "INFRA" && g != "OTHER" { whale.insert(a.to_lowercase()); } }
        }
    }
    if let Ok(s) = std::fs::read_to_string(format!("{bf}/mm-wallets.txt")) {
        for l in s.lines() { whale.remove(&l.trim().to_lowercase()); }
    }

    // priced-токени: day -> price (останній за день)
    let mut price: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    if let Ok(v) = std::fs::read_to_string(format!("{bf}/token-prices-hourly.json")).ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok()).ok_or(()) {
        if let Some(tk) = v["tokens"].as_object() {
            for (c, x) in tk {
                let mut m = HashMap::new();
                for p in x["prices"].as_array().cloned().unwrap_or_default() {
                    if let Some(a) = p.as_array() { if let (Some(ts), Some(pr)) = (a[0].as_i64(), a[1].as_f64()) {
                        m.insert(ts / 86400, pr); // day -> price (перезапис = останній)
                    } }
                }
                if m.len() > 30 { price.insert(c.to_lowercase(), m); }
            }
        }
    }
    // + ОНЧЕЙН-ціни (GeckoTerminal) для токенів без DefiLlama-покриття (дрібняк)
    if let Ok(v) = std::fs::read_to_string(format!("{bf}/token-prices-onchain.json")).ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok()).ok_or(()) {
        if let Some(tk) = v["tokens"].as_object() {
            let mut added = 0;
            for (c, x) in tk {
                let c = c.to_lowercase();
                if price.contains_key(&c) { continue } // мажори лишаємо з DefiLlama
                let mut m = HashMap::new();
                for p in x["prices"].as_array().cloned().unwrap_or_default() {
                    if let Some(a) = p.as_array() { if let (Some(ts), Some(pr)) = (a[0].as_i64(), a[1].as_f64()) {
                        if pr > 0.0 { m.insert(ts / 86400, pr); } } }
                }
                if m.len() > 20 { price.insert(c, m); added += 1; }
            }
            eprintln!("[cascbt] + ончейн-цін (GeckoTerminal): {added}");
        }
    }
    let priced: HashSet<String> = price.keys().cloned().collect();
    eprintln!("[cascbt] китів {} · priced-токенів {}", whale.len(), priced.len());

    // події китів по priced-токенах: (day, signed)
    let mut ev: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
    let f = File::open(format!("{bf}/transfers.csv")).expect("transfers");
    let mut rows = 0u64;
    for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
        rows += 1;
        let mut it = line.splitn(9, ',');
        let w = it.next().unwrap_or(""); if !whale.contains(w) { continue }
        let _c = it.next(); let dir = it.next().unwrap_or("");
        let ts = it.next().unwrap_or(""); let tok = it.next().unwrap_or("").to_lowercase();
        if !priced.contains(&tok) { continue }
        let Some(day) = ts_day(ts) else { continue };
        let _s = it.next(); let _cp = it.next();
        let amt: f64 = it.next().unwrap_or("0").parse().unwrap_or(0.0);
        if amt == 0.0 { continue }
        ev.entry(tok).or_default().push((day, if dir == "i" { amt } else { -amt }));
    }
    eprintln!("[cascbt] transfers {rows}; токенів із подіями {}", ev.len());

    // по кожному горизонту: (distributed, fwd_ret)
    let mut data: Vec<Vec<(f64, f64)>> = horizons.iter().map(|_| vec![]).collect();
    for (tok, mut e) in ev {
        if e.len() < 5 { continue }
        e.sort_by_key(|x| x.0);
        let Some(pm) = price.get(&tok) else { continue };
        let mut days: Vec<i64> = pm.keys().copied().collect(); days.sort_unstable();
        // проходимо дні ціни; для кожного рахуємо net/peak за подіями ≤ day
        let (mut ei, mut cum, mut peak) = (0usize, 0.0f64, 0.0f64);
        for &d in &days {
            while ei < e.len() && e[ei].0 <= d { cum += e[ei].1; if cum > peak { peak = cum; } ei += 1; }
            if peak <= 0.0 { continue }
            let dist = ((peak - cum) / peak).clamp(0.0, 1.0) * 100.0;
            let p0 = pm[&d];
            for (hi, &h) in horizons.iter().enumerate() {
                // ціна найближча до d+h (у межах ±3 дні)
                if let Some(pf) = (0..=3).flat_map(|k| [d + h + k, d + h - k]).find_map(|dd| pm.get(&dd)) {
                    // winsorize: обрізаємо викиди неліквідних DEX-цін до [−90%,+200%] (глюки пулів)
                    if p0 > 0.0 { data[hi].push((dist, (pf / p0 - 1.0).clamp(-0.9, 2.0))); }
                }
            }
        }
    }

    println!("── БЕКТЕСТ КАСКАД: distributed% → форвардна ціна ──");
    println!("гіпотеза: <accum% (тримають)→росте; ≥47% (роздають)→падає. priced-токени, out-of-sample.\n");
    for (hi, &h) in horizons.iter().enumerate() {
        let d = &data[hi];
        if d.is_empty() { continue }
        let n = d.len();
        let med = |v: &[&(f64, f64)]| { let mut r: Vec<f64> = v.iter().map(|x| x.1).collect(); if r.is_empty() { return 0.0 } r.sort_by(|a, b| a.partial_cmp(b).unwrap()); r[r.len() / 2] };
        let lo: Vec<&(f64, f64)> = d.iter().filter(|(x, _)| *x < 12.0).collect();
        let mid: Vec<&(f64, f64)> = d.iter().filter(|(x, _)| *x >= 12.0 && *x < 47.0).collect();
        let hi_b: Vec<&(f64, f64)> = d.iter().filter(|(x, _)| *x >= 47.0).collect();
        let a_updown = auc(d.iter().map(|(x, r)| (-x, *r > 0.0)).collect());
        println!("+{h}дн (n={n}, ret winsorized [−90%,+200%]):");
        println!("   роздано <12%  → медіана ret {:>7.2}%  (n={})", med(&lo) * 100.0, lo.len());
        println!("   роздано 12-47%→ медіана ret {:>7.2}%  (n={})", med(&mid) * 100.0, mid.len());
        println!("   роздано ≥47%  → медіана ret {:>7.2}%  (n={})", med(&hi_b) * 100.0, hi_b.len());
        println!("   AUC(менше роздано → зростання): {a_updown:.3}  [0.5=нема едж, >0.55=гіпотеза працює]\n");
    }
    println!("ЧИТАТИ: якщо «<12%» дає вищий ret за «≥47%» — стратегія має основу. AUC>0.55 = distributed передбачає напрямок.");
    println!("ЗАСТЕРЕЖЕННЯ: лише priced-мажори (дрібняк-цілі ротації без цін); без комісій; минуле ≠ майбутнє.");
}
