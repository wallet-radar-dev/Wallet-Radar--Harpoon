//! rgverify — Rust-верифікатор координації (замінює python). Для набору ГАМАНЦІВ і ТОКЕНІВ
//! показує тиждень ПЕРШОГО входу кожного гаманця в кожен токен → видно, чи заходили синхронно.
//! Лише бекфіл (transfers.csv), без RPC. Args: ROOT. Env: WALLETS=comma, TOKENS=comma.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
fn week_of(ts: &str) -> Option<i32> {
    let y: i64 = ts.get(0..4)?.parse().ok()?;
    let m: i64 = ts.get(5..7)?.parse().ok()?;
    let d: i64 = ts.get(8..10)?.parse().ok()?;
    Some(days_from_civil(y, m, d).div_euclid(7) as i32)
}
fn wk_to_date(w: i32) -> String {
    let z = w as i64 * 7 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wallets: HashSet<String> = std::env::var("WALLETS").unwrap_or_default()
        .split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let tokens: HashSet<String> = std::env::var("TOKENS").unwrap_or_default()
        .split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    eprintln!("[rgverify] гаманців={} токенів={}", wallets.len(), tokens.len());

    // (wallet, token) -> (first_in_week, sum_in, sum_out)
    let mut fe: HashMap<(String, String), (i32, f64, f64)> = HashMap::new();
    let f = File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv");
    for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or("");
        let _chain = it.next();
        let dir = it.next().unwrap_or("");
        let ts = it.next().unwrap_or("");
        let tok = it.next().unwrap_or("");
        if w == "wallet" || (dir != "i" && dir != "o") { continue; }
        let wl = w.to_lowercase();
        if !wallets.contains(&wl) { continue; }
        let tl = tok.to_lowercase();
        if !tokens.contains(&tl) { continue; }
        let mut rit = line.rsplitn(4, ',');
        let _h = rit.next();
        let amt: f64 = rit.next().unwrap_or("0").parse().unwrap_or(0.0);
        if amt <= 0.0 { continue; }
        let Some(week) = week_of(ts) else { continue };
        let e = fe.entry((wl, tl)).or_insert((i32::MAX, 0.0, 0.0));
        if dir == "i" { e.1 += amt; if week < e.0 { e.0 = week; } } else { e.2 += amt; }
    }

    // матриця: рядок=гаманець, колонка=токен → тиждень першого входу
    let mut ws: Vec<&String> = wallets.iter().collect(); ws.sort();
    let mut ts: Vec<&String> = tokens.iter().collect(); ts.sort();
    println!("\nТИЖДЕНЬ ПЕРШОГО ВХОДУ (рядок=гаманець, колонка=токен):");
    print!("{:<44}", "wallet\\token");
    for t in &ts { print!(" {:<12}", &t[..12.min(t.len())]); }
    println!();
    for w in &ws {
        print!("{:<44}", w);
        for t in &ts {
            match fe.get(&((*w).clone(), (*t).clone())) {
                Some((wk, _, _)) if *wk != i32::MAX => print!(" {:<12}", wk_to_date(*wk)),
                _ => print!(" {:<12}", "-"),
            }
        }
        println!();
    }
    // синхронність: для кожного токена — чи входи в той самий тиждень
    println!("\nСИНХРОННІСТЬ по токенах (спред тижнів першого входу):");
    for t in &ts {
        let mut weeks: Vec<i32> = ws.iter().filter_map(|w| fe.get(&((*w).clone(), (*t).clone())).map(|x| x.0).filter(|x| *x != i32::MAX)).collect();
        if weeks.is_empty() { continue; }
        weeks.sort();
        let spread = weeks.last().unwrap() - weeks.first().unwrap();
        println!("  {:<12} входів={} спред={} тижнів  [{} .. {}]", &t[..12.min(t.len())], weeks.len(), spread, wk_to_date(weeks[0]), wk_to_date(*weeks.last().unwrap()));
    }
}
