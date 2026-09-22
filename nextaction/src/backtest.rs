//! Крок D: WALK-FORWARD backtest контраріанної стратегії (без lookahead).
//!
//! Крок C показав: сильне USD-накопичення китами передує ПАДІННЮ ціни. Тут формалізуємо
//! у стратегію й чесно бектестимо БЕЗ підглядання майбутнього:
//!   - на кожен день t сигнал(t) = USD net-flow китів за W днів;
//!   - поріг = 90-й перцентиль сигналу ЛИШЕ по МИНУЛИХ днях (expanding, ≥MIN_HIST);
//!   - якщо сигнал(t) у топ-децилі минулого й >0 -> SELL-сигнал (контраріан);
//!   - short-return = -(price[t+H]/price[t]-1) (шорт заробляє на падінні).
//! Порівнюємо: середній short-return стратегії vs базовий short-return усіх днів (random entry),
//! hit-rate (частка сигналів, де ціна впала), к-сть угод. Це чесна оцінка торгової переваги.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

const DAY: i64 = 86_400;
const HORIZONS: [i64; 3] = [7, 14, 30];
const MIN_HIST: usize = 30; // мінімум минулих сигналів, щоб рахувати поріг

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::INFINITY;
    }
    let idx = ((sorted.len() as f64 * q) as usize).min(sorted.len() - 1);
    sorted[idx]
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let w: i64 = std::env::var("NA_W").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
    let prices_dir = format!("{root}/dataset/nextaction/prices");

    // Ціни: token -> day -> price
    let mut price_of: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let Ok(entries) = fs::read_dir(&prices_dir) else {
        eprintln!("[backtest] нема {prices_dir}");
        return;
    };
    for ent in entries.flatten() {
        let fname = ent.file_name().to_string_lossy().to_string();
        let Some(stem) = fname.strip_suffix(".json") else { continue };
        let Some((chain, addr)) = stem.split_once('_') else { continue };
        let Ok(txt) = fs::read_to_string(ent.path()) else { continue };
        let Ok(v): Result<Value, _> = serde_json::from_str(&txt) else { continue };
        let mut days: HashMap<i64, f64> = HashMap::new();
        if let Some(arr) = v["prices"].as_array() {
            for p in arr {
                let ts = p["timestamp"].as_i64().unwrap_or(0);
                let pr = p["price"].as_f64().unwrap_or(0.0);
                if ts > 0 && pr > 0.0 {
                    days.insert(ts / DAY, pr);
                }
            }
        }
        if days.len() >= 30 {
            price_of.insert(format!("{chain}\t{addr}"), days);
        }
    }
    eprintln!("[backtest] токенів з цінами: {}", price_of.len());

    // USD net-flow per (token, day)
    let mm: HashSet<String> = std::env::var("MM_EXCLUDE").ok().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
    eprintln!("[bin] MM-виключено китів: {}", mm.len());
    let f = BufReader::new(File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"));
    let mut flow_usd: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for line in f.lines() {
        let Ok(line) = line else { continue };
        let mut left = line.splitn(6, ',');
        let w = left.next().unwrap_or("");
        // Нульова адреса (mint/burn) — не кит; її потоки засмічували б USD net-flow.
        if w == wr_store::addrs::zero() { continue; }
        if mm.contains(w) { continue; }
        let chain = left.next().unwrap_or("");
        let dir = left.next().unwrap_or("?");
        let ts = left.next().unwrap_or("");
        let token = left.next().unwrap_or("");
        if dir != "i" && dir != "o" {
            continue;
        }
        let key = format!("{chain}\t{token}");
        let Some(pdays) = price_of.get(&key) else { continue };
        let amount: f64 = line.rsplit(',').nth(1).and_then(|a| a.parse().ok()).unwrap_or(0.0);
        if amount <= 0.0 {
            continue;
        }
        let day = match chrono::DateTime::parse_from_rfc3339(ts) {
            Ok(dt) => dt.timestamp() / DAY,
            Err(_) => continue,
        };
        if let Some(&pr) = pdays.get(&day) {
            let signed = if dir == "i" { amount } else { -amount };
            *flow_usd.entry(key).or_default().entry(day).or_insert(0.0) += signed * pr;
        }
    }

    // Ринковий forward-return: market[h][t] = середній (p1/p0-1) усіх токенів того дня.
    // Віднімання його робить стратегію market-neutral (прибирає спільний рух/режим).
    let mut market: HashMap<i64, HashMap<i64, (f64, u32)>> = HashMap::new();
    for pdays in price_of.values() {
        for (&t, &p0) in pdays.iter() {
            if p0 <= 0.0 {
                continue;
            }
            for &h in HORIZONS.iter() {
                if let Some(&p1) = pdays.get(&(t + h)) {
                    let e = market.entry(h).or_default().entry(t).or_insert((0.0, 0));
                    e.0 += p1 / p0 - 1.0;
                    e.1 += 1;
                }
            }
        }
    }
    let mkt = |h: i64, t: i64| -> Option<f64> {
        market.get(&h).and_then(|m| m.get(&t)).filter(|(_, c)| *c >= 5).map(|(s, c)| s / *c as f64)
    };

    println!("=== Крок D: walk-forward backtest КОНТРАРІАН-стратегії (SELL на топ-децил USD-накопичення) ===");
    println!("(short-return = виграш від падіння; поріг з МИНУЛОГО, без lookahead; W={w}д)\n");
    println!("{:>3} | {:>6} угод | strat short-ret сер={:>7} мед={:>7} hit={:>4} | база(усі дні) сер={:>7} | edge",
        "H", "", "", "", "", "");
    for &h in HORIZONS.iter() {
        let mut strat: Vec<f64> = Vec::new(); // short-returns за сигналами
        let mut base: Vec<f64> = Vec::new(); // short-returns усіх днів (random entry)
        for (key, pdays) in &price_of {
            let Some(fdays) = flow_usd.get(key) else { continue };
            let mut days: Vec<i64> = pdays.keys().copied().collect();
            days.sort_unstable();
            let mut past: Vec<f64> = Vec::new(); // минулі сигнали (для перцентиля)
            for &t in &days {
                // сигнал(t) = USD-накопичення за (t-w, t]
                let mut s = 0.0;
                for d in (t - w + 1)..=t {
                    if let Some(&fv) = fdays.get(&d) {
                        s += fv;
                    }
                }
                // forward short-return
                if let (Some(&p0), Some(&p1)) = (pdays.get(&t), pdays.get(&(t + h))) {
                    if p0 > 0.0 {
                        // winsorize до [-100%, +100%] — моделює стоп-лос і прибирає вплив
                        // екстремальних пампів дрібняку на mean (медіана/hit і так робастні).
                        let short_ret = (-(p1 / p0 - 1.0)).clamp(-1.0, 1.0);
                        base.push(short_ret);
                        // сигнал спрацював, якщо у топ-децилі МИНУЛОГО
                        if past.len() >= MIN_HIST {
                            let mut sorted = past.clone();
                            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            let thr = percentile(&sorted, 0.9);
                            if s >= thr && s > 0.0 {
                                strat.push(short_ret);
                            }
                        }
                    }
                }
                past.push(s);
            }
        }
        if strat.is_empty() {
            println!("H={h:>2}д: немає сигналів");
            continue;
        }
        let s_mean = mean(&strat);
        let s_med = median(strat.clone());
        let hit = strat.iter().filter(|&&v| v > 0.0).count() as f64 / strat.len() as f64;
        let b_mean = mean(&base);
        let edge = s_mean - b_mean;
        println!(
            "H={h:>2}д | {:>6} угод | strat short-ret сер={:+.2}% мед={:+.2}% hit={:.0}% | база сер={:+.2}% | edge={:+.2}%",
            strat.len(),
            s_mean * 100.0,
            s_med * 100.0,
            hit * 100.0,
            b_mean * 100.0,
            edge * 100.0
        );
    }
    // MARKET-NEUTRAL LONG-SHORT + decile-sweep: SHORT топ-децил накопичення, LONG bottom-децил
    // (кити РОЗПОДІЛЯЮТЬ). adj = token_ret - market_ret[t]. Пороги top-5/10/20%.
    println!("\n=== Market-neutral LONG-SHORT + decile-sweep (чистий edge, W={w}д) ===");
    println!("SHORT=накопичення→падіння; LONG=розподіл→ріст; LS=обидві ноги (комб edge)");
    let thresholds = [0.80f64, 0.90, 0.95];
    for &h in HORIZONS.iter() {
        let mut short_r: Vec<Vec<f64>> = vec![Vec::new(); thresholds.len()];
        let mut long_r: Vec<Vec<f64>> = vec![Vec::new(); thresholds.len()];
        for (key, pdays) in &price_of {
            let Some(fdays) = flow_usd.get(key) else { continue };
            let mut days: Vec<i64> = pdays.keys().copied().collect();
            days.sort_unstable();
            let mut past: Vec<f64> = Vec::new();
            for &t in &days {
                let mut s = 0.0;
                for d in (t - w + 1)..=t {
                    if let Some(&fv) = fdays.get(&d) {
                        s += fv;
                    }
                }
                if let (Some(&p0), Some(&p1)) = (pdays.get(&t), pdays.get(&(t + h))) {
                    if p0 > 0.0 {
                        if let Some(m) = mkt(h, t) {
                            let radj = (p1 / p0 - 1.0) - m;
                            if past.len() >= MIN_HIST {
                                let mut sorted = past.clone();
                                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                                for (qi, &q) in thresholds.iter().enumerate() {
                                    let hi = percentile(&sorted, q);
                                    let lo = percentile(&sorted, 1.0 - q);
                                    if s >= hi && s > 0.0 {
                                        short_r[qi].push((-radj).clamp(-1.0, 1.0));
                                    }
                                    if s <= lo && s < 0.0 {
                                        long_r[qi].push(radj.clamp(-1.0, 1.0));
                                    }
                                }
                            }
                        }
                    }
                }
                past.push(s);
            }
        }
        for (qi, &q) in thresholds.iter().enumerate() {
            let s = &short_r[qi];
            let l = &long_r[qi];
            let sm = mean(s);
            let sh = if s.is_empty() { 0.0 } else { s.iter().filter(|&&v| v > 0.0).count() as f64 / s.len() as f64 };
            let lm = mean(l);
            let lh = if l.is_empty() { 0.0 } else { l.iter().filter(|&&v| v > 0.0).count() as f64 / l.len() as f64 };
            let comb = (sm * s.len() as f64 + lm * l.len() as f64) / (s.len() + l.len()).max(1) as f64;
            println!(
                "H={h:>2}д top{:>2}% | SHORT n={:>5} edge={:+.2}% hit={:.0}% | LONG n={:>5} edge={:+.2}% hit={:.0}% | LS={:+.2}%",
                ((1.0 - q) * 100.0) as i32, s.len(), sm * 100.0, sh * 100.0, l.len(), lm * 100.0, lh * 100.0, comb * 100.0
            );
        }
    }

    eprintln!("\n[backtest] готово. Market-neutral long-short: чистий edge сигналу без режиму.");
}
