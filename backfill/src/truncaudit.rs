//! truncaudit — вибіркова перевірка: скільки гаманців зі статусом `done` насправді обірвані.
//!
//! Навіщо: у `backfill2` була діра — якщо в ОДНОМУ блоці понад 1000 переказів, наступна
//! сторінка повертала ті самі записи, всі відсіювались як дублі межового блока, лічильник
//! записаного ставав нулем — і краул позначав гаманець `done`, викидаючи решту історії.
//! Дефект виправлено, але 119 388 гаманців зібрано ще зі старим кодом, і з самого CSV
//! обрив не видно: там немає номера блока.
//!
//! Метод (дві ознаки, обидві дешеві):
//!   1. НАСИЧЕНИЙ БЛОК — питаємо ланцюг про останній блок гаманця (startblock=endblock=sb).
//!      Рівно 1000 записів означає, що блок упирається в сторінку, і саме там краул спинявся.
//!   2. ЩО ПІСЛЯ — беремо перші записи після sb і дивимось їхню дату. Якщо вони СТАРІШІ за
//!      день, коли гаманець позначено `done`, це втрачена історія, а не нова активність.
//!      Без цієї перевірки будь-який живий гаманець виглядав би обірваним просто тому, що
//!      після збору минув час.
//!
//! Вихід: dataset/backfill/truncaudit.json. Args: ROOT.
//! Env: SAMPLE(200) PROG(_progress-grp.csv) CONC(3) SLEEP_MS(250) DONE_DATE(2026-08-19)

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn envs(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| d.to_string())
}
fn envn(k: &str, d: usize) -> usize { envs(k, "").parse().unwrap_or(d) }
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
fn limited(v: &Value) -> bool {
    let msg = v["message"].as_str().unwrap_or("");
    let res = v["result"].as_str().unwrap_or("");
    msg == "NOTOK" || res.contains("rate limit") || res.contains("Max calls")
        || res.contains("Max daily") || res.contains("Too many")
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".to_string());
    let b = format!("{root}/dataset/backfill");
    let sample = envn("SAMPLE", 200);
    let prog = envs("PROG", "_progress-grp.csv");
    let conc = envn("CONC", 3);
    let sleep_ms = envn("SLEEP_MS", 250) as u64;
    let done_date = envs("DONE_DATE", "2026-08-19");

    let keys: Vec<String> = dotenv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");

    // останній стан кожного гаманця: статус + блок чекпойнта
    let mut last: HashMap<String, (String, u64)> = HashMap::new();
    let mut rows_of: HashMap<String, u64> = HashMap::new();
    let f = File::open(format!("{b}/{prog}")).expect("файл прогресу");
    for l in BufReader::new(f).lines().map_while(Result::ok) {
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 6 { continue }
        let sb = p[5].strip_prefix("es:sb=").and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
        let r: u64 = p[4].trim().parse().unwrap_or(0);
        let w = p[0].to_lowercase();
        let e = rows_of.entry(w.clone()).or_insert(0);
        if r > *e { *e = r }
        last.insert(w, (p[3].to_string(), sb));
    }
    let mut done: Vec<(String, u64)> = last.into_iter()
        .filter(|(_, (st, sb))| st == "done" && *sb > 0)
        .map(|(w, (_, sb))| (w, sb)).collect();
    done.sort();
    let total = done.len();
    // вибірка рівномірним кроком, а не випадково: прогін відтворюваний
    // Рівномірна вибірка майже не ловить ВАЖКІ гаманці, а дефект б'є саме по них:
    // насичений блок можливий лише там, де тисячі переказів. TOP=1 бере верхівку за
    // кількістю зібраних рядків — це прицільна, а не репрезентативна перевірка.
    let by_top = envs("TOP", "0") == "1";
    let picked: Vec<(String, u64)> = if by_top {
        let mut v: Vec<(String, u64, u64)> = done.iter()
            .map(|(w, sb)| (w.clone(), *sb, rows_of.get(w).copied().unwrap_or(0))).collect();
        v.sort_by(|a, b| b.2.cmp(&a.2));
        v.into_iter().take(sample).map(|(w, sb, _)| (w, sb)).collect()
    } else {
        let stride = (total / sample.max(1)).max(1);
        done.into_iter().step_by(stride).take(sample).collect()
    };
    eprintln!("[truncaudit] гаманців done {total}, у вибірці {}", picked.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40)).build().unwrap();
    let keyc = Arc::new(AtomicUsize::new(0));
    let ctr = Arc::new(AtomicUsize::new(0));
    let n_pick = picked.len();
    let chunks: Vec<Vec<(String, u64)>> = picked
        .chunks(picked.len().div_ceil(conc).max(1)).map(|c| c.to_vec()).collect();

    let mut handles = vec![];
    for chunk in chunks {
        let (client, keys, keyc, ctr, dd) =
            (client.clone(), keys.clone(), keyc.clone(), ctr.clone(), done_date.clone());
        handles.push(tokio::spawn(async move {
            let mut out: Vec<Value> = vec![];
            for (w, sb) in chunk {
                let get = |url: String| {
                    let c = client.clone();
                    async move {
                        for a in 0..5u64 {
                            if let Ok(r) = c.get(&url).send().await {
                                if let Ok(v) = r.json::<Value>().await {
                                    if !limited(&v) { return v["result"].as_array().cloned().unwrap_or_default(); }
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(500 * (1 << a.min(4)))).await;
                        }
                        vec![]
                    }
                };
                // 1) чи насичений останній блок
                let k1 = keys[keyc.fetch_add(1, Ordering::Relaxed) % keys.len()].clone();
                let at_block = get(format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=tokentx&address={w}&startblock={sb}&endblock={sb}&page=1&offset=1000&sort=asc&apikey={k1}")).await;
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
                // 2) що йде після нього
                let k2 = keys[keyc.fetch_add(1, Ordering::Relaxed) % keys.len()].clone();
                let after = get(format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action=tokentx&address={w}&startblock={}&endblock=99999999&page=1&offset=20&sort=asc&apikey={k2}", sb + 1)).await;

                let saturated = at_block.len() >= 1000;
                let first_after = after.first()
                    .and_then(|x| x["timeStamp"].as_str())
                    .and_then(|s| s.parse::<i64>().ok());
                // дата першого запису після чекпойнта у вигляді YYYY-MM-DD
                let first_after_day = first_after.map(|t| {
                    let days = t / 86400;
                    let z = days + 719468;
                    let era = if z >= 0 { z } else { z - 146096 } / 146097;
                    let doe = z - era * 146097;
                    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
                    let y = yoe + era * 400;
                    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
                    let mp = (5 * doy + 2) / 153;
                    let d = doy - (153 * mp + 2) / 5 + 1;
                    let m = if mp < 10 { mp + 3 } else { mp - 9 };
                    let y = if m <= 2 { y + 1 } else { y };
                    format!("{y:04}-{m:02}-{d:02}")
                });
                // втрачене — це записи, СТАРІШІ за день збору; свіжіші це просто нове життя
                let lost = first_after_day.as_deref().map(|d| d < dd.as_str()).unwrap_or(false);
                out.push(json!({
                    "wallet": w, "checkpointBlock": sb,
                    "rowsAtCheckpointBlock": at_block.len(),
                    "saturatedBlock": saturated,
                    "rowsAfter": after.len(),
                    "firstAfterDay": first_after_day,
                    "lostHistory": lost
                }));
                let i = ctr.fetch_add(1, Ordering::Relaxed) + 1;
                if i % 25 == 0 { eprintln!("[truncaudit] {i}/{n_pick}") }
                tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
            }
            out
        }));
    }
    let mut rows: Vec<Value> = vec![];
    for h in handles { if let Ok(v) = h.await { rows.extend(v) } }

    let n = rows.len().max(1);
    let sat = rows.iter().filter(|r| r["saturatedBlock"].as_bool().unwrap_or(false)).count();
    let lost = rows.iter().filter(|r| r["lostHistory"].as_bool().unwrap_or(false)).count();
    let both = rows.iter().filter(|r| r["saturatedBlock"].as_bool().unwrap_or(false)
        && r["lostHistory"].as_bool().unwrap_or(false)).count();

    let doc = json!({
        "generated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "walletsDone": total, "sampled": rows.len(),
        "saturatedBlock": sat, "lostHistory": lost, "both": both,
        "estimatedTruncatedPct": 100.0 * lost as f64 / n as f64,
        "rows": rows
    });
    // Список на ремонт: ЛИШЕ обидві ознаки разом. Сама по собі «історія після чекпойнта»
    // дає хибні спрацювання — гаманець міг просто ожити після збору; сам по собі насичений
    // блок теж не доводить втрати. Разом вони і є відбитком дефекту.
    let repair: Vec<&str> = rows.iter()
        .filter(|r| r["saturatedBlock"].as_bool().unwrap_or(false) && r["lostHistory"].as_bool().unwrap_or(false))
        .filter_map(|r| r["wallet"].as_str()).collect();
    let _ = std::fs::write(format!("{b}/truncaudit-repair.txt"), repair.join("
"));
    eprintln!("[truncaudit] на ремонт: {} гаманців", repair.len());

    File::create(format!("{b}/truncaudit.json")).unwrap()
        .write_all(serde_json::to_string_pretty(&doc).unwrap().as_bytes()).unwrap();

    println!("\n=== ВИБІРКОВА ПЕРЕВІРКА ОБРИВІВ ===");
    println!("гаманців зі статусом done: {total}, перевірено: {}", rows.len());
    println!("останній блок насичений (>=1000 записів): {sat}");
    println!("є ВТРАЧЕНА історія (записи, старіші за день збору): {lost}");
    println!("обидві ознаки разом: {both}");
    println!("оцінка частки обірваних: {:.1}%", 100.0 * lost as f64 / n as f64);
    for r in rows.iter().filter(|r| r["lostHistory"].as_bool().unwrap_or(false)).take(8) {
        println!("  {} блок {} · записів у блоці {} · після нього {} з {}",
            r["wallet"].as_str().unwrap_or(""), r["checkpointBlock"],
            r["rowsAtCheckpointBlock"], r["rowsAfter"],
            r["firstAfterDay"].as_str().unwrap_or("?"));
    }
    println!("\nJSON: {b}/truncaudit.json");
}
