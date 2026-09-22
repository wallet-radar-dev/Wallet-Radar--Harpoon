//! supplyat — ЕМІСІЯ НА ДАТУ, відновлена ходою назад по мінтах і спаленнях.
//!
//! Навіщо: «частка емісії» за минулий цикл від СЬОГОДНІШНЬОГО supply — брехня в обидва боки.
//! MKR: сьогодні 88 275 (більшість конвертовано в SKY), у травні 2021 було ~1 005 577 —
//! позиція 18 238 монет дає 20.6% від сьогоднішнього обігу і лише ~1.8% від тодішнього.
//! Помилка в 11 разів, і вона МОВЧАЗНА: цифра виглядає правдоподібно.
//!
//! Чому не архівний виклик: безкоштовний Etherscan ігнорує номер блока в eth_call і віддає
//! ПОТОЧНИЙ стан (перевірено: totalSupply на блоці 12400000 = totalSupply на latest), а
//! account/balancehistory відповідає «API Pro». Тобто «архівна» відповідь тут — тихий підлог.
//! CoinGecko безкоштовно теж не дає: історія обрізана 365 днями.
//!
//! Метод (точний і безкоштовний): емісія змінюється лише мінтами й спаленнями, а це перекази
//! з/на нульову адресу. Беремо всі такі перекази ПІСЛЯ потрібної дати й віднімаємо:
//!     supply(дата) = supply(сьогодні) − намінтовано_після + спалено_після
//! Якщо подій більше, ніж Etherscan узагалі віддає (стеля 10 000 записів), значення НЕ
//! пишеться, а пара позначається truncated — краще без цифри, ніж із заниженою.
//! Обмеження: токени з ребейзом (aUSDT, cDAI) міняють баланси без подій Transfer — для них
//! метод не працює, і вони мають відсіюватись як обгортки на боці аналізу.
//!
//! Вхід: PAIRS_FILE — рядки `контракт,YYYY-MM-DD`.
//! Вихід: dataset/backfill/token-supply-at.json {"контракт|дата": {supply, minted, burned, events, truncated}}
//! Args: ROOT. Env: PAIRS_FILE, SLEEP_MS(250), FORCE(0), MAX_PAGES(10)

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

