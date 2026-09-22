//! realbal — РЕАЛЬНІ баланси замість оцінки з потоків, і перерахунок часток емісії.
//!
//! Навіщо: баланс, обчислений як «сума входів мінус виходів» по зібраній історії,
//! систематично бреше — історія неповна, тож частина виходів відсутня, і залишок
//! завищується. Звірка показала розрив до ×11 (genesis-адреса UNI: 147 млн проти
//! реальних 12.5 млн) і випадки «наш 1.9 млн проти реальних 0». Через це в таблиці
//! з'являлись неможливі числа: WETH 66% усієї емісії, USDP 111%, CAMP 77%.
//!
//! Тут кожна пара (гаманець, токен) перевіряється через Etherscan `tokenbalance`, і всі
//! підсумки будуються ЛИШЕ на підтверджених балансах.
//!
//! Вхід:  dataset/backfill/net6k-pairs.jsonl (з net6k), token-prices.json, token-supply.json
//! Вихід: dataset/backfill/net6k-real.json + таблиця
//! Args: ROOT. Env: CONC(8) TOP(40)
use backfill::known::pat;
use backfill::patterns::{Finding, PatternDb};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn envv(root: &str, k: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let conc: usize = envn("CONC", 8);
    let top: usize = envn("TOP", 40);

    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());

    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).unwrap())).unwrap();
    let mut price: HashMap<String, f64> = HashMap::new();
    if let Some(o) = cur["tokens"].as_object() {
        for (c, v) in o { price.insert(c.to_lowercase(), v["usd"].as_f64().unwrap_or(0.0)); }
    }
    let mut supply: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-supply.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (c, v) in m { if let Some(s) = v["supply"].as_f64() { supply.insert(c.to_lowercase(), s); } }
        }
    }
    if let Ok(f) = File::open(format!("{bf}/tokenpnl.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(d) = serde_json::from_str::<Value>(&line) {
                let c = d["contract"].as_str().unwrap_or("").to_lowercase();
                if let Some(s) = d["supply"].as_f64() { if s > 0.0 { supply.entry(c).or_insert(s); } }
            }
        }
    }

    // decimals потрібні, щоб перевести сирий баланс у токени
    let mut decs: HashMap<String, i32> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-supply.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (c, v) in m { if let Some(d) = v["decimals"].as_i64() { decs.insert(c.to_lowercase(), d as i32); } }
        }
    }

    // КОНТРАКТИ ГЕТЬ. Перевірка коду по всьому списку членів показала: 718 із 6864 (10.5%) —
    // смарт-контракти, і серед них сховища протоколів. Balancer Vault
    // `0xba12222222228d8ba…` тримав 31.74 млн BAL із 31.90 млн, приписаних групі, — тобто
    // «44% емісії BAL» було балансом протоколу, а не позицією. Такі адреси спотворюють
    // частку емісії скрізь, де потрапляють у холдери.
    let mut is_contract: HashMap<String, bool> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/contract-flags.json")) {
        if let Ok(m) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (a, v) in m { if let Some(b) = v.as_bool() { is_contract.insert(a.to_lowercase(), b); } }
        }
    }
    let skip_contracts = std::env::var("KEEP_CONTRACTS").map(|v| v != "1").unwrap_or(true);
    eprintln!("[realbal] прапорців коду: {} · контракти {}",
        is_contract.len(), if skip_contracts { "ВІДКИДАЄМО" } else { "лишаємо" });

    let mut pairs: Vec<(String, String, f64, String)> = Vec::new();
    let mut skipped_contract = 0usize;
    for line in BufReader::new(File::open(format!("{bf}/net6k-pairs.jsonl")).expect("нема net6k-pairs.jsonl"))
        .lines().map_while(Result::ok)
    {
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            let w = v["w"].as_str().unwrap_or("").to_string();
            if skip_contracts && is_contract.get(&w) == Some(&true) {
                skipped_contract += 1;
                continue;
            }
            pairs.push((
                w,
                v["tok"].as_str().unwrap_or("").to_string(),
                v["est"].as_f64().unwrap_or(0.0),
                v["sym"].as_str().unwrap_or("").to_string(),
            ));
        }
    }
    eprintln!("[realbal] пар до перевірки: {} · відкинуто контрактів: {}",
        pairs.len(), skipped_contract);

    let client = Arc::new(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let keys = Arc::new(keys);
    let kc = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(conc));
    let res: Arc<Mutex<Vec<(String, String, f64, f64, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let prog = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let total = pairs.len();

    let mut hs = Vec::new();
    for (w, tok, est, sym) in pairs {
        let (client, keys, kc, sem, res, prog, failed) =
            (client.clone(), keys.clone(), kc.clone(), sem.clone(), res.clone(), prog.clone(), failed.clone());
        let dec = decs.get(&tok).copied().unwrap_or(18);
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut got = None;
            for attempt in 0..6u64 {
                let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=tokenbalance&contractaddress={tok}&address={w}&tag=latest&apikey={key}");
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        let msg = v["message"].as_str().unwrap_or("");
                        let s = v["result"].as_str().unwrap_or("");
                        // ліміт приходить як HTTP 200 з NOTOK; без цієї перевірки він
                        // зарахувався б як «баланс нуль»
                        let limited = msg == "NOTOK" || s.contains("rate limit")
                            || s.contains("Max calls") || s.contains("Too many");
                        if !limited {
                            if let Ok(raw) = s.parse::<f64>() {
                                got = Some(raw / 10f64.powi(dec));
                                break;
                            }
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(300 * (1u64 << attempt.min(5)))).await;
            }
            match got {
                Some(real) => res.lock().unwrap().push((w, tok, est, real, sym)),
                None => { failed.fetch_add(1, Ordering::Relaxed); }
            }
            let n = prog.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 100 == 0 || n == total { eprintln!("[realbal] {n}/{total}"); }
        }));
    }
    for h in hs { let _ = h.await; }

    let rows = res.lock().unwrap().clone();
    let nfail = failed.load(Ordering::Relaxed);
    eprintln!("[realbal] перевірено {} · не вдалось {}", rows.len(), nfail);

    // Кожен перевірений баланс коштував запиту з квоти (100k/добу на ключ), а жив лише
    // в пам'яті процесу: наступний аналіз мусив питати Etherscan заново. Тепер пари
    // лягають на диск, а сам факт володіння — у накопичувальну базу патернів.
    {
        let mut w = std::io::BufWriter::new(
            File::create(format!("{bf}/net6k-real-pairs.jsonl")).expect("real-pairs"));
        for (a, tok, est, real, sym) in &rows {
            let _ = writeln!(w, "{}", json!({"w": a, "tok": tok, "sym": sym, "real": real, "est": est}));
        }
        eprintln!("[realbal] збережено пар: {}", rows.len());
    }
    {
        let mut db = PatternDb::load(&bf);
        let mut n = 0usize;
        for (a, tok, _est, real, sym) in &rows {
            if *real <= 0.0 { continue }
            let usd = real * price.get(tok).copied().unwrap_or(0.0);
            if usd < 10_000.0 { continue }   // дрібниця не є позицією
            db.add(a, Finding::new(pat::HOLDER, "realbal").evs("sym", sym).ev("usd", usd));
            n += 1;
        }
        eprintln!("[realbal] у базу патернів: {n} позицій >=$10k", n = n);
        db.save(&bf);
    }

    // агрегація по токенах — ЛИШЕ на підтверджених балансах
    let mut agg: HashMap<String, (f64, f64, u32, String)> = HashMap::new();  // tok -> (real, est, holders, sym)
    for (_, tok, est, real, sym) in &rows {
        let e = agg.entry(tok.clone()).or_insert((0.0, 0.0, 0, sym.clone()));
        e.0 += *real; e.1 += *est;
        if *real > 0.0 { e.2 += 1 }
    }

    let mut out: Vec<Value> = Vec::new();
    for (tok, (real, est, holders, sym)) in &agg {
        let p = price.get(tok).copied().unwrap_or(0.0);
        let sup = supply.get(tok).copied();
        out.push(json!({
            "token": tok, "symbol": sym,
            "realHeld": real, "estHeld": est,
            "realUsd": real * p, "estUsd": est * p,
            "ratio": if *est > 0.0 { real / est } else { 0.0 },
            "holders": holders,
            "supply": sup,
            "emissionPct": sup.map(|s| 100.0 * real / s),
        }));
    }
    out.sort_by(|a, b| b["realUsd"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["realUsd"].as_f64().unwrap_or(0.0)).unwrap());

    let real_total: f64 = out.iter().filter_map(|r| r["realUsd"].as_f64()).sum();
    let est_total: f64 = out.iter().filter_map(|r| r["estUsd"].as_f64()).sum();

    let outp = format!("{bf}/net6k-real.json");
    let _ = File::create(&outp).unwrap().write_all(
        serde_json::to_string(&json!({"checked": rows.len(), "failed": nfail,
            "realUsdTotal": real_total, "estUsdTotal": est_total, "rows": out})).unwrap().as_bytes());

    eprintln!("\n=== ПОЗИЦІЇ НА РЕАЛЬНИХ БАЛАНСАХ ===");
    eprintln!("оцінка з потоків: ${:.0} · ПІДТВЕРДЖЕНО: ${:.0} ({:.1}% від оцінки)",
        est_total, real_total, 100.0 * real_total / est_total.max(1e-9));
    eprintln!("\n{:<10} {:>15} {:>15} {:>7} {:>9} {:>6}", "ТОКЕН", "РЕАЛЬНО $", "ОЦІНКА $", "РЕАЛ/ОЦ", "% ЕМІСІЇ", "ХОЛД");
    for r in out.iter().take(top) {
        let em = r["emissionPct"].as_f64().map(|x| format!("{x:.2}%")).unwrap_or("?".into());
        eprintln!("{:<10} {:>15.0} {:>15.0} {:>6.2}x {:>9} {:>6}",
            r["symbol"].as_str().unwrap_or(""), r["realUsd"].as_f64().unwrap_or(0.0),
            r["estUsd"].as_f64().unwrap_or(0.0), r["ratio"].as_f64().unwrap_or(0.0),
            em, r["holders"].as_u64().unwrap_or(0));
    }
    eprintln!("\n[realbal] збережено {outp}");
}
