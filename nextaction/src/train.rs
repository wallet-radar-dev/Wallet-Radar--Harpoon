//! Крок E: НАВЧАННЯ моделі whale-flow -> «недоперформить ринок» (SELL-сигнал).
//!
//! Пілот довів контраріан. Тут ТРЕНУЄМО логістичну регресію передбачати, чи токен
//! недоперформить ринок за H днів, за фічами накопичення китами — чесно:
//!   - фічі БЕЗ lookahead (перцентиль-ранг сигналу серед МИНУЛИХ днів + momentum);
//!   - ТЕМПОРАЛЬНИЙ train/test спліт (навчаємось на ранніх днях, тестуємо на пізніших);
//!   - оцінка: test AUC проти null (перемішані мітки) + baseline.
//! Фічі: usd_rank, tok_rank, mom (price[t]/price[t-W]-1). Мітка y=1 якщо
//! market-adjusted forward-return < 0 (токен програв ринку -> шорт/avoid був би правий).

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

const DAY: i64 = 86_400;
const W: i64 = 7;
const H: i64 = 14; // горизонт мітки
const MIN_HIST: usize = 30;
const EPOCHS: usize = 500;
const LR: f64 = 0.3;
const L2: f64 = 1e-3;
const NF: usize = 7; // usd_rank, tok_rank, mom, log_mag, supply_rank, recent_rank, accel_rank

struct Sample {
    day: i64,
    x: [f64; NF],
    y: u8,
}

fn percentile_rank(sorted: &[f64], v: f64) -> f64 {
    // частка минулих значень < v
    if sorted.is_empty() {
        return 0.5;
    }
    let mut lo = 0usize;
    let mut hi = sorted.len();
    while lo < hi {
        let m = (lo + hi) / 2;
        if sorted[m] < v {
            lo = m + 1;
        } else {
            hi = m;
        }
    }
    lo as f64 / sorted.len() as f64
}

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