fn envs(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| d.to_string())
}
fn envn(k: &str, d: u64) -> u64 { envs(k, "").parse().unwrap_or(d) }
fn dotenv(root: &str, k: &str) -> String {
    // Ключі переїхали з .env у deploy/backfill.env (19.08). Читаємо обидва місця:
    // спершу нове, потім старе — інакше після переїзду бін мовчки лишається без ключів
    // і падає вже в роботі, а не на старті.
    for p in [format!("{root}/deploy/backfill.env"), format!("{root}/.env")] {
        let v = std::fs::read_to_string(&p).unwrap_or_default().lines()
            .find(|l| l.starts_with(&format!("{k}=")))
            .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
            .unwrap_or_default();
        if !v.is_empty() { return v; }
    }
    String::new()
}
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
/// Кінець доби UTC: стан на ЗАВЕРШЕННЯ дня.
fn ts_of(date: &str) -> Option<u64> {
    if date.len() < 10 { return None; }
    let y: i64 = date[0..4].parse().ok()?;
    let m: i64 = date[5..7].parse().ok()?;
    let d: i64 = date[8..10].parse().ok()?;
    Some((days_from_civil(y, m, d) * 86400 + 86399) as u64)
}
/// Ліміт Etherscan приходить як HTTP 200 з NOTOK у ТІЛІ — ловимо саме там.
fn limited(v: &Value) -> bool {
    let msg = v["message"].as_str().unwrap_or("");
    let res = v["result"].as_str().unwrap_or("");
    msg == "NOTOK" || res.contains("rate limit") || res.contains("Max calls") || res.contains("Too many")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".to_string());
    let bf = format!("{root}/dataset/backfill");
    let sleep_ms = envn("SLEEP_MS", 250);
    let force = envs("FORCE", "0") == "1";
    let max_pages = envn("MAX_PAGES", 10);

    let keys: Vec<String> = dotenv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");

    // сьогоднішня емісія — точка відліку ходи назад
    let mut today: HashMap<String, f64> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/token-supply.json")) {
        if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)) {
            for (k, x) in v {
                if let Some(s) = x["supply"].as_f64() { today.insert(k.to_lowercase(), s); }
            }
        }
    }
    let mut out: serde_json::Map<String, Value> = File::open(format!("{bf}/token-supply-at.json"))
        .ok()
        .and_then(|f| serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)).ok())
        .unwrap_or_default();

    let pf = envs("PAIRS_FILE", "");
    let mut pairs: Vec<(String, String)> = vec![];
    for l in std::fs::read_to_string(&pf).expect("PAIRS_FILE").lines() {
        let mut p = l.trim().split(',');
        if let (Some(c), Some(d)) = (p.next(), p.next()) {
            let c = c.trim().to_lowercase();
            if c.starts_with("0x") { pairs.push((c, d.trim().to_string())); }
        }
    }
    pairs.sort();
    pairs.dedup();
    let before = pairs.len();
    if !force { pairs.retain(|(c, d)| !out.contains_key(&format!("{c}|{d}"))); }
    eprintln!("[supplyat] цілей {} (вже знаємо {})", pairs.len(), before - pairs.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45)).build().unwrap();
    let mut blocks: HashMap<String, u64> = HashMap::new();
    let (mut ok, mut trunc, mut miss) = (0usize, 0usize, 0usize);

    for (i, (c, date)) in pairs.iter().enumerate() {
        let Some(&sup_now) = today.get(c) else { miss += 1; continue };
        let key = |n: usize| &keys[n % keys.len()];

        // блок на дату (дат мало — кешуємо)
        let blk = match blocks.get(date) {
            Some(&b) => b,
            None => {
                let Some(ts) = ts_of(date) else { miss += 1; continue };
                let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=block&action=getblocknobytime&timestamp={ts}&closest=before&apikey={}", key(i));
                let mut got = 0u64;
                for a in 0..5u64 {
                    if let Ok(r) = client.get(&url).send().await {
                        if let Ok(v) = r.json::<Value>().await {
                            if !limited(&v) {
                                if let Some(b) = v["result"].as_str().and_then(|s| s.parse::<u64>().ok()) { got = b; break; }
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(400 * (1 << a.min(4)))).await;
                }
                if got == 0 { miss += 1; continue }
                blocks.insert(date.clone(), got);
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
                got
            }
        };

        // Чи індексує Etherscan перекази з нульовою адресою для ЦЬОГО токена.
        // Без цієї перевірки «подій після дати нема» означало б «емісія не змінювалась»,
        // хоч насправді означає «ми їх не бачимо»: BNT і LRC віддають нуль подій за ВСЮ
        // історію, хоч Bancor карбує BNT постійно. Мовчазний нуль = найгірший вид помилки.
        // Нульова адреса підставляється в URL через inline-формат, тож потрібне
        // локальне імʼя: Rust бере його зі scope прямо всередині рядка.
        let zero = wr_store::addrs::zero();
        let mut indexed = false;
        {
            let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=tokentx&contractaddress={c}&address={zero}&startblock=0&endblock=99999999&page=1&offset=100&sort=asc&apikey={}", key(i));
            for a in 0..5u64 {
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        if !limited(&v) {
                            indexed = !v["result"].as_array().map(|x| x.is_empty()).unwrap_or(true);
                            break;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(500 * (1 << a.min(5)))).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        }

        // усі мінти й спалення ПІСЛЯ цього блока
        let (mut minted, mut burned, mut events) = (0f64, 0f64, 0u64);
        let mut truncated = false;
        let mut page = 1u64;
        let mut failed = false;
        loop {
            let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=tokentx&contractaddress={c}&address={zero}&startblock={}&endblock=99999999&page={page}&offset=1000&sort=asc&apikey={}", blk + 1, key(i + page as usize));
            let mut rows: Option<Vec<Value>> = None;
            for a in 0..6u64 {
                if let Ok(r) = client.get(&url).send().await {
                    if let Ok(v) = r.json::<Value>().await {
                        if limited(&v) {
                            tokio::time::sleep(std::time::Duration::from_millis(500 * (1 << a.min(5)))).await;
                            continue;
                        }
                        // «No transactions found» — це порожньо, а не помилка
                        rows = Some(v["result"].as_array().cloned().unwrap_or_default());
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(500 * (1 << a.min(5)))).await;
            }
            let Some(rows) = rows else { failed = true; break };
            let n = rows.len();
            for r in &rows {
                let dec: i32 = r["tokenDecimal"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
                let raw: f64 = r["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let amt = raw / 10f64.powi(dec);
                let from = r["from"].as_str().unwrap_or("");
                let to = r["to"].as_str().unwrap_or("");
                if from == wr_store::addrs::zero() { minted += amt } else if to == wr_store::addrs::zero() { burned += amt }
                events += 1;
            }
            if n < 1000 { break }
            page += 1;
            // стеля Etherscan: більше 10 сторінок по 1000 воно однаково не віддасть
            if page > max_pages { truncated = true; break }
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        }

        if failed { miss += 1; }
        else if truncated {
            trunc += 1;
            out.insert(format!("{c}|{date}"), json!({
                "truncated": true, "events": events,
                "note": "подій більше за стелю вибірки — значення не рахуємо, щоб не занизити"
            }));
        } else {
            let sup = sup_now - minted + burned;
            if sup > 0.0 {
                out.insert(format!("{c}|{date}"), json!({
                    "supply": sup, "supplyToday": sup_now, "minted": minted, "burned": burned,
                    "events": events, "indexed": indexed, "truncated": false, "block": blk,
                    "method": "хода назад по мінтах/спаленнях від сьогоднішнього supply"
                }));
                ok += 1;
            } else { miss += 1 }
        }
        if (i + 1) % 20 == 0 {
            eprintln!("[supplyat] {}/{} · порахованих {ok}, обрізаних {trunc}, без даних {miss}", i + 1, pairs.len());
        }
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
    }

    std::fs::write(format!("{bf}/token-supply-at.json"), serde_json::to_string(&out).unwrap()).ok();
    eprintln!("[supplyat] ГОТОВО: у файлі {} записів (пораховано {ok}, обрізано {trunc}, без даних {miss})", out.len());
}
