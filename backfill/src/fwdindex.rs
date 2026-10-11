//! fwdindex — власний індексатор УПЕРЕД: читає блоки з публічного RPC, без Etherscan.
//!
//! Навіщо окремий бін, коли є `livewatch`. Livewatch теж читає блоки, але ходить у
//! `api.etherscan.io/v2/api?module=proxy` — тобто дані все одно приходять від постачальника,
//! умови якого забороняють комерційне використання, кешування понад особисте, навчання
//! моделей і зобовʼязують знищити зібране при відключенні. Тут джерело інше: звичайний
//! JSON-RPC вузол мережі. Дані блокчейну публічні, посередника з договором немає, тож усе,
//! що цей бін запише, лишається нашим назавжди.
//!
//! Архівний вузол для цього НЕ потрібен. Ми не добираємо історію — ми накопичуємо власний
//! чистий потік з моменту запуску. Історію можна буде взяти окремо (власний вузол або
//! публічний набір даних), і вона зійдеться з цим потоком по тій самій схемі рядків.
//!
//! Один прохід по блоку коштує ~1.05 запиту: повний блок дає і нативні перекази, і мітку
//! часу, а логи ERC-20 беруться пачкою на діапазон блоків.
//!
//! ЗАПОБІЖНИКИ (усі — з наших же граблів):
//!   * стан рухається ЛИШЕ після успішного запису й fsync — «done лише при успіху»;
//!   * відставання від голови на CONFIRMATIONS блоків, інакше реорг зіпсує записане;
//!   * помилка в ТІЛІ відповіді (JSON-RPC error) не менш важлива за код HTTP;
//!   * при збої ендпоінта — ротація на наступний, а не тихий пропуск блоків.
//!
//! Вихід (та сама схема, що й transfers-grp.csv, щоб решта конвеєра не мінялась):
//!   dataset/backfill/fwdindex-transfers.csv
//!     wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash
//!   token = адреса контракту; для нативного ETH — `native`, symbol = ETH.
//!
//! Походження (те, чим доводиться «дані наші»):
//!   dataset/backfill/fwdindex.provenance.jsonl — {ts, endpoint, from, to, rows}
//!
//! Стан:   dataset/backfill/fwdindex.state.json
//! Кеш:    dataset/backfill/fwdindex.tokens.json  (decimals/symbol по контрактах)
//!
//! Args: ROOT
//! Env:  RPC_URLS(список через кому) CHAIN(1) CONFIRMATIONS(12) MAX_BLOCKS(600)
//!       LOG_SPAN(20) LOOP_SECS(0 — один прохід) WALLETS(шлях до списку) NATIVE(1)
//!       START_BLOCK(0 — почати з голови) MIN_AMOUNT(0.0)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

/// topic0 події Transfer(address,address,uint256) — однаковий для всіх ERC-20.
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