fn auc(pairs: &[(f64, u8)]) -> f64 {
    // Mann-Whitney U / (n_pos*n_neg)
    let mut v: Vec<&(f64, u8)> = pairs.iter().collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (mut rank, mut sum_pos_ranks, mut npos, mut i) = (0.0f64, 0.0f64, 0u64, 0usize);
    while i < v.len() {
        let mut j = i;
        while j < v.len() && v[j].0 == v[i].0 {
            j += 1;
        }
        let avg_rank = (i + j - 1) as f64 / 2.0 + 1.0; // 1-based середній ранг тай-групи
        for k in i..j {
            if v[k].1 == 1 {
                sum_pos_ranks += avg_rank;
                npos += 1;
            }
        }
        rank += (j - i) as f64;
        i = j;
    }
    let _ = rank;
    let nneg = v.len() as u64 - npos;
    if npos == 0 || nneg == 0 {
        return 0.5;
    }
    let u = sum_pos_ranks - (npos as f64 * (npos as f64 + 1.0)) / 2.0;
    u / (npos as f64 * nneg as f64)
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let prices_dir = format!("{root}/dataset/nextaction/prices");

    // ціни
    let mut price_of: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let Ok(entries) = fs::read_dir(&prices_dir) else {
        eprintln!("[train] нема {prices_dir}");
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
    eprintln!("[train] токенів з цінами: {}", price_of.len());

    // supply.tsv: token -> supply_tokens (для нормалізації накопичення на емісію)
    let mut supply: HashMap<String, f64> = HashMap::new();
    if let Ok(txt) = fs::read_to_string(format!("{root}/dataset/nextaction/supply.tsv")) {
        for line in txt.lines().skip(1) {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() >= 3 {
                if let Ok(s) = c[2].parse::<f64>() {
                    if s > 0.0 {
                        supply.insert(format!("{}\t{}", c[0], c[1]), s);
                    }
                }
            }
        }
    }
    eprintln!("[train] supply відомий для {} токенів", supply.len());

    // виключення: MM-кити (env MM_EXCLUDE) + нульова адреса
    let mut excl: HashSet<String> = std::env::var("MM_EXCLUDE").ok().and_then(|p| fs::read_to_string(p).ok()).map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
    excl.insert(wr_store::addrs::zero().to_string());
    eprintln!("[train] виключено китів (MM+null): {}", excl.len());
    // flow (token) + flow_usd
    let f = BufReader::new(File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"));
    let mut flow: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let mut flow_usd: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for line in f.lines() {
        let Ok(line) = line else { continue };
        let mut left = line.splitn(6, ',');
        let w = left.next().unwrap_or("");
        if excl.contains(w) { continue; }
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
        let signed = if dir == "i" { amount } else { -amount };
        *flow.entry(key.clone()).or_default().entry(day).or_insert(0.0) += signed;
        if let Some(&pr) = pdays.get(&day) {
            *flow_usd.entry(key).or_default().entry(day).or_insert(0.0) += signed * pr;
        }
    }

    let h: i64 = std::env::var("H").ok().and_then(|s| s.parse().ok()).unwrap_or(H);
    let target = std::env::var("TARGET").unwrap_or_else(|_| "underperf".to_string());
    let thr: f64 = std::env::var("THR").ok().and_then(|s| s.parse().ok()).unwrap_or(0.10);
    eprintln!("[train] TARGET={target} H={h} THR={thr}");
    // market forward-return для мітки: market[day] = середній (p1/p0-1) за H того дня
    let mut market: HashMap<i64, (f64, u32)> = HashMap::new();
    for pdays in price_of.values() {
        for (&t, &p0) in pdays.iter() {
            if p0 <= 0.0 {
                continue;
            }
            if let Some(&p1) = pdays.get(&(t + h)) {
                let e = market.entry(t).or_insert((0.0, 0));
                e.0 += p1 / p0 - 1.0;
                e.1 += 1;
            }
        }
    }
    let mkt = |t: i64| -> Option<f64> { market.get(&t).filter(|(_, c)| *c >= 5).map(|(s, c)| s / *c as f64) };

    // Будуємо семпли: фічі (no lookahead) + мітка.
    let mut samples: Vec<Sample> = Vec::new();
    for (key, pdays) in &price_of {
        let (Some(fu), Some(ft)) = (flow_usd.get(key), flow.get(key)) else { continue };
        let mut days: Vec<i64> = pdays.keys().copied().collect();
        days.sort_unstable();
        let sup = supply.get(key).copied();
        let mut past_usd: Vec<f64> = Vec::new();
        let mut past_tok: Vec<f64> = Vec::new();
        let mut past_sup: Vec<f64> = Vec::new();
        let mut past_r2: Vec<f64> = Vec::new();
        let mut past_accel: Vec<f64> = Vec::new();
        for &t in &days {
            let usd_acc: f64 = (t - W + 1..=t).filter_map(|d| fu.get(&d)).sum();
            let tok_acc: f64 = (t - W + 1..=t).filter_map(|d| ft.get(&d)).sum();
            let r2: f64 = (t - 1..=t).filter_map(|d| fu.get(&d)).sum();
            let r2_prev: f64 = (t - 3..=t - 2).filter_map(|d| fu.get(&d)).sum();
            let accel = r2 - r2_prev;
            let sup_norm = sup.map(|s| tok_acc / s).unwrap_or(0.0); // частка емісії за W
            // мітка з market-adjusted forward-return
            if let (Some(&p0), Some(&p1), Some(&pw)) = (pdays.get(&t), pdays.get(&(t + h)), pdays.get(&(t - W))) {
                if p0 > 0.0 && pw > 0.0 && past_usd.len() >= MIN_HIST {
                    if let Some(m) = mkt(t) {
                        let mut su = past_usd.clone();
                        su.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let mut st = past_tok.clone();
                        st.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let usd_rank = percentile_rank(&su, usd_acc);
                        let tok_rank = percentile_rank(&st, tok_acc);
                        let mom = (p0 / pw - 1.0).clamp(-1.0, 3.0);
                        let log_mag = (usd_acc.max(0.0) + 1.0).ln();
                        let supply_rank = if sup.is_some() {
                            let mut ss = past_sup.clone();
                            ss.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            percentile_rank(&ss, sup_norm)
                        } else {
                            0.5
                        };
                        let mut sr = past_r2.clone();
                        sr.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let recent_rank = percentile_rank(&sr, r2);
                        let mut sa = past_accel.clone();
                        sa.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let accel_rank = percentile_rank(&sa, accel);
                        let raw = p1 / p0 - 1.0;
                        let adj = raw - m;
                        let y = match target.as_str() {
                            "bigdrop" => if raw < -thr { 1u8 } else { 0 },
                            "bigdrop_adj" => if adj < -thr { 1u8 } else { 0 },
                            "extreme" => if adj.abs() > thr { 1u8 } else { 0 },
                            "bigup" => if raw > thr { 1u8 } else { 0 },
                            _ => if adj < 0.0 { 1u8 } else { 0 },
                        };
                        samples.push(Sample { day: t, x: [usd_rank, tok_rank, mom, log_mag, supply_rank, recent_rank, accel_rank], y });
                    }
                }
            }
            past_usd.push(usd_acc);
            past_tok.push(tok_acc);
            past_sup.push(sup_norm);
            past_r2.push(r2);
            past_accel.push(accel);
        }
    }
    eprintln!("[train] семплів: {}", samples.len());
    // HIGH_SIGNAL=1 -> лише топ-децил накопичення (usd_rank>=0.9): чи є прогноз СЕРЕД подій?
    let high_signal = std::env::var("HIGH_SIGNAL").is_ok();
    if high_signal {
        samples.retain(|s| s.x[0] >= 0.9);
        eprintln!("[train] HIGH_SIGNAL: топ-децил USD, семплів {}", samples.len());
    }
    let supply_signal = std::env::var("SUPPLY_SIGNAL").is_ok();
    if supply_signal {
        samples.retain(|s| s.x[4] >= 0.9); // топ-децил по частці емісії
        eprintln!("[train] SUPPLY_SIGNAL: топ-децил емісії, семплів {}", samples.len());
    }
    if samples.len() < 500 {
        eprintln!("[train] замало семплів");
        return;
    }

    // Темпоральний спліт: сортуємо за днем, 70% train / 30% test.
    samples.sort_by_key(|s| s.day);
    let split = samples.len() * 70 / 100;
    let cut_day = samples[split].day;
    let (train, test): (Vec<&Sample>, Vec<&Sample>) = samples.iter().partition(|s| s.day < cut_day);
    eprintln!("[train] train={} test={} (межа день {cut_day})", train.len(), test.len());

    // Стандартизація фіч по TRAIN.
    let mut mean = [0.0; NF];
    let mut std = [0.0; NF];
    for s in &train {
        for j in 0..NF {
            mean[j] += s.x[j];
        }
    }
    for j in 0..NF {
        mean[j] /= train.len() as f64;
    }
    for s in &train {
        for j in 0..NF {
            std[j] += (s.x[j] - mean[j]).powi(2);
        }
    }
    for j in 0..NF {
        std[j] = (std[j] / train.len() as f64).sqrt().max(1e-9);
    }
    let stdz = |x: &[f64; NF]| -> [f64; NF] {
        let mut o = [0.0; NF];
        for j in 0..NF {
            o[j] = (x[j] - mean[j]) / std[j];
        }
        o
    };

    // Логістична регресія, class-weighted GD.
    let pos = train.iter().filter(|s| s.y == 1).count() as f64;
    let neg = train.len() as f64 - pos;
    let (wp, wn) = (train.len() as f64 / (2.0 * pos.max(1.0)), train.len() as f64 / (2.0 * neg.max(1.0)));
    let mut w = [0.0; NF];
    let mut b = 0.0;
    for _ in 0..EPOCHS {
        let mut gw = [0.0; NF];
        let mut gb = 0.0;
        for s in &train {
            let xz = stdz(&s.x);
            let z = (0..NF).map(|j| w[j] * xz[j]).sum::<f64>() + b;
            let p = sigmoid(z);
            let cw = if s.y == 1 { wp } else { wn };
            let err = (p - s.y as f64) * cw;
            for j in 0..NF {
                gw[j] += err * xz[j];
            }
            gb += err;
        }
        let n = train.len() as f64;
        for j in 0..NF {
            w[j] -= LR * (gw[j] / n + L2 * w[j]);
        }
        b -= LR * gb / n;
    }

    // Оцінка на TEST.
    let test_pairs: Vec<(f64, u8)> = test
        .iter()
        .map(|s| {
            let xz = stdz(&s.x);
            let z = (0..NF).map(|j| w[j] * xz[j]).sum::<f64>() + b;
            (sigmoid(z), s.y)
        })
        .collect();
    let a = auc(&test_pairs);
    // Діагностика: in-sample train AUC -> порожні фічі (underfit) чи overfit/regime-break?
    let train_pairs: Vec<(f64, u8)> = train.iter().map(|s| {
        let xz = stdz(&s.x);
        let z = (0..NF).map(|j| w[j] * xz[j]).sum::<f64>() + b;
        (sigmoid(z), s.y)
    }).collect();
    let train_auc = auc(&train_pairs);
    let test_pos = test.iter().filter(|s| s.y == 1).count() as f64 / test.len() as f64;
    // null: перемішати мітки test (детерміновано - реверс)
    let mut shuffled: Vec<(f64, u8)> = test_pairs.iter().map(|p| p.0).zip(test.iter().rev().map(|s| s.y)).collect();
    shuffled.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    let null_auc = auc(&shuffled);
    if std::env::var("SAVE_MODEL").is_ok() {
        let path = std::env::var("MODEL_OUT").unwrap_or_else(|_| format!("{root}/dataset/nextaction/vol-model.json"));
        let m = serde_json::json!({
            "w": w.to_vec(), "b": b, "mean": mean.to_vec(), "std": std.to_vec(),
            "feats": ["usd_rank","tok_rank","mom","log_mag","supply_rank","recent_rank","accel_rank"],
            "target": target, "h": h, "thr": thr, "high_signal": high_signal,
            "test_auc": a, "null_auc": null_auc,
        });
        let _ = fs::write(&path, serde_json::to_string_pretty(&m).unwrap());
        eprintln!("[train] модель збережено -> {path}");
    }

    let mode = if supply_signal { " [SUPPLY топ-децил емісії]" } else if high_signal { " [HIGH-SIGNAL топ-децил USD]" } else { "" };
    let tgt_desc = match target.as_str() {
        "bigdrop" => format!("велике падіння >{:.0}%", thr * 100.0),
        "bigdrop_adj" => format!("велике adj-падіння >{:.0}%", thr * 100.0),
        "extreme" => format!("великий ±рух >{:.0}% (ВОЛАТИЛЬНІСТЬ, не напрям)", thr * 100.0),
        "bigup" => format!("велике зростання >{:.0}%", thr * 100.0),
        _ => "недоперформить ринок".to_string(),
    };
    println!("\n=== НАВЧЕНА МОДЕЛЬ: whale-flow -> {tgt_desc} за {h}д{mode} ===");
    print!("фічі-ваги [usd,tok,mom,logmag,supply,recent,accel]:");
    for j in 0..NF { print!(" {:+.3}", w[j]); }
    println!("  bias={:+.3}", b);
    println!("train={} test={} (темпоральний спліт 70/30)", train.len(), test.len());
    println!("TEST posRate={:.0}% (частка з міткою y=1: {})", test_pos * 100.0, tgt_desc);
    println!("TRAIN AUC = {train_auc:.4}  (in-sample fit)");
    println!("TEST AUC = {a:.4}   (null={null_auc:.4})   -> {}", if a - null_auc > 0.02 && a > 0.53 { "Є прогнозна сила OOS" } else { "слабка/нема OOS" });
    // LIFT-таблиця: групуємо TEST за передбаченою ймовірністю у квінтилі -> реальна частка y=1.
    // Монотонне зростання = score економічно осмислений (не лише AUC>0.5).
    {
        let mut sp: Vec<&(f64, u8)> = test_pairs.iter().collect();
        sp.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let base = test_pairs.iter().filter(|p| p.1 == 1).count() as f64 / test_pairs.len().max(1) as f64;
        println!("LIFT-таблиця (TEST за квінтилем P; base y=1 = {:.1}%):", base * 100.0);
        for q in 0..5 {
            let lo = q * sp.len() / 5;
            let hi = (q + 1) * sp.len() / 5;
            if hi <= lo { continue; }
            let seg = &sp[lo..hi];
            let rate = seg.iter().filter(|p| p.1 == 1).count() as f64 / seg.len() as f64;
            let pmid = (seg[0].0 + seg[seg.len() - 1].0) / 2.0;
            let lift = if base > 0.0 { rate / base } else { 0.0 };
            println!("  Q{} P≈{:.3}  реальний y=1 {:>5.1}%  lift ×{:.2}", q + 1, pmid, rate * 100.0, lift);
        }
    }
    eprintln!("[train] готово. usd_rank>0 позитивна вага = високе накопичення -> вища P(недоперформить).");
}
