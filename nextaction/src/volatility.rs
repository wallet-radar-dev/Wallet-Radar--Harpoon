//! Виробничий ВОЛАТИЛЬНИЙ сигнал: P(великий рух ±THR за H днів) для токенів, де кити ЗАРАЗ
//! у топ-децилі USD-накопичення. Застосовує навчену модель `vol-model.json`
//! (TARGET=extreme, HIGH_SIGNAL, test AUC ~0.57 vs null 0.50 — реальна OOS-сила).
//! Не вгадує НАПРЯМ, а величину руху → straddle/«очікуй феєрверк». Виключає null+MM.
//! Вихід: dataset/strategy-volatility.json — прод віддає через passthrough-роут /api/whale-volatility.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

const DAY: i64 = 86_400;
const W: i64 = 7;
const MIN_HIST: usize = 30;
const NF: usize = 9;
const RVOL_WIN: i64 = 14;

fn percentile_rank(sorted: &[f64], v: f64) -> f64 {
    if sorted.is_empty() {
        return 0.5;
    }
    let (mut lo, mut hi) = (0usize, sorted.len());
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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn arr(v: &Value, k: &str) -> Vec<f64> {
    v[k].as_array()
        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect())
        .unwrap_or_default()
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(5000.0);

    // 1) навчена модель
    let mv: Value = serde_json::from_str(
        &fs::read_to_string(format!("{root}/dataset/nextaction/vol-model.json")).expect("vol-model.json"),
    )
    .expect("parse vol-model.json");
    let w = arr(&mv, "w");
    let mean = arr(&mv, "mean");
    let std = arr(&mv, "std");
    let b = mv["b"].as_f64().unwrap_or(0.0);
    let thr = mv["thr"].as_f64().unwrap_or(0.2);
    let h = mv["h"].as_i64().unwrap_or(14);
    let model_auc = mv["test_auc"].as_f64().unwrap_or(0.0);
    let null_auc = mv["null_auc"].as_f64().unwrap_or(0.5);
    assert_eq!(w.len(), NF, "модель має {} ваг, очікую {NF}", w.len());

    // 2) виключення: MM (env) + null
    let mut excl: HashSet<String> = std::env::var("MM_EXCLUDE")
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    excl.insert(wr_store::addrs::zero().to_string());

    // 3) ціни + символ
    let mut price_of: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Ok(rd) = fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
        for ent in rd.flatten() {
            let fname = ent.file_name().to_string_lossy().to_string();
            let Some(stem) = fname.strip_suffix(".json") else { continue };
            let Some((c, a)) = stem.split_once('_') else { continue };
            let key = format!("{c}\t{a}");
            let Ok(txt) = fs::read_to_string(ent.path()) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
            if let Some(sym) = v["symbol"].as_str() {
                symbol.insert(key.clone(), sym.to_string());
            }
            let mut days = HashMap::new();
            if let Some(pa) = v["prices"].as_array() {
                for p in pa {
                    let t = p["timestamp"].as_i64().unwrap_or(0);
                    let pr = p["price"].as_f64().unwrap_or(0.0);
                    if t > 0 && pr > 0.0 {
                        days.insert(t / DAY, pr);
                    }
                }
            }
            if !days.is_empty() {
                price_of.insert(key, days);
            }
        }
    }

    // 4) supply.tsv: key -> total supply
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

    // 4b) ОРТОГОНАЛЬНЕ: holders-к-ть (candidates.tsv) -> проксі поширеності/ліквідності (менше -> вибуховіше)
    let mut hld: HashMap<String, f64> = HashMap::new();
    if let Ok(txt) = fs::read_to_string(format!("{root}/dataset/nextaction/candidates.tsv")) {
        for line in txt.lines() {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() >= 4 && c[0] != "chain" {
                if let Ok(hh) = c[3].parse::<f64>() {
                    hld.insert(format!("{}\t{}", c[0].to_lowercase(), c[1].to_lowercase()), (hh + 1.0).ln());
                }
            }
        }
    }

    // 5) whale flow_usd + flow_tok per (token, day) — без null/MM, лише priced
    let f = BufReader::new(
        File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"),
    );
    let mut fu: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let mut ft: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for line in f.lines() {
        let Ok(line) = line else { continue };
        let mut left = line.splitn(6, ',');
        let wallet = left.next().unwrap_or("");
        if wallet == "wallet" || excl.contains(wallet) {
            continue;
        }
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
        *ft.entry(key.clone()).or_default().entry(day).or_insert(0.0) += signed;
        if let Some(&pr) = pdays.get(&day) {
            *fu.entry(key).or_default().entry(day).or_insert(0.0) += signed * pr;
        }
    }

    // 6) для кожного токена — фічі ОСТАННЬОГО дня (як у train.rs), скоринг моделлю
    let mut out: Vec<Value> = Vec::new();
    for (key, pdays) in &price_of {
        let (Some(fud), Some(ftd)) = (fu.get(key), ft.get(key)) else { continue };
        let mut days: Vec<i64> = pdays.keys().copied().collect();
        days.sort_unstable();
        let sup = supply.get(key).copied();
        let holders_log = hld.get(&key.to_lowercase()).copied().unwrap_or(0.0);
        let (mut p_usd, mut p_tok, mut p_sup, mut p_r2, mut p_ac, mut p_rvol): (
            Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>,
        ) = (vec![], vec![], vec![], vec![], vec![], vec![]);
        let mut last: Option<([f64; NF], f64, i64)> = None; // (features, usd_acc, day)
        for &t in &days {
            let usd_acc: f64 = (t - W + 1..=t).filter_map(|d| fud.get(&d)).sum();
            let tok_acc: f64 = (t - W + 1..=t).filter_map(|d| ftd.get(&d)).sum();
            let r2: f64 = (t - 1..=t).filter_map(|d| fud.get(&d)).sum();
            let r2_prev: f64 = (t - 3..=t - 2).filter_map(|d| fud.get(&d)).sum();
            let accel = r2 - r2_prev;
            let sup_norm = sup.map(|s| tok_acc / s).unwrap_or(0.0);
            let mut rets: Vec<f64> = Vec::new();
            for d in (t - RVOL_WIN + 1)..=t {
                if let (Some(&pa), Some(&pb)) = (pdays.get(&d), pdays.get(&(d - 1))) {
                    if pa > 0.0 && pb > 0.0 { rets.push((pa / pb).ln()); }
                }
            }
            let rvol = if rets.len() >= 3 {
                let mm = rets.iter().sum::<f64>() / rets.len() as f64;
                (rets.iter().map(|r| (r - mm).powi(2)).sum::<f64>() / rets.len() as f64).sqrt()
            } else { 0.0 };
            if p_usd.len() >= MIN_HIST {
                if let (Some(&p0), Some(&pw)) = (pdays.get(&t), pdays.get(&(t - W))) {
                    if p0 > 0.0 && pw > 0.0 {
                        let mut su = p_usd.clone();
                        su.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let mut st = p_tok.clone();
                        st.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let mut ss = p_sup.clone();
                        ss.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let mut sr = p_r2.clone();
                        sr.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let mut sa = p_ac.clone();
                        sa.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let mut sv = p_rvol.clone();
                        sv.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let x = [
                            percentile_rank(&su, usd_acc),
                            percentile_rank(&st, tok_acc),
                            (p0 / pw - 1.0).clamp(-1.0, 3.0),
                            (usd_acc.max(0.0) + 1.0).ln(),
                            if sup.is_some() { percentile_rank(&ss, sup_norm) } else { 0.5 },
                            percentile_rank(&sr, r2),
                            percentile_rank(&sa, accel),
                            percentile_rank(&sv, rvol),
                            holders_log,
                        ];
                        last = Some((x, usd_acc, t));
                    }
                }
            }
            p_usd.push(usd_acc);
            p_tok.push(tok_acc);
            p_sup.push(sup_norm);
            p_r2.push(r2);
            p_ac.push(accel);
            p_rvol.push(rvol);
        }
        let Some((x, usd_acc, lday)) = last else { continue };
        // модель навчена на HIGH_SIGNAL (топ-децил USD) — скоримо лише in-distribution
        if x[0] < 0.9 || usd_acc < min_usd {
            continue; // топ-децил накопичення + значущий USD (не пил)
        }
        let z: f64 = (0..NF).map(|j| w[j] * (x[j] - mean[j]) / std[j].max(1e-9)).sum::<f64>() + b;
        let p = sigmoid(z);
        let (chain, token) = key.split_once('\t').unwrap();
        out.push(json!({
            "chain": chain,
            "token": token,
            "symbol": symbol.get(key).cloned().unwrap_or_default(),
            "pBigMove": (p * 1000.0).round() / 1000.0,
            "usdRank": (x[0] * 100.0).round() / 100.0,
            "usdAccum7d": (usd_acc * 100.0).round() / 100.0,
            "lastDay": lday,
        }));
    }
    out.sort_by(|a, b| {
        b["pBigMove"].as_f64().unwrap_or(0.0).partial_cmp(&a["pBigMove"].as_f64().unwrap_or(0.0)).unwrap()
    });

    let res = json!({
        "generated": now(),
        "signal": "ВОЛАТИЛЬНІСТЬ: P(великий рух >|THR| за H днів) для токенів у топ-децилі USD-накопичення китів",
        "model": {"target": "extreme", "thresholdPct": (thr * 100.0), "horizonDays": h, "testAuc": (model_auc*1000.0).round()/1000.0, "nullAuc": (null_auc*1000.0).round()/1000.0},
        "note": "НЕ вгадує напрям — лише величину. Кити тяжко набирають -> токен імовірно рвоне ±20-30%. Торгується як straddle/очікування великого руху. Пороговий-хвостовий сигнал.",
        "count": out.len(),
        "tokens": out,
    });
    let path = format!("{root}/dataset/strategy-volatility.json");
    fs::write(&path, serde_json::to_string_pretty(&res).unwrap()).expect("write");
    eprintln!("[volatility] {} токенів -> {path} (модель AUC {model_auc:.3} vs null {null_auc:.3})", res["count"]);
    println!("=== ВОЛАТИЛЬНІ КАНДИДАТИ (P великого руху, топ USD-накопичення) ===");
    for t in res["tokens"].as_array().unwrap().iter().take(20) {
        println!(
            "  {:<10} {:<9} P={:<6} usd7d={:>14}",
            t["symbol"].as_str().unwrap_or(""),
            t["chain"].as_str().unwrap_or(""),
            t["pBigMove"],
            t["usdAccum7d"]
        );
    }
}
