//! nullbase — нульова база для ЩОДЕННИХ сигналів: що дав би випадковий день входу.
//!
//! НАВІЩО. Присуд «сигнал працює» має сенс лише проти чогось. Проти нуля — не має:
//! у бичачий період зростає майже все, у ведмежий падає майже все. Проти 50% — теж:
//! альти проти ETH падають частіше, ніж ростуть, і «вгадано падіння 70%» може бути
//! просто дрейфом, а не знанням. Єдине чесне порівняння — з ВИПАДКОВИМ ДНЕМ ВХОДУ в
//! той самий актив на тому самому горизонті.
//!
//! Для ретро-реплею така база вже рахується (`sigretro`, зі свічок біржі). Але живі
//! щоденні сигнали — `groupwatch` і все, що з'явиться далі, — працюють не на свічках,
//! а на денних рішеннях, і для них потрібна своя база: та сама за змістом, але з
//! денним кроком і на значно довшій історії.
//!
//! ДЖЕРЕЛО — локальне: `dataset/backfill/token-prices-hist.json` (денні ряди DefiLlama,
//! до 2449 точок на токен). Жодного мережевого запиту: база — це властивість історії,
//! а не поточного стану, і ганяти за нею в мережу щоразу безглуздо.
//!
//! КЛЮЧ — ТІКЕР, і це свідоме спрощення з відомою ціною: однакових тікерів у наших
//! даних вистачає. Тому при збігу береться ряд, у якому БІЛЬШЕ точок (майже завжди це
//! той токен, що реально торгується), а кількість таких збігів друкується — щоб масштаб
//! спрощення був видний, а не мовчазний.
//!
//! Args: ROOT. Env: HORIZONS(1,7,30,90) MIN_POINTS(200) MIN_SAMPLES(50) TOL_D(2) BENCH(eth)

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;

fn env_s(k: &str, d: &str) -> String { std::env::var(k).unwrap_or_else(|_| d.to_string()) }
fn env_u(k: &str, d: u64) -> u64 { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

/// Ціна в момент `ts` — найближча точка, але не далі за `tol` секунд.
///
/// Допуск обов'язковий: у рядах бувають діри (токен не торгувався, джерело мовчало).
/// Узяти сусіда за тиждень означало б підмінити результат вигадкою, тож краще не дати
/// відповіді зовсім.
fn price_at(series: &[(i64, f64)], ts: i64, tol: i64) -> Option<f64> {
    if series.is_empty() { return None }
    let i = series.partition_point(|p| p.0 < ts);
    let cand = match (i.checked_sub(1).and_then(|k| series.get(k)), series.get(i)) {
        (_, Some(a)) if (a.0 - ts).abs() <= tol => Some(a),
        (Some(b), _) if (ts - b.0).abs() <= tol => Some(b),
        _ => None,
    }?;
    if cand.1.is_finite() && cand.1 > 0.0 { Some(cand.1) } else { None }
}

fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0 }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// Розподіл надлишку над базовим активом для всіх можливих днів входу.
fn distribution(series: &[(i64, f64)], bench: &[(i64, f64)], horizon_d: u32, tol: i64) -> Option<(usize, f64, f64)> {
    let h = horizon_d as i64 * 86_400;
    let last = series.last()?.0;
    let mut ex: Vec<f64> = Vec::new();
    let mut up = 0usize;
    for (ts, p0) in series {
        let t1 = ts + h;
        if t1 > last { break }
        if !(p0.is_finite() && *p0 > 0.0) { continue }
        let (Some(p1), Some(b0), Some(b1)) =
            (price_at(series, t1, tol), price_at(bench, *ts, tol), price_at(bench, t1, tol)) else { continue };
        if b0 <= 0.0 { continue }
        let e = 100.0 * (p1 - p0) / p0 - 100.0 * (b1 - b0) / b0;
        if !e.is_finite() { continue }
        if e > 0.0 { up += 1 }
        ex.push(e);
    }
    let n = ex.len();
    if n == 0 { return None }
    Some((n, median(&mut ex), 100.0 * up as f64 / n as f64))
}