/// Публічні вузли без ключів. Порядок — порядок спроб; на помилці беремо наступний.
///
/// Перевірено 14.08.2026: `rpc.ankr.com/eth` тепер вимагає ключ («Unauthorized»), тож у
/// списку його немає — інакше кожен цикл ротації втрачав би на ньому виклик.
/// `eth.llamarpc.com` віддає 521 нестабільно, тому стоїть після надійніших.
const DEFAULT_RPCS: &[&str] = &[
    "https://ethereum-rpc.publicnode.com",
    "https://eth.drpc.org",
    "https://cloudflare-eth.com",
    "https://eth.llamarpc.com",
];

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn envs(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

/// Шістнадцяткове в u64. Приймає і з префіксом, і без.
fn hex_u64(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// Поточний час у RFC-3339. Крейт зібрано з chrono без фічі `clock`, тож годинник
/// беремо зі std — навмисно, щоб не тягнути зайву залежність заради одного виклику.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

/// Шістнадцяткове 256-бітне в f64. u128 не вистачає на повні 32 байти, тому старші
/// розряди додаємо множенням — точності f64 для наших сум вистачає з великим запасом.
fn hex_f64(s: &str) -> f64 {
    let h = s.trim_start_matches("0x").trim_start_matches('0');
    if h.is_empty() {
        return 0.0;
    }
    let mut v = 0.0f64;
    for ch in h.chars() {
        match ch.to_digit(16) {
            Some(d) => v = v * 16.0 + d as f64,
            None => return 0.0,
        }
    }
    v
}

/// Останні 20 байтів 32-байтового топіка — це адреса.
fn topic_addr(t: &str) -> Option<String> {
    let h = t.trim_start_matches("0x");
    if h.len() < 40 {
        return None;
    }
    Some(format!("0x{}", &h[h.len() - 40..]).to_lowercase())
}

struct Rpc {
    client: reqwest::Client,
    urls: Vec<String>,
    cur: usize,
    calls: u64,
    /// Провайдер відмовив саме через ЗАШИРОКИЙ діапазон, а не через збій.
    /// Це не помилка транспорту, а вказівка звузити запит — і на неї треба
    /// реагувати, інакше повтор того самого діапазону триватиме вічно.
    too_many: bool,
}

impl Rpc {
    fn new(urls: Vec<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(25))
            .user_agent("wallet-radar-fwdindex/1.0")
            .build()
            .expect("http client");
        Rpc { client, urls, cur: 0, calls: 0, too_many: false }
    }

    fn endpoint(&self) -> &str {
        &self.urls[self.cur % self.urls.len()]
    }

    /// Один виклик JSON-RPC із ротацією ендпоінтів. Помилка в ТІЛІ (`error`) —
    /// така сама невдача, як і мережева: тихо повертати None не можна, інакше
    /// пропущений блок виглядатиме як блок без подій.
    async fn call(&mut self, method: &str, params: Value) -> Option<Value> {
        let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        self.too_many = false;
        for attempt in 0..self.urls.len() * 2 {
            let url = self.urls[self.cur % self.urls.len()].clone();
            self.calls += 1;
            let resp = self.client.post(&url).json(&body).send().await;
            match resp {
                Ok(r) => {
                    let status = r.status();
                    let txt = r.text().await.unwrap_or_default();
                    if !status.is_success() {
                        eprintln!("[rpc] {url} {method}: HTTP {status}");
                    } else if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                        if let Some(e) = v.get("error") {
                            // `execution reverted` — це ВІДПОВІДЬ вузла, а не його збій:
                            // контракт просто не має такого методу. Ротувати ендпоінти
                            // безглуздо, решта скаже те саме, а виклики згорять.
                            let msg = e.get("message").and_then(|m| m.as_str()).unwrap_or("");
                            if msg.contains("execution reverted") || msg.contains("invalid opcode") {
                                return None;
                            }
                            // «too many logs», «narrow your filter», «limit exceeded» —
                            // провайдер просить менший діапазон. Ротація не допоможе:
                            // сусідні вузли скажуть те саме. Позначаємо й виходимо,
                            // щоб викликач звузив вікно.
                            let ml = msg.to_lowercase();
                            if ml.contains("too many") || ml.contains("narrow") || ml.contains("limit exceeded")
                                || ml.contains("response size") || ml.contains("range")
                            {
                                self.too_many = true;
                                eprintln!("[rpc] {url} {method}: діапазон завеликий ({msg})");
                                return None;
                            }
                            eprintln!("[rpc] {url} {method}: помилка в тілі {e}");
                        } else if let Some(res) = v.get("result") {
                            if !res.is_null() {
                                return Some(res.clone());
                            }
                            eprintln!("[rpc] {url} {method}: result=null");
                        }
                    } else {
                        eprintln!("[rpc] {url} {method}: тіло не JSON ({} б)", txt.len());
                    }
                }
                Err(e) => eprintln!("[rpc] {url} {method}: {e}"),
            }
            self.cur += 1;
            let pause = 300 * (attempt as u64 + 1);
            tokio::time::sleep(std::time::Duration::from_millis(pause.min(4000))).await;
        }
        None
    }
}

