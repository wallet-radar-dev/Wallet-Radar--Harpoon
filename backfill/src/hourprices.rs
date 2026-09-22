//! hourprices — ПОГОДИННІ ціни токенів (DefiLlama /chart, period=1h).
//!
//! Навіщо окремо від `histprices`: там глибина жорстко обрізана до 500 точок і вікна
//! рахуються в ДНЯХ. Для перевірки впливу на ціну всередині дня потрібна година, а 500 точок
//! по годині — це лише ~20 днів на запит, тож вікна й арифметика інші.
//!
//! Денна роздільність не бачить того, що робить маркет-мейкер: купівля і рух ціни можуть
//! вкластися в кілька годин, і на денному графіку це зіллється в одну свічку.
//!
//! Вихід: dataset/backfill/token-prices-hourly.json {"tokens":{контракт:{sym,prices:[[ts,p]…]}}}
//! Args: ROOT [TOKEN_FILE]. Env: DAYS(540) BATCH(8) SLEEP_MS(400) TOPN(25)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let days: u64 = envn("DAYS", 540);
    let batch: usize = envn("BATCH", 8);
    let sleep_ms: u64 = envn("SLEEP_MS", 400);
    let topn: usize = envn("TOPN", 25);

    // список токенів: або з файлу, або топ-N позицій групи за вартістю
    let mut toks: Vec<(String, String)> = Vec::new();
    if let Some(p) = std::env::args().nth(2) {
        for l in BufReader::new(File::open(&p).expect("token file")).lines().map_while(Result::ok) {
            let c = l.trim().to_lowercase();
            if c.len() == 42 { toks.push((c, String::new())) }
        }
    } else {
        let pos: Value = serde_json::from_reader(BufReader::new(
            File::open(format!("{bf}/net6k-positions.json")).expect("нема net6k-positions.json"))).unwrap();
        for r in pos["rows"].as_array().cloned().unwrap_or_default() {
            if r["price"].as_f64().unwrap_or(0.0) <= 0.0 { continue }
            let c = r["token"].as_str().unwrap_or("").to_lowercase();
            if c.len() == 42 { toks.push((c, r["symbol"].as_str().unwrap_or("").to_string())) }
            if toks.len() >= topn { break }
        }
    }
    eprintln!("[hourprices] токенів: {} · глибина {days} днів (погодинно)", toks.len());

    // 500 точок на запит; беремо 480 годин на вікно із запасом
    const WIN_H: u64 = 480;
    let total_h = days * 24;
    let n_win = total_h.div_ceil(WIN_H);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let start = now - total_h * 3600;
    eprintln!("[hourprices] вікон на пачку: {n_win}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60)).build().unwrap();
    let mut acc: HashMap<String, (String, Vec<(u64, f64)>)> = HashMap::new();

    for (bi, chunk) in toks.chunks(batch).enumerate() {
        let ids: Vec<String> = chunk.iter().map(|(c, _)| format!("ethereum:{c}")).collect();
        for wi in 0..n_win {
            let w_h = WIN_H.min(total_h - wi * WIN_H);
            let w_start = start + wi * WIN_H * 3600;
            let url = format!("https://coins.llama.fi/chart/{}?start={w_start}&span={w_h}&period=1h&searchWidth=30m",
                ids.join(","));
            for attempt in 0..3u64 {
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        if let Some(coins) = v["coins"].as_object() {
                            if !coins.is_empty() {
                                for (k, c) in coins {
                                    let Some(addr) = k.split(':').nth(1) else { continue };
                                    let e = acc.entry(addr.to_lowercase()).or_insert_with(||
                                        (c["symbol"].as_str().unwrap_or("").to_string(), Vec::new()));
                                    for p in c["prices"].as_array().cloned().unwrap_or_default() {
                                        if let (Some(t), Some(pr)) = (p["timestamp"].as_u64(), p["price"].as_f64()) {
                                            e.1.push((t, pr));
                                        }
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms * (attempt + 2))).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        }
        eprintln!("[hourprices] пачка {}/{} · токенів із рядом {}",
            bi + 1, toks.len().div_ceil(batch), acc.len());
    }

    let mut out: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut pts_total = 0usize;
    for (addr, (sym, mut pts)) in acc {
        pts.sort_by_key(|p| p.0);
        pts.dedup_by_key(|p| p.0);
        if pts.len() < 100 { continue }              // менше 100 годин — сенсу нема
        pts_total += pts.len();
        let prices: Vec<Value> = pts.iter().map(|(t, p)| json!([t, p])).collect();
        out.insert(addr, json!({"sym": sym, "prices": prices}));
    }
    std::fs::write(format!("{bf}/token-prices-hourly.json"),
        serde_json::to_string(&json!({"periodHours": 1, "days": days, "tokens": out})).unwrap()).ok();
    eprintln!("[hourprices] ГОТОВО: {} токенів, {} точок → token-prices-hourly.json",
        out.len(), pts_total);
}