fn series_of(v: &Value) -> Vec<(i64, f64)> {
    let mut out: Vec<(i64, f64)> = v["prices"].as_array().map(|a| a.iter().filter_map(|p| {
        let arr = p.as_array()?;
        Some((arr.first()?.as_f64()? as i64, arr.get(1)?.as_f64()?))
    }).collect()).unwrap_or_default();
    out.sort_by_key(|p| p.0);
    out
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let ds = std::env::var("DATASET_DIR").unwrap_or_else(|_| format!("{root}/dataset"));
    let horizons: Vec<u32> = env_s("HORIZONS", "1,7,30,90").split(',')
        .filter_map(|s| s.trim().parse().ok()).filter(|h| *h > 0).collect();
    let min_points = env_u("MIN_POINTS", 200) as usize;
    let min_samples = env_u("MIN_SAMPLES", 50) as usize;
    let tol = env_u("TOL_D", 2) as i64 * 86_400;
    let bench_key = env_s("BENCH", "eth");

    let path = format!("{ds}/backfill/token-prices-hist.json");
    let Some(v): Option<Value> = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()) else {
        eprintln!("[nullbase] {path} не прочитано — рахувати нема з чого");
        std::process::exit(2);
    };
    let Some(tokens) = v["tokens"].as_object() else {
        eprintln!("[nullbase] у файлі немає поля tokens");
        std::process::exit(2);
    };

    let bench = tokens.get(&bench_key).map(series_of).unwrap_or_default();
    if bench.len() < min_points {
        // Без базового активу надлишок не існує як величина. Це відмова, а не попередження.
        eprintln!("[nullbase] базовий ряд «{bench_key}» закороткий ({} точок) — без нього надлишок не порахувати", bench.len());
        std::process::exit(2);
    }
    eprintln!("[nullbase] базовий ряд {bench_key}: {} точок", bench.len());

    // тікер -> (кількість точок, ряд) — при збігу тікерів виграє довший ряд
    let mut best: HashMap<String, (usize, Vec<(i64, f64)>)> = HashMap::new();
    let mut collisions = 0usize;
    for (contract, t) in tokens {
        if contract == &bench_key { continue }
        let sym = t["sym"].as_str().unwrap_or("").trim().to_uppercase();
        if sym.is_empty() { continue }
        let s = series_of(t);
        if s.len() < min_points { continue }
        match best.get(&sym) {
            Some((n, _)) if *n >= s.len() => { collisions += 1; }
            Some(_) => { collisions += 1; best.insert(sym, (s.len(), s)); }
            None => { best.insert(sym, (s.len(), s)); }
        }
    }
    eprintln!("[nullbase] придатних рядів: {} (збігів тікерів: {collisions})", best.len());

    let mut by_sym = serde_json::Map::new();
    let mut cells = 0usize;
    for (sym, (_, s)) in &best {
        let mut per_h = serde_json::Map::new();
        for &h in &horizons {
            let Some((n, med, up)) = distribution(s, &bench, h, tol) else { continue };
            // Замало спостережень — клітинки просто немає. Порожнеча чесніша за
            // число, порахуване з десяти точок.
            if n < min_samples { continue }
            per_h.insert(h.to_string(), json!({"n": n, "median_excess": med, "share_up": up}));
            cells += 1;
        }
        if !per_h.is_empty() { by_sym.insert(sym.clone(), Value::Object(per_h)); }
    }

    // Дописуємо в наявний файл, не перетираючи бази інших таймфреймів: 1h і 5m
    // рахує sigretro, і вони мають лишитись на місці.
    let out_dir = format!("{ds}/retro");
    let _ = fs::create_dir_all(&out_dir);
    let out_path = format!("{out_dir}/retro-null.json");
    let mut all: Value = fs::read_to_string(&out_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"by_tf": {}}));
    if all["by_tf"].as_object().is_none() { all["by_tf"] = json!({}); }
    all["by_tf"]["1d"] = Value::Object(by_sym);
    all["asof1d"] = json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0));
    all["source1d"] = json!("token-prices-hist.json (денні ряди DefiLlama), ключ — тікер");

    match fs::write(&out_path, serde_json::to_string_pretty(&all).unwrap_or_default()) {
        Ok(_) => eprintln!("[nullbase] записано: {out_path} · активів {} · клітинок {cells}", best.len()),
        Err(e) => { eprintln!("[nullbase] НЕ записано: {e}"); std::process::exit(2) }
    }
    if cells == 0 {
        eprintln!("[nullbase] УВАГА: жодної клітинки — присуду для щоденних сигналів не буде");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_beyond_tolerance_is_refused() {
        let s = vec![(0i64, 1.0), (100i64, 2.0)];
        assert_eq!(price_at(&s, 50, 10), None, "найближча точка за 50 с при допуску 10 — це не ціна моменту");
        assert_eq!(price_at(&s, 105, 10), Some(2.0));
    }

    /// Актив, що рухається точно як база, має дати нульовий надлишок і 0% зростань.
    #[test]
    fn asset_moving_with_the_benchmark_has_no_excess() {
        let mut s = Vec::new();
        for i in 0..400i64 { s.push((i * 86_400, 100.0 * (1.0 + i as f64 / 1000.0))); }
        let bench = s.clone();
        let (n, med, up) = distribution(&s, &bench, 7, 2 * 86_400).unwrap();
        assert!(n > 300);
        assert!(med.abs() < 1e-9, "надлишок над самим собою має бути нульовим, а не {med}");
        assert_eq!(up, 0.0);
    }

    /// Актив, що стабільно обганяє базу, має додатний надлишок і частку зростань 100%.
    #[test]
    fn asset_outperforming_the_benchmark_is_detected() {
        let mut s = Vec::new();
        let mut b = Vec::new();
        for i in 0..400i64 {
            s.push((i * 86_400, 100.0 * 1.01f64.powi(i as i32)));
            b.push((i * 86_400, 100.0));
        }
        let (_, med, up) = distribution(&s, &b, 7, 2 * 86_400).unwrap();
        assert!(med > 5.0, "тиждень по +1% на день — це помітно більше за нуль");
        assert_eq!(up, 100.0);
    }

    #[test]
    fn horizon_longer_than_history_yields_nothing() {
        let s = vec![(0i64, 1.0), (86_400, 1.1)];
        let b = s.clone();
        assert!(distribution(&s, &b, 90, 86_400).is_none());
    }
}
