//! ПАТЕРН eventscan (Фаза 5.2/X.3 плану): РОЗШИРЕНИЙ БЕКТЕСТ на подіях координованих викупів.
//! Замість 39 хвиль роздавача — бере ВСІ датовані викупи емісії (coord-*.json, first_ts) як події
//! й міряє форвардну дохідність токена на +1/+3/+7/+30 днів проти НУЛЬ-МОДЕЛІ (випадкові дати того ж
//! токена) та надлишкову проти ETH. Довірчі інтервали 95% — чесно показує, чи сигнал доведений.
//! Обмеження: форвардна ціна є лише для токенів у ціновій історії (DefiLlama hourly) — дрібняк без цін
//! відсіюється; eventscan чесно друкує, скільки подій із N мали ціни.
//! Читає token-prices-hourly.json + coord-*.json. Аргументи: null_iters=200 max_gap_h=48
use std::collections::HashMap;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct EventScan;
const DAY: i64 = 86_400;
const HORIZONS: [i64; 4] = [1, 3, 7, 30];

/// детермінований LCG (відтворюваність важливіша за «справжню» випадковість для нуль-моделі)
struct Lcg(u64);
impl Lcg { fn next(&mut self) -> u64 { self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); self.0 >> 16 } }

fn find_hourly(dir: &str) -> Option<String> {
    for p in [format!("{dir}/token-prices-hourly.json"), format!("{dir}/../token-prices-hourly.json"),
              "/opt/wallet-tracker/dataset/backfill/token-prices-hourly.json".to_string()] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

/// ціна найближча до ts у межах max_gap секунд (серія відсортована за ts)
fn price_at(series: &[(i64, f64)], ts: i64, max_gap: i64) -> Option<f64> {
    if series.is_empty() { return None }
    let i = series.partition_point(|&(t, _)| t < ts);
    let cand = [i.saturating_sub(1), i.min(series.len() - 1)];
    cand.iter().map(|&j| series[j]).filter(|&(t, _)| (t - ts).abs() <= max_gap)
        .min_by_key(|&(t, _)| (t - ts).abs()).map(|(_, p)| p)
}

/// форвардна дохідність (ret) на горизонті H днів від ts0
fn fwd_ret(series: &[(i64, f64)], ts0: i64, h_days: i64, gap: i64) -> Option<f64> {
    let p0 = price_at(series, ts0, gap)?;
    let ph = price_at(series, ts0 + h_days * DAY, gap)?;
    if p0 > 0.0 { Some(ph / p0 - 1.0) } else { None }
}

impl Pattern for EventScan {
    fn name(&self) -> &str { "eventscan" }
    fn about(&self) -> &str { "розширений бектест: подієве дослідження викупів vs нуль-модель + CI (X.3)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let null_iters = num(args, "null_iters", 200.0) as u64;
        let gap = (num(args, "max_gap_h", 48.0) * 3600.0) as i64;

        // ціни: contract -> [(ts, price)]
        let hp = find_hourly(&ctx.dir).ok_or("нема token-prices-hourly.json (спершу hourprices)")?;
        let hv: Value = serde_json::from_str(&std::fs::read_to_string(&hp).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let mut px: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        if let Some(tk) = hv["tokens"].as_object() {
            for (c, v) in tk {
                let mut s: Vec<(i64, f64)> = v["prices"].as_array().map(|a| a.iter().filter_map(|p| {
                    let p = p.as_array()?; Some((p[0].as_i64()?, p[1].as_f64()?)) }).collect()).unwrap_or_default();
                s.sort_by_key(|x| x.0);
                if !s.is_empty() { px.insert(c.to_lowercase(), s); }
            }
        }
        let eth = px.get(wr_store::addrs::weth()).cloned();

        // події: (token, first_ts) з усіх coord-*.json у теці
        let mut events: Vec<(String, i64)> = vec![];
        if let Ok(rd) = std::fs::read_dir(&ctx.dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !(name.starts_with("coord-") && name.ends_with(".json")) { continue }
                let Some(v) = std::fs::read_to_string(e.path()).ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { continue };
                if let Some(arr) = v.as_array() {
                    for c in arr {
                        let t = c["token"].as_str().unwrap_or("").to_lowercase();
                        let ts = c["first_ts"].as_i64().unwrap_or(0);
                        if !t.is_empty() && ts > 0 { events.push((t, ts)); }
                    }
                }
            }
        }
        let total_events = events.len();

        // збираємо форвардні дохідності по горизонтах (тільки де є ціни)
        let mut ret: [Vec<f64>; 4] = Default::default();     // сира дохідність
        let mut exc: [Vec<f64>; 4] = Default::default();     // надлишкова vs ETH
        let mut nul: [Vec<f64>; 4] = Default::default();     // нуль-модель (випадкові дати)
        let mut priced = 0usize;
        let mut lcg = Lcg(0x9E3779B97F4A7C15);
        for (t, ts0) in &events {
            let Some(s) = px.get(t) else { continue };
            let mut any = false;
            for (hi, &h) in HORIZONS.iter().enumerate() {
                if let Some(r) = fwd_ret(s, *ts0, h, gap) {
                    ret[hi].push(r); any = true;
                    if let Some(es) = &eth { if let Some(er) = fwd_ret(es, *ts0, h, gap) { exc[hi].push(r - er); } }
                }
            }
            if any { priced += 1; }
            // нуль-модель: null_iters випадкових дат у діапазоні серії цього токена
            let (lo, hi_ts) = (s[0].0, s[s.len() - 1].0 - 30 * DAY);
            if hi_ts > lo {
                for _ in 0..null_iters {
                    let rts = lo + (lcg.next() as i64 % (hi_ts - lo)).abs();
                    for (hi, &h) in HORIZONS.iter().enumerate() {
                        if let Some(r) = fwd_ret(s, rts, h, gap) { nul[hi].push(r); }
                    }
                }
            }
        }

        let stats = |v: &[f64]| -> (f64, f64, f64, f64) { // (mean, ci95, hit_neg, n)
            let n = v.len() as f64;
            if n < 1.0 { return (0.0, 0.0, 0.0, 0.0) }
            let m = v.iter().sum::<f64>() / n;
            let sd = (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n).sqrt();
            let ci = 1.96 * sd / n.sqrt();
            let hit = v.iter().filter(|&&x| x < 0.0).count() as f64 / n;
            (m, ci, hit, n)
        };

        println!("── РОЗШИРЕНИЙ БЕКТЕСТ ВИКУПІВ (X.3, dict {}) ──", wr_dict::dict_version());
        println!("подій-викупів усього: {total_events} · З ЦІНОВОЮ ІСТОРІЄЮ: {priced} (стеля — покриття DefiLlama; дрібняк без цін відсіяно)");
        println!("нуль-модель: {null_iters} випадкових дат/токен · ETH-надлишок: {}", if eth.is_some() { "так" } else { "нема WETH у цінах" });
        println!("\nгіпотеза плану: сильний координований викуп ПЕРЕДУЄ падінню (контраріан) → очікуємо середню дохідність < 0\n");
        println!("{:<7} {:>10} {:>16} {:>9} {:>14} {:>10}", "гориз.", "n", "серед.ret ±CI95", "hit<0", "надлишок vsETH", "нуль сер.");
        for (hi, &h) in HORIZONS.iter().enumerate() {
            let (m, ci, hit, n) = stats(&ret[hi]);
            let (em, eci, _, _) = stats(&exc[hi]);
            let (nm, _, _, _) = stats(&nul[hi]);
            let sig = if n >= 2.0 && (m - ci > 0.0 || m + ci < 0.0) { "*" } else { " " };
            println!("{:<7} {:>10.0} {:>10.1}% ±{:.1}%{} {:>7.0}% {:>10.1}% ±{:.1}% {:>9.1}%",
                format!("+{h}дн"), n, m * 100.0, ci * 100.0, sig, hit * 100.0, em * 100.0, eci * 100.0, nm * 100.0);
        }
        println!("\n* = CI95 не накриває 0 (статистично відрізняється від нуля). Інакше — сигнал НЕ доведений при цьому n.");
        if priced < 100 { println!("УВАГА: n={priced} < 100 — доказова база все ще тонка; щоб побільшити, спричинити hourprices на токени викупів із цінового всесвіту."); }
        Ok(())
    }
}