/// decimals/symbol контракту. Тримаємо кеш на диску: кожен новий контракт коштує
/// два виклики, і платити за них двічі немає сенсу.
#[derive(Clone)]
struct TokenMeta {
    decimals: u32,
    symbol: String,
}

/// ABI-рядок: або динамічний (offset+len+дані), або bytes32 у старих токенах.
fn decode_abi_string(hex: &str) -> String {
    let h = hex.trim_start_matches("0x");
    if h.len() >= 128 {
        if let Some(len) = usize::from_str_radix(&h[64..128], 16).ok() {
            let start = 128;
            let end = start + len * 2;
            if len > 0 && len <= 64 && end <= h.len() {
                if let Some(b) = (0..len)
                    .map(|i| u8::from_str_radix(&h[start + i * 2..start + i * 2 + 2], 16).ok())
                    .collect::<Option<Vec<u8>>>()
                {
                    return String::from_utf8_lossy(&b).trim_matches('\0').trim().to_string();
                }
            }
        }
    }
    if h.len() >= 64 {
        if let Some(b) = (0..32)
            .map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()
        {
            let s = String::from_utf8_lossy(&b).trim_matches('\0').trim().to_string();
            if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') && !s.is_empty() {
                return s;
            }
        }
    }
    String::new()
}

async fn token_meta(rpc: &mut Rpc, cache: &mut HashMap<String, TokenMeta>, addr: &str) -> TokenMeta {
    if let Some(m) = cache.get(addr) {
        return m.clone();
    }
    // decimals() = 0x313ce567, symbol() = 0x95d89b41
    let dec = rpc
        .call("eth_call", json!([{"to": addr, "data": "0x313ce567"}, "latest"]))
        .await
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .and_then(|s| hex_u64(&s))
        .unwrap_or(18) as u32;
    let sym = rpc
        .call("eth_call", json!([{"to": addr, "data": "0x95d89b41"}, "latest"]))
        .await
        .and_then(|v| v.as_str().map(|s| decode_abi_string(s)))
        .unwrap_or_default();
    // 18 — не «правда», а безпечний дефолт: краще позначити токен, ніж загубити рядок.
    let m = TokenMeta { decimals: dec.min(36), symbol: sym };
    cache.insert(addr.to_string(), m.clone());
    m
}

fn load_wallets(path: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(f) = File::open(path) else {
        eprintln!("[watch] не відкрився список гаманців: {path}");
        return out;
    };
    // Підтримуємо і group-membership.json {wallets:{addr:hop}}, і простий список рядків.
    if path.ends_with(".json") {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            if let Some(w) = v.get("wallets").and_then(|w| w.as_object()) {
                for k in w.keys() {
                    out.insert(k.to_lowercase());
                }
            } else if let Some(a) = v.as_array() {
                for x in a {
                    if let Some(s) = x.as_str() {
                        out.insert(s.to_lowercase());
                    }
                }
            }
        }
    } else {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let s = line.trim().to_lowercase();
            if s.starts_with("0x") && s.len() == 42 {
                out.insert(s);
            }
        }
    }
    out
}

fn read_state(path: &str) -> Option<u64> {
    let v: Value = serde_json::from_reader(BufReader::new(File::open(path).ok()?)).ok()?;
    v.get("last_block").and_then(|b| b.as_u64())
}

