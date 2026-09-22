//! Крок C: тест ПРЕДИКТИВНОЇ СИЛИ whale-накопичення -> forward-return ціни, з null-baseline.
//!
//! Гіпотеза: коли наші кити НАКОПИЧУЮТЬ токен, ціна згодом росте. Перевіряємо чесно:
//! IC(signal, forward-return) по токенах + event-study (return після топ-10% накопичення)
//! + null (перемішування). Два варіанти сигналу:
//!   - token-одиниці (сирий net-flow), і
//!   - USD-зважено (amount × ціна дня) — щоб дрібні токени не домінували.
//!
//! Вхід: dataset/backfill/transfers.csv + dataset/nextaction/prices/{chain}_{addr}.json.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

const DAY: i64 = 86_400;
const W: i64 = 7;
const HORIZONS: [i64; 3] = [7, 14, 30];
const MIN_PAIRS: usize = 20;
const NULL_REPS: usize = 200;

struct Lcg(u64);
impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

fn pearson(xs: &[f64], ys: &[f64]) -> Option<f64> {
    let n = xs.len();
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let mx = xs.iter().sum::<f64>() / nf;
    let my = ys.iter().sum::<f64>() / nf;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let dx = xs[i] - mx;
        let dy = ys[i] - my;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx <= 0.0 || syy <= 0.0 {
        return None;
    }
    Some(sxy / (sxx.sqrt() * syy.sqrt()))
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// IC + event-study(+null) для заданої flow-мапи (token-key -> day -> net-flow). w = вікно накопичення.
fn analyze(label: &str, flow: &HashMap<String, HashMap<i64, f64>>, price_of: &HashMap<String, HashMap<i64, f64>>, w: i64) {
    println!("\n========== СИГНАЛ: {label} (W={w}д) ==========");
    // IC
    for &h in HORIZONS.iter() {
        let mut ics: Vec<f64> = Vec::new();
        let mut null_means: Vec<f64> = Vec::new();
        for (key, pdays) in price_of {
            let Some(fdays) = flow.get(key) else { continue };
            let mut days: Vec<i64> = pdays.keys().copied().collect();
            days.sort_unstable();
            let mut sig = Vec::new();
            let mut ret = Vec::new();
            for &t in &days {
                let (Some(&p0), Some(&p1)) = (pdays.get(&t), pdays.get(&(t + h))) else { continue };
                if p0 <= 0.0 {
                    continue;
                }
                let mut s = 0.0;
                for d in (t - w + 1)..=t {
                    if let Some(&fv) = fdays.get(&d) {
                        s += fv;
                    }
                }
                sig.push(s);
                ret.push(p1 / p0 - 1.0);
            }
            if sig.len() < MIN_PAIRS {
                continue;
            }
            let Some(ic) = pearson(&sig, &ret) else { continue };
            ics.push(ic);
            let mut lcg = Lcg(0x9E3779B97F4A7C15 ^ (key.len() as u64).wrapping_mul(2654435761) ^ (h as u64));
            let mut acc = 0.0;
            let mut cnt = 0;
            let mut shuffled = ret.clone();
            for _ in 0..NULL_REPS {
                lcg.shuffle(&mut shuffled);
                if let Some(nic) = pearson(&sig, &shuffled) {
                    acc += nic;
                    cnt += 1;
                }
            }
            if cnt > 0 {
                null_means.push(acc / cnt as f64);
            }
        }
        let mean_ic = if ics.is_empty() { 0.0 } else { ics.iter().sum::<f64>() / ics.len() as f64 };
        let med_ic = median(ics.clone());
        let pos = if ics.is_empty() { 0.0 } else { ics.iter().filter(|&&v| v > 0.0).count() as f64 / ics.len() as f64 };
        let null_mean = if null_means.is_empty() { 0.0 } else { null_means.iter().sum::<f64>() / null_means.len() as f64 };
        println!("IC  H={h:>2}д | ток={:>4} | mean={mean_ic:+.4} med={med_ic:+.4} pos={:.0}% | null={null_mean:+.4}", ics.len(), pos * 100.0);
    }
    // Event-study + null
    for &h in HORIZONS.iter() {
        let mut excess: Vec<f64> = Vec::new();
        let mut null_excess: Vec<f64> = Vec::new();
        let mut ev_all: Vec<f64> = Vec::new();
        let mut base_all: Vec<f64> = Vec::new();
        let mut signif = 0usize;
        for (key, pdays) in price_of {
            let Some(fdays) = flow.get(key) else { continue };
            let mut days: Vec<i64> = pdays.keys().copied().collect();
            days.sort_unstable();
            let mut sig = Vec::new();
            let mut ret = Vec::new();
            for &t in &days {
                let (Some(&p0), Some(&p1)) = (pdays.get(&t), pdays.get(&(t + h))) else { continue };
                if p0 <= 0.0 {
                    continue;
                }
                let mut s = 0.0;
                for d in (t - w + 1)..=t {
                    if let Some(&fv) = fdays.get(&d) {
                        s += fv;
                    }
                }
                sig.push(s);
                ret.push(p1 / p0 - 1.0);
            }
            if sig.len() < MIN_PAIRS {
                continue;
            }
            let mut sorted = sig.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let thr = sorted[((sorted.len() as f64 * 0.9) as usize).min(sorted.len() - 1)];
            let ev_idx: Vec<usize> = (0..sig.len()).filter(|&i| sig[i] >= thr && sig[i] > 0.0).collect();
            if ev_idx.len() < 3 {
                continue;
            }
            let avg_all = ret.iter().sum::<f64>() / ret.len() as f64;
            let avg_ev = ev_idx.iter().map(|&i| ret[i]).sum::<f64>() / ev_idx.len() as f64;
            let real_ex = avg_ev - avg_all;
            excess.push(real_ex);
            ev_all.push(avg_ev);
            base_all.push(avg_all);
            let mut lcg = Lcg(0xD1B54A32D192ED03 ^ (key.len() as u64).wrapping_mul(0x9E3779B1) ^ (h as u64));
            let mut shuffled = ret.clone();
            let mut nulls: Vec<f64> = Vec::with_capacity(NULL_REPS);
            for _ in 0..NULL_REPS {
                lcg.shuffle(&mut shuffled);
                let a = ev_idx.iter().map(|&i| shuffled[i]).sum::<f64>() / ev_idx.len() as f64;
                nulls.push(a - avg_all);
            }
            null_excess.push(nulls.iter().sum::<f64>() / nulls.len() as f64);
            nulls.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p5 = nulls[((nulls.len() as f64 * 0.05) as usize).min(nulls.len() - 1)];
            if real_ex < p5 {
                signif += 1;
            }
        }
        if excess.is_empty() {
            println!("EV  H={h:>2}д: недостатньо подій");
            continue;
        }
        let n = excess.len();
        let mean_ex = excess.iter().sum::<f64>() / n as f64;
        let pos = excess.iter().filter(|&&v| v > 0.0).count() as f64 / n as f64;
        let ev_avg = ev_all.iter().sum::<f64>() / n as f64;
        let base_avg = base_all.iter().sum::<f64>() / n as f64;
        let null_mean = null_excess.iter().sum::<f64>() / null_excess.len() as f64;
        println!(
            "EV  H={h:>2}д | ток={n:>4} | подія={:+.2}% база={:+.2}% надл={:+.2}% (null {:+.3}%) | +надл={:.0}% | знач<null5%={:.0}%",
            ev_avg * 100.0, base_avg * 100.0, mean_ex * 100.0, null_mean * 100.0, pos * 100.0, signif as f64 / n as f64 * 100.0
        );
    }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let prices_dir = format!("{root}/dataset/nextaction/prices");

    // 1) Ціни: token -> day -> price.
    let mut price_of: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let Ok(entries) = fs::read_dir(&prices_dir) else {
        eprintln!("[signal] нема {prices_dir} (Крок B ще не відпрацював?)");
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
    eprintln!("[signal] токенів з цінами (>=30 днів): {}", price_of.len());
    if price_of.is_empty() {
        return;
    }

    // 2) Whale net-flow per (token, day): token-одиниці + USD-зважено.
    let mm: HashSet<String> = std::env::var("MM_EXCLUDE").ok().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
    eprintln!("[bin] MM-виключено китів: {}", mm.len());
    let f = BufReader::new(File::open(format!("{root}/dataset/backfill/transfers.csv")).expect("transfers.csv"));
    let mut flow: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let mut flow_usd: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    let mut whale_id: HashMap<String, u32> = HashMap::new();
    // breadth: token -> day -> whale -> net того дня (потім -> net к-сть китів)
    let mut breadth_tmp: HashMap<String, HashMap<i64, HashMap<u32, f64>>> = HashMap::new();
    let mut rows = 0u64;
    for line in f.lines() {
        let Ok(line) = line else { continue };
        let mut left = line.splitn(6, ',');
        let wallet = left.next().unwrap_or("");
        // Нульова адреса (mint/burn) — не кит; пропускаємо, щоб не засмічувати net-flow.
        if wallet == wr_store::addrs::zero() { continue; }
        if mm.contains(wallet) { continue; }
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
        rows += 1;
        let signed = if dir == "i" { amount } else { -amount };
        *flow.entry(key.clone()).or_default().entry(day).or_insert(0.0) += signed;
        let next_id = whale_id.len() as u32;
        let wid = *whale_id.entry(wallet.to_string()).or_insert(next_id);
        *breadth_tmp.entry(key.clone()).or_default().entry(day).or_default().entry(wid).or_insert(0.0) += signed;
        if let Some(&pr) = pdays.get(&day) {
            *flow_usd.entry(key).or_default().entry(day).or_insert(0.0) += signed * pr;
        }
    }
    eprintln!("[signal] whale-рядків (priced): {rows}; токенів flow={} flow_usd={}", flow.len(), flow_usd.len());

    // breadth: net к-сть КИТІВ за день (net-in - net-out); analyze сумує за W.
    let mut flow_breadth: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    for (key, ddays) in &breadth_tmp {
        let dst = flow_breadth.entry(key.clone()).or_default();
        for (&day, wm) in ddays {
            let mut c = 0.0;
            for &net in wm.values() {
                if net > 0.0 {
                    c += 1.0;
                } else if net < 0.0 {
                    c -= 1.0;
                }
            }
            dst.insert(day, c);
        }
    }
    drop(breadth_tmp);

    let w: i64 = std::env::var("NA_W").ok().and_then(|s| s.parse().ok()).unwrap_or(7);
    analyze("token-одиниці (сирий обсяг)", &flow, &price_of, w);
    analyze("USD-зважено (amount x ціна дня)", &flow_usd, &price_of, w);
    analyze("breadth (net к-сть китів/день)", &flow_breadth, &price_of, w);

    eprintln!("[signal] готово. W={w}д, MIN_PAIRS={MIN_PAIRS}, NULL_REPS={NULL_REPS}");
}
