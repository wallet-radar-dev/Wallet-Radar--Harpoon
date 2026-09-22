//! ethflow — історія НАТИВНОГО ETH (txlist + txlistinternal) з датами й сумами.
//!
//! Навіщо: докачка на 358 млн рядків — це лише `tokentx`, тобто самі токени. Нативного ETH
//! у ній немає жодного рядка, хоч саме він і є капіталом груп: HUB тримає $2.35 млрд, і
//! 99.9% з них — ETH. Наявний `eth_native` рахує тільки САЛЬДО з 2025-09-01, тож питання
//! «по чому купили і по чому продали» на ньому не відповісти: сальдо не має ні дат, ні цін.
//!
//! Схема виходу НАВМИСНЕ ідентична transfers*.csv (9 колонок), а токен і символ — "ETH".
//! Так `bullsell`/`groupbook` читають нативний ETH тим самим кодом, що й токени, без гілок.
//!
//! Резюме: PROG_FILE у форматі backfill2 (`wallet,chain,kind,status,rows,es:sb=<блок>`),
//! статус `done` пишеться ЛИШЕ після успішного завершення. Ліміт Etherscan приходить як
//! HTTP 200 з NOTOK у ТІЛІ — ловимо там, інакше «залімічено» не відрізнити від «порожньо»,
//! і гаманець назавжди лишиться позначеним як зібраний з нулем рядків.
//!
//! Args: ROOT. Env: WALLETS_FILE, OUT_FILE, PROG_FILE, CONCURRENCY(8), STARTBLOCK(0)

use serde_json::Value;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

fn envs(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| d.to_string())
}
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
/// RFC3339 з .000000Z — як у решті CSV, щоб парсери downstream не ламались.
fn ts_str(sec: i64) -> String {
    let days = sec.div_euclid(86400);
    let rem = sec.rem_euclid(86400);
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
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000000Z",
        y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}
