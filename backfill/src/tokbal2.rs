//! tokbal2 — РЕАЛЬНІ баланси токенів через Etherscan (`action=tokenbalance`, безкоштовно, попарно).
//! Blockscout ріже за лімітом («Too many requests»), тож беремо не всі позиції, а НАЙБІЛЬШІ:
//! рахуємо flow-net оцінку по transfers.csv, ранжуємо за $ і перевіряємо топ-MAXPAIRS.
//! Питання, на яке відповідаємо: наскільки бреше токенна оцінка (з ETH розрив був до ×300000).
//! Вихід: dataset/backfill/token-balances-real.json {pairs:[{w,tok,sym,est,real,ratio,usdEst,usdReal}], summary}
//! Args: ROOT [WALLET_FILE]. Env: MAXPAIRS(2500) MIN_USD(1000) CONC(8)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{root}/dataset/backfill/net-members.txt"));
    let maxpairs: usize = std::env::var("MAXPAIRS").ok().and_then(|s| s.parse().ok()).unwrap_or(2500);
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(1000.0);
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let bf = format!("{root}/dataset/backfill");
    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty());

    let members: HashSet<String> = fs::read_to_string(&wfile).expect("wallets").lines()
        .map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 42).collect();
    // ціни
    let tp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices.json")).expect("token-prices"))).unwrap();
    let mut px: HashMap<String, (f64, String)> = tp["tokens"].as_object().map(|m| m.iter()
        .filter_map(|(k, v)| v["usd"].as_f64().map(|u| (k.to_lowercase(), (u, v["symbol"].as_str().unwrap_or("").to_string())))).collect()).unwrap_or_default();
    if let Ok(oc) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())) {
        for (k, v) in &oc { let k = k.to_lowercase();
            if px.contains_key(&k) { continue; }
            if let Some(u) = v["usd"].as_f64() { if u > 0.0 && u < 10_000.0 { px.insert(k, (u, String::new())); } } }
    }
    // flow-net оцінка по членах
    let mut net: HashMap<(String, String), f64> = HashMap::new();
    let mut dec: HashMap<String, i32> = HashMap::new();
    if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
        for (c, v) in &sv { dec.insert(c.to_lowercase(), v["decimals"].as_u64().unwrap_or(18) as i32); }
    }
    for line in BufReader::with_capacity(1 << 20, File::open(format!("{bf}/transfers.csv")).expect("transfers")).lines().map_while(Result::ok) {
        let mut it = line.splitn(6, ',');
        let w = it.next().unwrap_or(""); let _c = it.next(); let d = it.next().unwrap_or(""); let _t = it.next(); let tok = it.next().unwrap_or("");
        if w == "wallet" || (d != "i" && d != "o") { continue; }
        let wl = w.to_lowercase(); if !members.contains(&wl) { continue; }
        let tl = tok.to_lowercase(); if !px.contains_key(&tl) { continue; }
        let amt: f64 = line.rsplitn(4, ',').nth(1).unwrap_or("0").parse().unwrap_or(0.0);
        if amt <= 0.0 { continue; }
        *net.entry((wl, tl)).or_insert(0.0) += if d == "i" { amt } else { -amt };
    }
    let mut pairs: Vec<((String, String), f64, f64)> = net.into_iter()
        .filter(|(_, v)| *v > 0.0)
        .map(|(k, v)| { let usd = v * px.get(&k.1).map(|p| p.0).unwrap_or(0.0); (k, v, usd) })
        .filter(|(_, _, usd)| *usd >= min_usd).collect();
    pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    pairs.truncate(maxpairs);
    eprintln!("[tokbal2] пар до перевірки: {} (поріг ${min_usd:.0}, оцінка разом ${:.0})", pairs.len(), pairs.iter().map(|p| p.2).sum::<f64>());

    let client = Arc::new(reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let keys = Arc::new(keys); let kc = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(conc));
    let res: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let prog = Arc::new(AtomicUsize::new(0));
    let total = pairs.len();
    let px = Arc::new(px); let dec = Arc::new(dec);

    let mut hs = Vec::new();
    for ((w, t), est, usd_est) in pairs {
        let (client, keys, kc, sem, res, prog, px, dec) = (client.clone(), keys.clone(), kc.clone(), sem.clone(), res.clone(), prog.clone(), px.clone(), dec.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut real_raw: Option<f64> = None;
            for _ in 0..4 {
                let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=tokenbalance&contractaddress={t}&address={w}&tag=latest&apikey={key}");
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        if v["status"] == "1" { if let Some(s) = v["result"].as_str() { if let Ok(x) = s.parse::<f64>() { real_raw = Some(x); break; } } }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
            if let Some(raw) = real_raw {
                let d = dec.get(&t).copied().unwrap_or(18);
                let real = raw / 10f64.powi(d);
                let (p, sym) = px.get(&t).cloned().unwrap_or((0.0, String::new()));
                res.lock().unwrap().push(json!({"w": w, "tok": t, "sym": sym, "est": est, "real": real,
                    "ratio": if real > 0.0 { est / real } else { f64::INFINITY }, "usdEst": usd_est, "usdReal": real * p}));
            }
            let n = prog.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 250 == 0 || n == total { eprintln!("[tokbal2] {n}/{total}"); }
        }));
    }
    for h in hs { let _ = h.await; }

    let mut r = res.lock().unwrap().clone();
    r.sort_by(|a, b| b["usdEst"].as_f64().unwrap_or(0.0).partial_cmp(&a["usdEst"].as_f64().unwrap_or(0.0)).unwrap());
    let (sum_est, sum_real): (f64, f64) = (r.iter().map(|x| x["usdEst"].as_f64().unwrap_or(0.0)).sum(), r.iter().map(|x| x["usdReal"].as_f64().unwrap_or(0.0)).sum());
    let zero = r.iter().filter(|x| x["real"].as_f64().unwrap_or(0.0) <= 0.0).count();
    let over10 = r.iter().filter(|x| x["ratio"].as_f64().unwrap_or(0.0) > 10.0).count();
    eprintln!("[tokbal2] ===== ОЦІНКА vs РЕАЛЬНИЙ БАЛАНС =====");
    eprintln!("[tokbal2] перевірено пар {} · оцінка ${:.0} · реально ${:.0} ({:.1}% від оцінки)",
        r.len(), sum_est, sum_real, if sum_est > 0.0 { 100.0 * sum_real / sum_est } else { 0.0 });
    eprintln!("[tokbal2] позицій, яких НЕМА зовсім: {} ({:.0}%) · завищених >10×: {}", zero, 100.0 * zero as f64 / r.len().max(1) as f64, over10);
    for x in r.iter().take(10) {
        eprintln!("     {:<8} оцінка {:>14.0} факт {:>14.0} · ${:>12.0} → ${:>12.0}  {}",
            x["sym"].as_str().unwrap_or("?"), x["est"].as_f64().unwrap_or(0.0), x["real"].as_f64().unwrap_or(0.0),
            x["usdEst"].as_f64().unwrap_or(0.0), x["usdReal"].as_f64().unwrap_or(0.0), &x["w"].as_str().unwrap_or("")[0..12]);
    }
    fs::write(format!("{bf}/token-balances-real.json"), serde_json::to_string_pretty(&json!({
        "checked": r.len(), "usdEstimate": sum_est, "usdReal": sum_real, "zeroPositions": zero, "over10x": over10, "pairs": r})).unwrap()).ok();
    eprintln!("[tokbal2] збережено token-balances-real.json");
}