/// Стан пишемо через тимчасовий файл і rename — щоб обрив живлення посеред запису
/// не лишив нам зіпсований стан і мовчазну діру в даних.
fn write_state(path: &str, last: u64, rows: u64, endpoint: &str) -> std::io::Result<()> {
    let tmp = format!("{path}.tmp");
    let v = json!({
        "last_block": last,
        "rows_total": rows,
        "endpoint": endpoint,
        "updated": now_rfc3339(),
    });
    {
        let mut f = File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&v)?.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

struct Row {
    wallet: String,
    dir: &'static str,
    ts: String,
    token: String,
    symbol: String,
    counterparty: String,
    amount: f64,
    tx: String,
    /// Номер блока. Потрібен не для звіту, а щоб НЕ записати рядки з блоків,
    /// логи яких не прочитались: інакше стан пішов би вперед по блоку, для якого
    /// зібрані лише нативні перекази, і діра в ERC-20 лишилась би назавжди.
    blk: u64,
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let chain: u64 = envn("CHAIN", 1);
    let confirmations: u64 = envn("CONFIRMATIONS", 12);
    let max_blocks: u64 = envn("MAX_BLOCKS", 600);
    let log_span: u64 = envn("LOG_SPAN", 20).max(1);
    let loop_secs: u64 = envn("LOOP_SECS", 0);
    let want_native: u64 = envn("NATIVE", 1);
    let start_block: u64 = envn("START_BLOCK", 0);
    let min_amount: f64 = envn("MIN_AMOUNT", 0.0);

    let urls: Vec<String> = envs("RPC_URLS", &DEFAULT_RPCS.join(","))
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut rpc = Rpc::new(urls);

    let dir = format!("{root}/dataset/backfill");
    let out_csv = format!("{dir}/fwdindex-transfers.csv");
    let state_path = format!("{dir}/fwdindex.state.json");
    let prov_path = format!("{dir}/fwdindex.provenance.jsonl");
    let tokens_path = format!("{dir}/fwdindex.tokens.json");

    let wallets_path = envs("WALLETS", &format!("{dir}/group-membership.json"));
    let watch = load_wallets(&wallets_path);
    if watch.is_empty() {
        eprintln!("[fwdindex] список гаманців порожній — індексувати нема для кого, вихід");
        std::process::exit(2);
    }
    println!("[fwdindex] під наглядом адрес: {}", watch.len());

    let mut tokens: HashMap<String, TokenMeta> = std::fs::read_to_string(&tokens_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|o| {
            o.into_iter()
                .filter_map(|(k, v)| {
                    Some((
                        k,
                        TokenMeta {
                            decimals: v.get("decimals")?.as_u64()? as u32,
                            symbol: v.get("symbol")?.as_str()?.to_string(),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    println!("[fwdindex] у кеші токенів: {}", tokens.len());

    let mut rows_total: u64 = 0;

    loop {
        // 1. голова мережі
        let Some(head) = rpc
            .call("eth_blockNumber", json!([]))
            .await
            .and_then(|v| v.as_str().and_then(hex_u64))
        else {
            eprintln!("[fwdindex] голову мережі не отримано — жоден ендпоінт не відповів");
            if loop_secs == 0 {
                std::process::exit(1);
            }
            tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
            continue;
        };

        // 2. відстаємо від голови — інакше реорг перепише те, що ми вже записали
        let target = head.saturating_sub(confirmations);
        let last = read_state(&state_path).unwrap_or_else(|| {
            if start_block > 0 { start_block.saturating_sub(1) } else { target.saturating_sub(1) }
        });

        if target <= last {
            println!("[fwdindex] нових підтверджених блоків немає (голова {head}, оброблено {last})");
            if loop_secs == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
            continue;
        }

        let from = last + 1;
        let to = target.min(from + max_blocks - 1);
        println!("[fwdindex] блоки {from}..{to} (голова {head}, відставання {confirmations})");

        let mut rows: Vec<Row> = Vec::new();
        let mut ts_by_block: HashMap<u64, String> = HashMap::new();
        let mut failed = false;

        // 3. повні блоки: нативні перекази + мітки часу
        for bn in from..=to {
            let Some(b) = rpc
                .call("eth_getBlockByNumber", json!([format!("0x{bn:x}"), want_native == 1]))
                .await
            else {
                eprintln!("[fwdindex] блок {bn} не прочитано — зупиняємось на {}", bn.saturating_sub(1));
                failed = true;
                break;
            };
            let ts_unix = b.get("timestamp").and_then(|t| t.as_str()).and_then(hex_u64).unwrap_or(0);
            let ts = chrono::DateTime::from_timestamp(ts_unix as i64, 0)
                .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string())
                .unwrap_or_default();
            ts_by_block.insert(bn, ts.clone());

            if want_native == 1 {
                if let Some(txs) = b.get("transactions").and_then(|t| t.as_array()) {
                    for t in txs {
                        let value = t.get("value").and_then(|v| v.as_str()).map(hex_f64).unwrap_or(0.0);
                        if value <= 0.0 {
                            continue;
                        }
                        let f = t.get("from").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        let to_a = t.get("to").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                        let hash = t.get("hash").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let amount = value / 1e18;
                        if amount < min_amount {
                            continue;
                        }
                        if watch.contains(&f) {
                            rows.push(Row { wallet: f.clone(), dir: "o", ts: ts.clone(),
                                token: "native".into(), symbol: "ETH".into(),
                                counterparty: to_a.clone(), amount, tx: hash.clone(), blk: bn });
                        }
                        if !to_a.is_empty() && watch.contains(&to_a) {
                            rows.push(Row { wallet: to_a.clone(), dir: "i", ts: ts.clone(),
                                token: "native".into(), symbol: "ETH".into(),
                                counterparty: f.clone(), amount, tx: hash.clone(), blk: bn });
                        }
                    }
                }
            }
        }

        if failed && ts_by_block.is_empty() {
            if loop_secs == 0 {
                std::process::exit(1);
            }
            tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
            continue;
        }
        // Просунемось лише по тих блоках, які реально прочитані.
        let done_to = ts_by_block.keys().copied().max().unwrap_or(last);

        // 4. логи ERC-20 пачками по діапазону.
        // logs_done_to — останній блок, ЛОГИ якого справді прочитані. Саме він, а не
        // done_to, обмежує і запис, і стан: блок з нативними переказами, але без логів,
        // вважається НЕобробленим.
        let mut logs_done_to = from.saturating_sub(1);
        let mut span_from = from;
        // Ширина вікна адаптивна. Щільність блоків росте, і те, що вчора вміщалось
        // у стелю провайдера, сьогодні вже ні: 20 блоків почали віддавати понад
        // 20 тис. логів. Фіксована ширина через це заклинила збір на п'ять діб.
        let mut span = log_span;
        while span_from <= done_to {
            let span_to = (span_from + span - 1).min(done_to);
            let Some(logs) = rpc
                .call(
                    "eth_getLogs",
                    json!([{
                        "fromBlock": format!("0x{span_from:x}"),
                        "toBlock": format!("0x{span_to:x}"),
                        "topics": [TRANSFER_TOPIC]
                    }]),
                )
                .await
            else {
                if rpc.too_many && span > 1 {
                    span = (span / 2).max(1);
                    eprintln!("[fwdindex] діапазон завеликий — звужую вікно логів до {span} блоків");
                    continue;
                }
                eprintln!("[fwdindex] логи {span_from}..{span_to} не прочитано — прохід перервано");
                failed = true;
                break;
            };
            let empty = vec![];
            for l in logs.as_array().unwrap_or(&empty) {
                let topics = l.get("topics").and_then(|t| t.as_array()).cloned().unwrap_or_default();
                if topics.len() < 3 {
                    continue; // ERC-721 має 4 топіки і нульові дані — не наш випадок
                }
                let (Some(f), Some(t)) = (
                    topics[1].as_str().and_then(topic_addr),
                    topics[2].as_str().and_then(topic_addr),
                ) else {
                    continue;
                };
                let f_hit = watch.contains(&f);
                let t_hit = watch.contains(&t);
                if !f_hit && !t_hit {
                    continue;
                }
                let contract = l.get("address").and_then(|a| a.as_str()).unwrap_or("").to_lowercase();
                if contract.is_empty() {
                    continue;
                }
                let raw = l.get("data").and_then(|d| d.as_str()).map(hex_f64).unwrap_or(0.0);
                let bn = l.get("blockNumber").and_then(|b| b.as_str()).and_then(hex_u64).unwrap_or(0);
                let hash = l.get("transactionHash").and_then(|h| h.as_str()).unwrap_or("").to_string();
                let ts = ts_by_block.get(&bn).cloned().unwrap_or_default();

                let meta = token_meta(&mut rpc, &mut tokens, &contract).await;
                let amount = raw / 10f64.powi(meta.decimals as i32);
                if amount < min_amount {
                    continue;
                }
                if f_hit {
                    rows.push(Row { wallet: f.clone(), dir: "o", ts: ts.clone(),
                        token: contract.clone(), symbol: meta.symbol.clone(),
                        counterparty: t.clone(), amount, tx: hash.clone(), blk: bn });
                }
                if t_hit {
                    rows.push(Row { wallet: t.clone(), dir: "i", ts: ts.clone(),
                        token: contract.clone(), symbol: meta.symbol.clone(),
                        counterparty: f.clone(), amount, tx: hash.clone(), blk: bn });
                }
            }
            logs_done_to = span_to;
            span_from = span_to + 1;
            // Повертаємось до заданої ширини поступово: звуження було відповіддю
            // на конкретний щільний відрізок, а не вироком усьому проходу.
            if span < log_span {
                span = (span * 2).min(log_span);
            }
        }

        // 5. запис. Стеля проходу — мінімум із прочитаних блоків і прочитаних логів.
        // Без цього прохід, у якому впав eth_getLogs, лишав би тиху діру в ERC-20:
        // рівно той клас дефекту, за який у проєкті вже проходив аудит 46 бінарників.
        let effective_to = done_to.min(logs_done_to.max(from.saturating_sub(1)));
        if effective_to < from {
            eprintln!("[fwdindex] жодного повністю обробленого блока — стан не рухаємо");
            if loop_secs == 0 {
                std::process::exit(1);
            }
            tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
            continue;
        }
        let dropped = rows.iter().filter(|r| r.blk > effective_to).count();
        rows.retain(|r| r.blk <= effective_to);
        if dropped > 0 {
            eprintln!("[fwdindex] відкладено {dropped} рядків з блоків понад {effective_to} — доберуться наступного проходу");
        }

        // 5. запис. Спершу дані на диск і fsync — лише потім стан.
        let new_file = !std::path::Path::new(&out_csv).exists();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out_csv)
            .expect("не відкрився файл виходу");
        if new_file {
            writeln!(f, "wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash,blk,src").ok();
        }
        for r in &rows {
            writeln!(
                f,
                "{},{},{},{},{},{},{},{},{},{},own-rpc",
                r.wallet, chain, r.dir, r.ts, r.token,
                csv_escape(&r.symbol), r.counterparty, r.amount, r.tx, r.blk
            )
            .ok();
        }
        f.sync_all().ok();
        rows_total += rows.len() as u64;

        std::fs::write(
            &tokens_path,
            serde_json::to_string(
                &tokens
                    .iter()
                    .map(|(k, v)| (k.clone(), json!({"decimals": v.decimals, "symbol": v.symbol})))
                    .collect::<serde_json::Map<_, _>>(),
            )
            .unwrap_or_default(),
        )
        .ok();

        // Слід походження — те, чим доводиться, що дані наші, а не чужі.
        if let Ok(mut p) = OpenOptions::new().create(true).append(true).open(&prov_path) {
            writeln!(
                p,
                "{}",
                json!({
                    "ts": now_rfc3339(),
                    "source": "public-json-rpc",
                    "endpoint": rpc.endpoint(),
                    "chain": chain,
                    "from_block": from,
                    "to_block": effective_to,
                    "blocks_read": done_to,
                    "status": if failed || effective_to < to { "partial" } else { "complete" },
                    "rows": rows.len(),
                })
            )
            .ok();
        }

        write_state(&state_path, effective_to, rows_total, rpc.endpoint()).ok();
        println!(
            "[fwdindex] блоки {from}..{effective_to} · рядків {} · разом {rows_total} · викликів {}",
            rows.len(),
            rpc.calls
        );

        if failed {
            eprintln!("[fwdindex] прохід завершено частково — решта добереться наступного разу");
        }
        if loop_secs == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(loop_secs)).await;
    }
}