/// Ліміт у ТІЛІ відповіді, а не в HTTP-коді.
fn limited(v: &Value) -> bool {
    let msg = v["message"].as_str().unwrap_or("");
    let res = v["result"].as_str().unwrap_or("");
    msg == "NOTOK" || res.contains("rate limit") || res.contains("Max calls")
        || res.contains("Max daily") || res.contains("Too many")
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".to_string());
    let bf = format!("{root}/dataset/backfill");
    let wfile = envs("WALLETS_FILE", &format!("{bf}/ethflow-wallets.txt"));
    let outf = envs("OUT_FILE", &format!("{bf}/transfers-native.csv"));
    let progf = envs("PROG_FILE", &format!("{bf}/_progress-native.csv"));
    let conc: usize = envs("CONCURRENCY", "8").parse().unwrap_or(8);
    let start_block: u64 = envs("STARTBLOCK", "0").parse().unwrap_or(0);

    let keys: Vec<String> = dotenv(&root, "BACKFILL_ETHERSCAN_KEYS")
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    assert!(!keys.is_empty(), "нема BACKFILL_ETHERSCAN_KEYS");

    // резюме: що вже зібрано (останній статус на пару гаманець+вид)
    let mut done: HashMap<(String, String), String> = HashMap::new();
    // Чекпойнт зберігає блок, на якому спинились. Без нього перерваний прохід (ребут,
    // зупинка) перекачував гаманець З НУЛЯ і дописував ті самі перекази вдруге — сальдо
    // з такого файлу завищене, і саме це зловила перевірка.
    let mut resume: HashMap<(String, String), u64> = HashMap::new();
    if let Ok(f) = File::open(&progf) {
        for l in BufReader::new(f).lines().map_while(Result::ok) {
            let p: Vec<&str> = l.split(',').collect();
            if p.len() >= 4 { done.insert((p[0].to_lowercase(), p[2].to_string()), p[3].to_string()); }
            if p.len() >= 6 {
                if let Some(sb) = p[5].strip_prefix("es:sb=").and_then(|s| s.trim().parse::<u64>().ok()) {
                    let k = (p[0].to_lowercase(), p[2].to_string());
                    let e = resume.entry(k).or_insert(0);
                    if sb > *e { *e = sb }
                }
            }
        }
    }
    let resume = Arc::new(resume);
    let wallets: Vec<String> = std::fs::read_to_string(&wfile).expect("WALLETS_FILE")
        .lines().map(|s| s.trim().to_lowercase()).filter(|s| s.starts_with("0x")).collect();

    let new_file = !std::path::Path::new(&outf).exists();
    let txw = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&outf).unwrap()));
    if new_file {
        let _ = txw.lock().await.write_all(b"wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash\n");
    }
    let pgw = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(&progf).unwrap()));

    let todo: Vec<String> = wallets.iter()
        .filter(|w| !(done.get(&((*w).clone(), "ext".into())).map(|s| s == "done").unwrap_or(false)
                   && done.get(&((*w).clone(), "int".into())).map(|s| s == "done").unwrap_or(false)))
        .cloned().collect();
    eprintln!("[ethflow] гаманців {} (лишилось {}), потоків {conc}", wallets.len(), todo.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build().unwrap();
    let ctr = Arc::new(AtomicUsize::new(0));
    let keyc = Arc::new(AtomicUsize::new(0));
    let total = todo.len();

    let mut handles = Vec::new();
    for chunk in todo.chunks(todo.len().div_ceil(conc).max(1)) {
        let chunk: Vec<String> = chunk.to_vec();
        let (client, keys, txw, pgw, ctr, keyc, resume) =
            (client.clone(), keys.clone(), txw.clone(), pgw.clone(), ctr.clone(), keyc.clone(), resume.clone());
        handles.push(tokio::spawn(async move {
            for w in chunk {
                let mut rows_all = 0u64;
                for kind in ["ext", "int"] {
                    let action = if kind == "ext" { "txlist" } else { "txlistinternal" };
                    let mut sb = resume.get(&(w.clone(), kind.to_string())).copied()
                        .unwrap_or(start_block).max(start_block);
                    let mut rows = 0u64;
                    let mut status = "done".to_string();
                    let mut page = 1u32;
                    loop {
                        let key = &keys[keyc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                        let url = format!("https://api.etherscan.io/v2/api?chainid=1&module=account&action={action}&address={w}&startblock={sb}&endblock=99999999&page={page}&offset=1000&sort=asc&apikey={key}");
                        let mut arr: Option<Vec<Value>> = None;
                        for attempt in 0..7u64 {
                            match client.get(&url).send().await {
                                Ok(r) => match r.json::<Value>().await {
                                    Ok(v) => {
                                        if limited(&v) {
                                            tokio::time::sleep(std::time::Duration::from_millis(600 * (1 << attempt.min(5)))).await;
                                            continue;
                                        }
                                        arr = Some(v["result"].as_array().cloned().unwrap_or_default());
                                        break;
                                    }
                                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(600 * (1 << attempt.min(5)))).await,
                                },
                                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(600 * (1 << attempt.min(5)))).await,
                            }
                        }
                        let Some(arr) = arr else { status = "err:req".into(); break };
                        let n = arr.len();
                        let mut last_block = sb;
                        let mut buf = String::new();
                        for it in &arr {
                            let bn: u64 = it["blockNumber"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                            if bn > last_block { last_block = bn }
                            // невдалі транзакції не рухають гроші
                            if it["isError"].as_str().unwrap_or("0") == "1" { continue }
                            let val: f64 = it["value"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                            if val <= 0.0 { continue }
                            let amount = val / 1e18;
                            let from = it["from"].as_str().unwrap_or("").to_lowercase();
                            let to = it["to"].as_str().unwrap_or("").to_lowercase();
                            let dir = if to == w { "i" } else if from == w { "o" } else { continue };
                            let cp = if dir == "o" { &to } else { &from };
                            let ts: i64 = it["timeStamp"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                            let hash = it["hash"].as_str().unwrap_or("");
                            buf.push_str(&format!("{w},eth,{dir},{},ETH,ETH,{cp},{amount},{hash}\n", ts_str(ts)));
                            rows += 1;
                        }
                        if !buf.is_empty() {
                            let _ = txw.lock().await.write_all(buf.as_bytes());
                        }
                        if n < 1000 { break }
                        // Сторінки В МЕЖАХ вікна: стрибати одразу на last_block не можна —
                        // якщо в одному блоці понад 1000 операцій, решта блока губиться
                        // назавжди, і саме так у біржових адрес зникали виходи (потік
                        // показував +1904 ETH там, де на ланцюгу лежало 2).
                        // Стеля Etherscan: page * offset <= 10000, тобто 10 сторінок.
                        if page < 10 {
                            page += 1;
                            continue;
                        }
                        // Вікно вичерпано. Зсуваємось на останній побачений блок (він
                        // перечитається, дублі знімаються дедупом на читанні); якщо блок
                        // не зрушив — у ньому понад 10000 операцій, іншого виходу немає.
                        page = 1;
                        sb = if last_block <= sb { sb + 1 } else { last_block };
                    }
                    rows_all += rows;
                    let _ = pgw.lock().await
                        .write_all(format!("{w},eth,{kind},{status},{rows},es:sb={sb}\n").as_bytes());
                }
                let i = ctr.fetch_add(1, Ordering::Relaxed) + 1;
                if i % 10 == 0 || rows_all > 5000 {
                    eprintln!("[{i}/{total}] {} -> {rows_all}", &w[..10.min(w.len())]);
                }
            }
        }));
    }
    for h in handles { let _ = h.await; }
    eprintln!("[ethflow] complete");
}
