//! toksupply — добирає ЗАГАЛЬНУ ЕМІСІЮ (totalSupply) для токенів, у яких її ще нема.
//!
//! Навіщо: частку емісії можна порахувати лише там, де відомий supply. У `tokenpnl.jsonl`
//! він є для 581 токена, але серед позицій групи 134 токени з ціною лишались без нього —
//! зокрема WETH ($2.88 млрд), WBTC, MKR, wstETH. Без цього «яку частку емісії тримає група»
//! неможливо відповісти саме там, де сума найбільша.
//!
//! Джерела: supply — Etherscan `stats/tokensupply` (сирі одиниці); decimals — DefiLlama
//! `/prices/current` (вона їх віддає разом із ціною). Ділити одне на інше без decimals не можна:
//! у WBTC їх 8, у більшості 18, і помилка дає різницю в 10 порядків.
//!
//! Вихід: dataset/backfill/token-supply.json {контракт: {supply, decimals, symbol}}
//! Args: ROOT. Env: BATCH(40) SLEEP_MS(250)
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

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
    let batch: usize = envn("BATCH", 40);
    let sleep_ms: u64 = envn("SLEEP_MS", 250);

    let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");

    // що вже відомо — не перепитуємо
    let mut have: HashMap<String, Value> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-supply.json")) {
        if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (k, val) in v { have.insert(k.to_lowercase(), val); }
        }
    }

    // цілі: або явний список (TOKENS_FILE), або токени з позицією і ціною, але без supply.
    // FORCE=1 перепитує навіть уже відоме. Це потрібно, бо збережене значення може бути
    // хибним: MKR лежав як 88 466 при реальних сотнях тисяч, і будь-яка «частка емісії»,
    // порахована від такого знаменника, завищена в рази й мовчки.
    let tfile = std::env::var("TOKENS_FILE").unwrap_or_default();
    let force = std::env::var("FORCE").map(|v| v == "1").unwrap_or(false);
    let mut want: Vec<(String, String)> = Vec::new();
    if !tfile.is_empty() {
        for l in std::fs::read_to_string(&tfile).expect("TOKENS_FILE").lines() {
            let c = l.trim().to_lowercase();
            if c.starts_with("0x") && (force || !have.contains_key(&c)) {
                want.push((c, String::new()));
            }
        }
    } else {
        let pos: Value = serde_json::from_reader(BufReader::new(
            File::open(format!("{bf}/net6k-positions.json")).expect("спершу net6k"))).unwrap();
        for r in pos["rows"].as_array().cloned().unwrap_or_default() {
            if r["price"].as_f64().unwrap_or(0.0) <= 0.0 { continue }
            let c = r["token"].as_str().unwrap_or("").to_lowercase();
            if c.is_empty() || have.contains_key(&c) { continue }
            if r["supply"].as_f64().is_some() { continue }
            want.push((c, r["symbol"].as_str().unwrap_or("").to_string()));
        }
    }
    eprintln!("[toksupply] треба добрати: {} (вже маємо {})", want.len(), have.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40)).build().unwrap();

    // 1) decimals пачками з DefiLlama — вона віддає їх разом із ціною
    let mut decs: HashMap<String, i32> = HashMap::new();
    for chunk in want.chunks(batch) {
        let ids: Vec<String> = chunk.iter().map(|(c, _)| format!("ethereum:{c}")).collect();
        let url = format!("https://coins.llama.fi/prices/current/{}", ids.join(","));
        if let Ok(r) = client.get(&url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                if let Some(o) = v["coins"].as_object() {
                    for (k, c) in o {
                        if let Some(a) = k.split(':').nth(1) {
                            if let Some(d) = c["decimals"].as_i64() {
                                decs.insert(a.to_lowercase(), d as i32);
                            }
                        }
                    }
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
    }
    eprintln!("[toksupply] decimals отримано для {}", decs.len());

    // 2) supply по одному (Etherscan не має пакетного tokensupply)
    let mut out: serde_json::Map<String, Value> = have.into_iter().collect();
    let (mut ok, mut miss) = (0usize, 0usize);
    for (i, (c, sym)) in want.iter().enumerate() {
        let key = &keys[i % keys.len()];
        let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=stats&action=tokensupply&contractaddress={c}&apikey={key}");
        let mut got = false;
        for attempt in 0..5u64 {
            if let Ok(r) = client.get(&url).send().await {
                if let Ok(v) = r.json::<Value>().await {
                    let msg = v["message"].as_str().unwrap_or("");
                    let res = v["result"].as_str().unwrap_or("");
                    // ліміт приходить як HTTP 200 з NOTOK — ловимо в тілі, інакше «supply нема»
                    let limited = msg == "NOTOK" || res.contains("rate limit")
                        || res.contains("Max calls") || res.contains("Too many");
                    if !limited {
                        if let Ok(raw) = res.parse::<f64>() {
                            let d = decs.get(c).copied().unwrap_or(18);
                            let supply = raw / 10f64.powi(d);
                            if supply > 0.0 {
                                // символ зі списку не приходить — лишаємо вже відомий
                                let sym2 = if sym.is_empty() {
                                    out.get(c).and_then(|v| v["symbol"].as_str()).unwrap_or("").to_string()
                                } else { sym.clone() };
                                out.insert(c.clone(), json!({"supply": supply, "decimals": d, "symbol": sym2}));
                                ok += 1;
                            }
                            got = true;
                            break;
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(300 * (1u64 << attempt.min(5)))).await;
        }
        if !got { miss += 1 }
        if (i + 1) % 25 == 0 { eprintln!("[toksupply] {}/{} · зібрано {ok}", i + 1, want.len()); }
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
    }

    std::fs::write(format!("{bf}/token-supply.json"),
        serde_json::to_string(&out).unwrap()).ok();
    eprintln!("[toksupply] ГОТОВО: у файлі {} токенів (додано {ok}, не вдалось {miss})", out.len());
}
