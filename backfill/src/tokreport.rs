//! tokreport — зведений звіт по токенах: емісія, вплив на ціну, мейкерська частка, походження.
//!
//! Збирає в одну таблицю те, що рахувалось окремими бінами:
//!   * `net6k-real.json`  — РЕАЛЬНІ баланси (з `tokenbalance`) і частка емісії на них;
//!   * `hourimpact.json`  — чи рухається ціна після купівель (годинна роздільність);
//!   * `mmfind.json`      — яка частка обігу токена має мейкерський характер;
//!   * `tokalloc.json`    — токен куплений на ринку чи роздали алокацією.
//!
//! Чому саме реальні баланси: оцінка з потоків (входи мінус виходи) завищувала суму вчетверо
//! і давала неможливі частки емісії (WETH 66%, USDP 111%). Тут беруться лише підтверджені.
//!
//! Вихід: dataset/backfill/tokreport.json + таблиця. Args: ROOT. Env: TOP(40) MIN_USD(50000)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn load(p: &str) -> Option<Value> {
    File::open(p).ok().and_then(|f| serde_json::from_reader(BufReader::new(f)).ok())
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let top: usize = envn("TOP", 40);
    let min_usd: f64 = envn("MIN_USD", 50000.0);

    // 1) реальні баланси й емісія
    let mut real: HashMap<String, (String, f64, f64, Option<f64>, u64)> = HashMap::new();
    if let Some(v) = load(&format!("{bf}/net6k-real.json")) {
        for r in v["rows"].as_array().cloned().unwrap_or_default() {
            let c = r["token"].as_str().unwrap_or("").to_lowercase();
            real.insert(c, (
                r["symbol"].as_str().unwrap_or("").to_string(),
                r["realUsd"].as_f64().unwrap_or(0.0),
                r["realHeld"].as_f64().unwrap_or(0.0),
                r["emissionPct"].as_f64(),
                r["holders"].as_u64().unwrap_or(0),
            ));
        }
    }
    // 2) вплив на ціну (години)
    let mut imp: HashMap<String, (f64, f64, f64, u64)> = HashMap::new();
    if let Some(v) = load(&format!("{bf}/hourimpact.json")) {
        for r in v["rows"].as_array().cloned().unwrap_or_default() {
            imp.insert(r["token"].as_str().unwrap_or("").to_lowercase(), (
                r["pre6h"].as_f64().unwrap_or(0.0),
                r["post6h"].as_f64().unwrap_or(0.0),
                r["gap6h"].as_f64().unwrap_or(0.0),
                r["events"].as_u64().unwrap_or(0),
            ));
        }
    }
    // 3) мейкерська частка обігу
    let mut mm: HashMap<String, (f64, u64)> = HashMap::new();
    if let Some(v) = load(&format!("{bf}/mmfind.json")) {
        for r in v["share"].as_array().cloned().unwrap_or_default() {
            mm.insert(r["token"].as_str().unwrap_or("").to_lowercase(), (
                r["mmSharePct"].as_f64().unwrap_or(0.0),
                r["mmWallets"].as_u64().unwrap_or(0),
            ));
        }
    }
    // 4) походження позиції
    let mut alloc: HashMap<String, (String, f64)> = HashMap::new();
    if let Some(v) = load(&format!("{bf}/tokalloc.json")) {
        for r in v["rows"].as_array().cloned().unwrap_or_default() {
            alloc.insert(r["token"].as_str().unwrap_or("").to_lowercase(), (
                r["verdict"].as_str().unwrap_or("").to_string(),
                r["topSharePct"].as_f64().unwrap_or(0.0),
            ));
        }
    }
    eprintln!("[tokreport] реальних позицій {} · вплив {} · ММ {} · походження {}",
        real.len(), imp.len(), mm.len(), alloc.len());

    let mut rows: Vec<Value> = Vec::new();
    for (c, (sym, usd, held, em, holders)) in &real {
        if *usd < min_usd { continue }
        let (pre, post, gap, ev) = imp.get(c).copied().unwrap_or((f64::NAN, f64::NAN, f64::NAN, 0));
        let (mmshare, mmw) = mm.get(c).copied().unwrap_or((f64::NAN, 0));
        let (verdict, topsrc) = alloc.get(c).cloned().unwrap_or((String::new(), f64::NAN));
        rows.push(json!({
            "token": c, "symbol": sym, "usd": usd, "held": held,
            "emissionPct": em, "holders": holders,
            "pre6h": pre, "post6h": post, "gap6h": gap, "events": ev,
            "mmSharePct": mmshare, "mmWallets": mmw,
            "origin": verdict, "topSourcePct": topsrc,
        }));
    }

    // ГОЛОВНЕ СОРТУВАННЯ — за часткою емісії: питання «де вони найбільше важать»,
    // а не «де найбільше грошей». Токен на $2 млн із 20% емісії значить більше за
    // $50 млн у монеті, де це соті відсотка.
    rows.sort_by(|a, b| b["emissionPct"].as_f64().unwrap_or(-1.0)
        .partial_cmp(&a["emissionPct"].as_f64().unwrap_or(-1.0)).unwrap());

    eprintln!("\n=== ДЕ ГРУПА ТРИМАЄ НАЙБІЛЬШУ ЧАСТКУ ЕМІСІЇ ===");
    eprintln!("{:<10} {:>13} {:>9} {:>7} {:>9} {:>8} {:>8}",
        "ТОКЕН", "СУМА $", "% ЕМІСІЇ", "ХОЛД", "ММ-обіг%", "-6г ДО", "+6г ПІСЛЯ");
    for r in rows.iter().take(top) {
        let em = r["emissionPct"].as_f64().map(|x| format!("{x:.2}%")).unwrap_or("?".into());
        let ms = r["mmSharePct"].as_f64().filter(|x| !x.is_nan())
            .map(|x| format!("{x:.0}%")).unwrap_or("-".into());
        let pre = r["pre6h"].as_f64().filter(|x| !x.is_nan())
            .map(|x| format!("{x:+.2}%")).unwrap_or("-".into());
        let post = r["post6h"].as_f64().filter(|x| !x.is_nan())
            .map(|x| format!("{x:+.2}%")).unwrap_or("-".into());
        eprintln!("{:<10} {:>13.0} {:>9} {:>7} {:>9} {:>8} {:>8}",
            r["symbol"].as_str().unwrap_or(""), r["usd"].as_f64().unwrap_or(0.0),
            em, r["holders"].as_u64().unwrap_or(0), ms, pre, post);
    }

    // де вплив на ціну ПІДТВЕРДЖУЄТЬСЯ: рух після купівлі більший за рух до неї
    let mut infl: Vec<&Value> = rows.iter()
        .filter(|r| r["gap6h"].as_f64().map(|g| !g.is_nan() && g > 0.0).unwrap_or(false))
        .collect();
    infl.sort_by(|a, b| b["gap6h"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["gap6h"].as_f64().unwrap_or(0.0)).unwrap());
    eprintln!("\n=== ДЕ Є ОЗНАКИ ВПЛИВУ НА ЦІНУ (рух після > руху до) ===");
    if infl.is_empty() {
        eprintln!("  жодного токена з додатною різницею — вплив не підтверджується");
    }
    for r in infl.iter().take(15) {
        eprintln!("  {:<10} різниця {:+.3}% · подій {} · {}% емісії · ${:.0}",
            r["symbol"].as_str().unwrap_or(""), r["gap6h"].as_f64().unwrap_or(0.0),
            r["events"].as_u64().unwrap_or(0),
            r["emissionPct"].as_f64().map(|x| format!("{x:.2}")).unwrap_or("?".into()),
            r["usd"].as_f64().unwrap_or(0.0));
    }

    let outp = format!("{bf}/tokreport.json");
    let _ = File::create(&outp).unwrap()
        .write_all(serde_json::to_string(&json!({"rows": rows})).unwrap().as_bytes());
    eprintln!("\n[tokreport] позицій у звіті: {} · збережено {}", rows.len(), outp);
}
