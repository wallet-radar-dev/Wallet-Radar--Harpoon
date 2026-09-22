//! flowbt — ЧЕСНИЙ БЕКТЕСТ: чи передбачає CEX taker-потік майбутній рух ціни?
//! Сигнал(t) = (takerBuy − takerSell)/обсяг ∈ [−1,1] (з Binance klines 1h, до 1000 год назад).
//! Форвардна дохідність ret(t→t+H) для H=1/6/24 год. Рахуємо по ВСІХ (токен×година):
//!   • hit-rate: sign(signal)==sign(ret) — чи вгадує напрямок;
//!   • середня дохідність при signal>0 vs <0 — чи є спред;
//!   • AUC (Mann-Whitney): P(ret вищий при signal>0) — 0.5=нема едж, >0.55 едж, <0.45 контраріан.
//! Це РЕАЛЬНА цифра з out-of-sample історії, не обіцянка. Один запуск, друк.
//! Запуск: flowbt SYM1USDT SYM2USDT ...  (дефолт — розширений кошик)
use serde_json::Value;

const HOSTS: [&str; 3] = ["https://api.binance.com", "https://api1.binance.com", "https://data-api.binance.vision"];

fn klines(symbol: &str, interval: &str) -> Option<Value> {
    for h in HOSTS {
        let url = format!("{h}/api/v3/klines?symbol={symbol}&interval={interval}&limit=1000");
        if let Ok(r) = ureq::get(&url).timeout(std::time::Duration::from_secs(25)).call() {
            if let Ok(v) = r.into_json::<Value>() { return Some(v) }
        }
    }
    None
}

/// AUC (Mann-Whitney): P(score вищий у мітки=1). samples=(score, label). 0.5 = випадково.
fn auc(mut s: Vec<(f64, bool)>) -> (f64, usize, usize) {
    s.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (mut rank_sum, mut i) = (0.0f64, 0usize);
    while i < s.len() {
        let mut j = i; while j + 1 < s.len() && s[j + 1].0 == s[i].0 { j += 1; }
        let avg_rank = (i + j) as f64 / 2.0 + 1.0; // середній ранг для звʼязок (1-based)
        for k in i..=j { if s[k].1 { rank_sum += avg_rank; } }
        i = j + 1;
    }
    let np = s.iter().filter(|x| x.1).count();
    let nn = s.len() - np;
    if np == 0 || nn == 0 { return (0.5, np, nn) }
    let auc = (rank_sum - (np * (np + 1)) as f64 / 2.0) / (np * nn) as f64;
    (auc, np, nn)
}

fn main() {
    let interval = std::env::var("INTERVAL").unwrap_or_else(|_| "1h".into());
    let horizons: Vec<usize> = std::env::var("HZ").unwrap_or_else(|_| "1,6,24".into())
        .split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let unit = if interval.ends_with('d') { "дн" } else if interval.ends_with('h') { "год" } else { &interval };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let symbols: Vec<String> = if args.is_empty() {
        "BTCUSDT ETHUSDT LINKUSDT UNIUSDT AAVEUSDT ONDOUSDT PEPEUSDT SHIBUSDT MKRUSDT COMPUSDT KNCUSDT CVXUSDT CRVUSDT ENAUSDT INJUSDT GALAUSDT LDOUSDT BLURUSDT SNXUSDT PENDLEUSDT ARBUSDT CHZUSDT SANDUSDT MANAUSDT WLDUSDT CYBERUSDT COWUSDT STGUSDT EULUSDT OGNUSDT"
            .split(' ').map(|s| s.to_string()).collect()
    } else { args };

    // для кожного горизонту: (signal, fwd_ret)
    let mut data: Vec<Vec<(f64, f64)>> = horizons.iter().map(|_| vec![]).collect();
    let mut toks = 0;
    for sym in &symbols {
        let Some(v) = klines(sym, &interval) else { eprintln!("{sym}: нема відповіді"); continue };
        let Some(arr) = v.as_array() else { continue };
        // (close, signal)
        let mut rows: Vec<(f64, f64)> = vec![];
        for k in arr {
            let Some(a) = k.as_array() else { continue };
            if a.len() < 11 { continue }
            let close: f64 = a[4].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            let qv: f64 = a[7].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            let tb: f64 = a[10].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            let sig = if qv > 0.0 { (2.0 * tb - qv) / qv } else { 0.0 }; // (buy-sell)/vol
            rows.push((close, sig));
        }
        if rows.len() < 50 { continue }
        toks += 1;
        for (hi, &h) in horizons.iter().enumerate() {
            for i in 0..rows.len().saturating_sub(h) {
                let (c0, sig) = rows[i];
                let ch = rows[i + h].0;
                if c0 > 0.0 { data[hi].push((sig, ch / c0 - 1.0)); }
            }
        }
    }

    println!("── ЧЕСНИЙ БЕКТЕСТ: CEX taker-потік → ціна (interval={interval}) ──");
    println!("токенів: {toks} · сигнал = (takerBuy−Sell)/обсяг · out-of-sample klines (~1000 свічок/токен)");
    println!("КОНТРАРІАН-стратегія: шорт коли купують (sig>0) / лонг коли продають (sig<0); pnl/період + t-стат.\n");
    println!("{:>8} {:>9} {:>9} {:>8} {:>16} {:>8}", "гориз.", "n", "hit-rate", "AUC", "контр.pnl/період", "t-стат");
    for (hi, &h) in horizons.iter().enumerate() {
        let d = &data[hi];
        if d.is_empty() { continue }
        let n = d.len();
        let hit = d.iter().filter(|(s, r)| (*s > 0.0 && *r > 0.0) || (*s < 0.0 && *r < 0.0)).count() as f64 / n as f64;
        let (a, _, _) = auc(d.iter().map(|(s, r)| (*s, *r > 0.0)).collect());
        // контраріан pnl: позиція = -sign(sig), pnl = pos*ret
        let pnls: Vec<f64> = d.iter().map(|(s, r)| -(if *s > 0.0 { 1.0 } else if *s < 0.0 { -1.0 } else { 0.0 }) * r).collect();
        let m = pnls.iter().sum::<f64>() / n as f64;
        let sd = (pnls.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n as f64).sqrt();
        let t = if sd > 0.0 { m / (sd / (n as f64).sqrt()) } else { 0.0 };
        let sig_mark = if t.abs() >= 2.0 { "*" } else { " " };
        println!("{:>5}{} {:>9} {:>8.1}% {:>8.3} {:>14.3}%{} {:>+8.2}",
            h, unit, n, hit * 100.0, a, m * 100.0, sig_mark, t);
    }
    println!("\nЧИТАТИ: AUC<0.45 = контраріан-едж; «контр.pnl/період» — середня дохідність контраріан-стратегії за період;");
    println!("t-стат |t|≥2 (*) = статистично значуще ≠0. Без * — навіть якщо pnl>0, це шум (не торгувати).");
    println!("ЗАСТЕРЕЖЕННЯ: без комісій/прослизання; реальна торгівля з'їсть малий едж. Це brutto-верхня межа.");
}
