//! chain — ЄДИНИЙ шар доступу до даних ланцюга.
//!
//! НАВІЩО. 32 біни ходять в експлорери, і кожен ніс власну копію одного й того самого
//! знання: ротація ключів, ловля NOTOK у ТІЛІ відповіді, експоненційний відступ, межа
//! 1000 записів на запит, просування стартового блоку. Копії розійшлися — аудит
//! 2026-07-31 знайшов дефект NOTOK у 13 бінах, а `offset=10000` мовчки різав історію
//! ще в 6. Це не випадковість: 32 копії протоколу означають 32 місця, де можна забути
//! урок.
//!
//! Тут протокол один. Бін не знає, звідки прийшли дані — з Etherscan, з Blockscout чи
//! зі СВОЄЇ НОДИ. Саме заради останнього все й робиться: коли з'явиться NVMe і Erigon,
//! перехід буде зміною `CHAIN_SOURCE=node` в оточенні, а не правкою 32 файлів.
//!
//! ─────────────────────── ГОЛОВНЕ ПРАВИЛО ЦЬОГО МОДУЛЯ ───────────────────────
//! `None` ≠ порожньо. `Some(vec![])` означає «джерело відповіло: подій немає».
//! `None` означає «після всіх спроб відповіді нема» — гаманець НЕ можна писати в `.done`.
//! Ціна плутанини вже виміряна: гаманці, позначені зробленими з нулем рядків, і звіт,
//! що рапортує «активності немає» там, де її просто не перевірили.
//! ────────────────────────────────────────────────────────────────────────────
//!
//! ЧОМУ enum, а не `dyn Trait`: async-функції в трейтах не дають object safety без
//! зовнішнього крейта (`async-trait`). Додавати залежність заради трьох реалізацій —
//! зайве; enum-диспетч коштує один match і не тягне нічого в Cargo.toml.
//!
//! Оточення (читається `ChainSource::from_env`):
//!   CHAIN_SOURCE  etherscan | blockscout | node   (за замовчуванням — за наявними даними)
//!   API_BASE      база Blockscout, напр. https://arbitrum.blockscout.com/api
//!   RPC_URLS      список JSON-RPC через кому (для node)
//!   RPS           запитів/с на ВСЕ джерело (типово 3×ключі, стеля 15; нода — 50)
//!   TRIES         спроб на запит (7)
//!   MAXWIN        стеля вікон пагінації на гаманець (200)
//! Ключі: `BACKFILL_ETHERSCAN_KEYS`/`ETHERSCAN_API_KEYS` — з оточення, потім
//! `deploy/backfill.env`, потім `.env` (див. `find_keys`).

use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ─────────────────────────────── типи ───────────────────────────────

/// Вид історії. Назви (`ext`/`int`/`tok`) збігаються з тим, що вже лежить у
/// `txfull-<chain>.jsonl` — щоб міграція бінів не міняла формат даних на диску.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Ext, Int, Tok }

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self { Kind::Ext => "ext", Kind::Int => "int", Kind::Tok => "tok" }
    }
    /// Дія Etherscan/Blockscout (обидва тримають один і той самий API аккаунта).
    fn action(self) -> &'static str {
        match self { Kind::Ext => "txlist", Kind::Int => "txlistinternal", Kind::Tok => "tokentx" }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim() {
            "ext" | "txlist" => Some(Kind::Ext),
            "int" | "txlistinternal" => Some(Kind::Int),
            "tok" | "tokentx" => Some(Kind::Tok),
            _ => None,
        }
    }
    /// Розбір списку виду `"ext,int,tok"` — типовий вміст env KINDS.
    pub fn parse_list(s: &str) -> Vec<Kind> {
        s.split(',').filter_map(Kind::parse).collect()
    }
}

/// Одна транзакція у нормалізованому вигляді.
///
/// Числові поля лишаються РЯДКАМИ у сирих одиницях (wei, найменша частка токена).
/// Це навмисно: `u64` не тримає wei, а `f64` тихо втрачає точність на великих сумах —
/// саме той клас похибки, через який потокові оцінки балансів дали 23% правди.
#[derive(Clone, Debug)]
pub struct Tx {
    pub w: String,          // гаманець, за яким робився запит
    pub kind: Kind,
    pub ts: u64,
    pub block: u64,
    pub hash: String,
    pub from: String,
    pub to: String,
    pub value: String,      // сирі одиниці
    pub token: String,      // контракт (лише для Tok)
    pub sym: String,
    pub dec: String,
    pub gas: String,
    pub gas_price: String,
    pub nonce: String,
    pub err: String,        // "1" = транзакція впала
}

impl Tx {
    /// Формат рядка ІДЕНТИЧНИЙ тому, що пише txfull. Міграція біна не переписує дані.
    pub fn to_json(&self) -> Value {
        json!({
            "w": self.w, "k": self.kind.as_str(), "ts": self.ts, "h": self.hash,
            "f": self.from, "t": self.to, "v": self.value,
            "tok": self.token, "sym": self.sym, "dec": self.dec,
            "gas": self.gas, "gp": self.gas_price, "nonce": self.nonce, "err": self.err
        })
    }

    /// Ключ дедуплікації. Потрібен, бо пагінація «з останнього блоку» перечитує
    /// граничний блок, а один hash може дати кілька рядків (кілька ERC-20 переказів
    /// в одній транзакції) — тому в ключі не лише hash.
    pub fn dedup_key(&self) -> String {
        format!("{}|{}|{}|{}|{}|{}", self.hash, self.kind.as_str(), self.from, self.to, self.value, self.token)
    }

    fn from_es_row(w: &str, kind: Kind, r: &Value) -> Tx {
        let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
        Tx {
            w: w.to_lowercase(),
            kind,
            ts: r["timeStamp"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0),
            block: r["blockNumber"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0),
            hash: s("hash"),
            from: s("from").to_lowercase(),
            to: s("to").to_lowercase(),
            value: if r["value"].is_string() { s("value") } else { "0".to_string() },
            token: s("contractAddress").to_lowercase(),
            sym: s("tokenSymbol"),
            dec: s("tokenDecimal"),
            gas: s("gas"),
            gas_price: s("gasPrice"),
            nonce: s("nonce"),
            err: if r["isError"].as_str().unwrap_or("0") == "1" { "1".to_string() } else { "0".to_string() },
        }
    }
}

/// Результат повного проходу історії гаманця.
///
/// `complete: false` означає, що частина вікон не відповіла. Рядки, що встигли
/// зібратись, лишаються (append-файл + дедуп по hash далі в пайплайні), але гаманець
/// НЕ можна позначати зробленим — інакше повторюємо помилку eth_edges2.
pub struct Pull {
    pub txs: Vec<Tx>,
    pub complete: bool,
}

// ─────────────────────────── обмежувач квоти ───────────────────────────

/// Token-bucket на все джерело.
///
/// ЧОМУ НА ДЖЕРЕЛО, А НЕ НА КЛЮЧ: у Etherscan ліміт 3 req/s і 100k/добу, і **ретрай
/// теж списується**. Коли кожен бін сам «розганявся» до свого RPS, паралельні біни
/// складались і разом виходили за стелю — так згоріло 8 із 9 ключів. Один обмежувач
/// на процес робить перевитрату неможливою за побудовою.
pub struct Quota {
    rps: f64,
    cell: Mutex<(Instant, f64)>,
}

impl Quota {
    pub fn new(rps: f64) -> Quota {
        let rps = if rps.is_finite() && rps > 0.05 { rps } else { 3.0 };
        Quota { rps, cell: Mutex::new((Instant::now(), rps)) }
    }
    /// Чекає, доки звільниться слот. Викликається ПЕРЕД кожною спробою, ретрай включно.
    pub async fn take(&self) {
        loop {
            let wait = {
                let mut g = self.cell.lock().unwrap();   // guard живе лише в цьому блоці
                let now = Instant::now();
                let dt = now.duration_since(g.0).as_secs_f64();
                g.0 = now;
                let cap = if self.rps > 1.0 { self.rps } else { 1.0 };
                g.1 = (g.1 + dt * self.rps).min(cap);
                if g.1 >= 1.0 {
                    g.1 -= 1.0;
                    None
                } else {
                    let need = (1.0 - g.1) / self.rps;
                    Some(if need > 0.005 { need } else { 0.005 })
                }
            };
            match wait {
                None => return,
                Some(s) => tokio::time::sleep(Duration::from_secs_f64(s)).await,
            }
        }
    }
}

// ─────────────────────────────── джерело ───────────────────────────────

enum Backend {
    Etherscan { keys: Vec<String>, kc: AtomicUsize },
    Blockscout { base: String },
    Node { rpc: Vec<String>, rc: AtomicUsize },
}

pub struct ChainSource {
    pub chain: u32,
    backend: Backend,
    client: reqwest::Client,
    quota: Quota,
    tries: u32,
    page: usize,
    maxwin: u32,
}

fn env_num<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

/// Де насправді лежать ключі Etherscan.
///
/// ЧОМУ СПИСОК, А НЕ ОДИН ФАЙЛ: 19.08.2026 ключі перенесли з `.env` у
/// `deploy/backfill.env` (права 600). Скрипти, що передають їх через оточення,
/// продовжили працювати, а `cexsignal` — який читав лише `.env` — замовк і сім
/// днів писав у cron-лог «нема BACKFILL_ETHERSCAN_KEYS» тричі на годину. Монітор,
/// що мав ловити злив мережі на біржі, весь цей час не дивився нікуди.
///
/// Порядок: оточення (скрипт передав явно) → deploy/backfill.env → .env.
/// Додавати нові місця СЮДИ, а не в черговий бін.
fn find_keys(root: &str) -> Vec<String> {
    for name in ["BACKFILL_ETHERSCAN_KEYS", "ETHERSCAN_API_KEYS"] {
        let v = split_list(&std::env::var(name).unwrap_or_default());
        if !v.is_empty() { return v; }
    }
    for file in [format!("{root}/deploy/backfill.env"), format!("{root}/.env")] {
        let body = std::fs::read_to_string(&file).unwrap_or_default();
        for name in ["BACKFILL_ETHERSCAN_KEYS", "ETHERSCAN_API_KEYS"] {
            let line = body.lines().find(|l| l.starts_with(&format!("{name}=")));
            let v = split_list(line.and_then(|l| l.splitn(2, '=').nth(1)).unwrap_or("").trim());
            if !v.is_empty() {
                eprintln!("[chain] ключів {} з {file}", v.len());
                return v;
            }
        }
    }
    Vec::new()
}

/// Ліміт приходить як HTTP **200** з тілом `{"status":"0","message":"NOTOK",
/// "result":"Max rate limit reached"}`. `result` тут РЯДОК, не масив, тому
/// `as_array()` мовчки не спрацьовує, а перевірка HTTP-статусу відказ не бачить.
/// Ловити треба саме тут.
fn is_limited(v: &Value) -> bool {
    let msg = v["message"].as_str().unwrap_or("");
    let res = v["result"].as_str().unwrap_or("");
    msg == "NOTOK"
        || res.contains("rate limit")
        || res.contains("Max calls")
        || res.contains("Too many")
        || res.contains("Max daily")
        || res.contains("Invalid API Key")
}

async fn backoff(attempt: u32) {
    let shift = if attempt > 6 { 6 } else { attempt };
    tokio::time::sleep(Duration::from_millis(350 * (1u64 << shift))).await;
}

fn hex_u128(s: &str) -> Option<u128> {
    u128::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}
fn hex_u64(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}
/// 20-байтова адреса, доповнена до слова ABI (без префікса `0x`).
fn pad_addr(a: &str) -> String {
    format!("{:0>64}", a.trim_start_matches("0x").to_lowercase())
}

/// topic0 події `Transfer(address,address,uint256)`.
pub const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

impl ChainSource {
    /// Збирає джерело з оточення. Пріоритет: явний `CHAIN_SOURCE` → `API_BASE`
    /// (Blockscout) → ключі Etherscan → `RPC_URLS`. Паніка тут навмисна: бін без
    /// джерела даних має впасти одразу, а не зібрати нуль рядків і назвати це результатом.
    pub fn from_env(root: &str, chain: u32) -> ChainSource {
        let api = std::env::var("API_BASE").unwrap_or_default();
        let rpc = split_list(&std::env::var("RPC_URLS").unwrap_or_default());
        let keys = find_keys(root);
        let want = std::env::var("CHAIN_SOURCE").unwrap_or_default().to_lowercase();
        let backend = match want.as_str() {
            "node" => {
                assert!(!rpc.is_empty(), "CHAIN_SOURCE=node потребує RPC_URLS");
                Backend::Node { rpc, rc: AtomicUsize::new(0) }
            }
            "blockscout" => {
                assert!(!api.is_empty(), "CHAIN_SOURCE=blockscout потребує API_BASE");
                Backend::Blockscout { base: api }
            }
            "etherscan" => {
                assert!(!keys.is_empty(), "CHAIN_SOURCE=etherscan потребує ключів у .env");
                Backend::Etherscan { keys, kc: AtomicUsize::new(0) }
            }
            _ if !api.is_empty() => Backend::Blockscout { base: api },
            _ if !keys.is_empty() => Backend::Etherscan { keys, kc: AtomicUsize::new(0) },
            _ if !rpc.is_empty() => Backend::Node { rpc, rc: AtomicUsize::new(0) },
            _ => panic!("немає джерела: задай API_BASE, RPC_URLS або ключі Etherscan у {root}/.env"),
        };
        let conc: usize = env_num("CONC", 8usize);
        let page: usize = env_num("PAGE", 1000usize);
        // Ліміт 3 req/s рахується НА КЛЮЧ, тож 8 ключів дають 24. Але стеля не єдина
        // межа: є ще 100k/добу на ключ, і ретрай списується теж. Тому беремо 3×ключі
        // з запасом і жорсткою стелею 15 — швидкість, за якої добовий бюджет не з'їдається
        // одним монітором. Явний RPS перекриває розрахунок.
        let nkeys = match &backend {
            Backend::Etherscan { keys, .. } => keys.len().max(1),
            _ => 1,
        };
        let auto_rps = match &backend {
            Backend::Node { .. } => 50.0,                       // своя нода — чужої квоти нема
            Backend::Blockscout { .. } => 5.0,                  // без ключа, м'який ліміт
            Backend::Etherscan { .. } => (3.0 * nkeys as f64 * 0.8).min(15.0),
        };
        let cs = ChainSource {
            chain,
            backend,
            client: reqwest::Client::builder()
                .user_agent("wallet-radar-chain/0.1")
                .pool_max_idle_per_host(conc)
                .timeout(Duration::from_secs(45))
                .build().expect("reqwest client"),
            quota: Quota::new(env_num("RPS", auto_rps)),
            tries: env_num("TRIES", 7u32),
            // 1000 — ЖОРСТКА стеля Etherscan на запит. Було `offset=10000`: сервер
            // мовчки віддавав 1000, і решта історії зникала. Не піднімати.
            page: if page > 1000 { 1000 } else { page },
            maxwin: env_num("MAXWIN", 200u32),
        };
        eprintln!("[chain] мережа {} · джерело {} · rps {} · спроб {} · сторінка {}",
            cs.chain, cs.source_name(), cs.quota.rps, cs.tries, cs.page);
        cs
    }

    pub fn source_name(&self) -> &'static str {
        match &self.backend {
            Backend::Etherscan { .. } => "etherscan",
            Backend::Blockscout { .. } => "blockscout",
            Backend::Node { .. } => "node",
        }
    }

    /// Чи тягне джерело квоту стороннього сервісу. Своя нода — ні, і біни можуть
    /// на цій підставі знімати штучні паузи між вікнами.
    pub fn is_metered(&self) -> bool {
        !matches!(self.backend, Backend::Node { .. })
    }

    // ─────────────────────────── низький рівень ───────────────────────────

    /// Запит до API аккаунта (Etherscan V2 або Blockscout). Повертає ВСЮ відповідь;
    /// розбір `result` — на викликачеві, бо семантика різна для різних дій.
    async fn es_call(&self, params: &str) -> Option<Value> {
        for attempt in 0..self.tries {
            self.quota.take().await;
            let url = match &self.backend {
                Backend::Etherscan { keys, kc } => {
                    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                    format!("https://api.etherscan.io/v2/api?chainid={}&{params}&apikey={key}", self.chain)
                }
                Backend::Blockscout { base } => format!("{base}?{params}"),
                Backend::Node { .. } => return None,
            };
            if let Ok(r) = self.client.get(url).send().await {
                let st = r.status().as_u16();
                if st != 429 && st < 500 {
                    if let Ok(v) = r.json::<Value>().await {
                        if !is_limited(&v) { return Some(v); }
                    }
                }
            }
            backoff(attempt).await;
        }
        None
    }

    /// Виклик JSON-RPC. `None` — і на транспортній помилці, і на `error` у тілі:
    /// для викликача це однаково «відповіді нема».
    async fn rpc_call(&self, method: &str, params: Value) -> Option<Value> {
        let (rpc, rc) = match &self.backend {
            Backend::Node { rpc, rc } => (rpc, rc),
            _ => return None,
        };
        for attempt in 0..self.tries {
            self.quota.take().await;
            let url = &rpc[rc.fetch_add(1, Ordering::Relaxed) % rpc.len()];
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params.clone()});
            if let Ok(r) = self.client.post(url).json(&body).send().await {
                if r.status().is_success() {
                    if let Ok(v) = r.json::<Value>().await {
                        if v.get("error").is_none() {
                            if let Some(res) = v.get("result") {
                                if !res.is_null() { return Some(res.clone()); }
                            }
                        }
                    }
                }
            }
            backoff(attempt).await;
        }
        None
    }

    /// `eth_call` для read-only викликів контракту. `data` — сирий ABI-запит.
    /// На експлорері йде через `module=proxy`, на ноді — прямо.
    async fn eth_call(&self, to: &str, data: &str) -> Option<String> {
        match &self.backend {
            Backend::Node { .. } => {
                let v = self.rpc_call("eth_call", json!([{"to": to, "data": data}, "latest"])).await?;
                v.as_str().map(|s| s.to_string())
            }
            _ => {
                let v = self.es_call(&format!(
                    "module=proxy&action=eth_call&to={to}&data={data}&tag=latest")).await?;
                v["result"].as_str().map(|s| s.to_string())
            }
        }
    }

    // ─────────────────────── історія транзакцій ───────────────────────

    /// Одна партія (до `page` записів) від блоку `from_block` включно, за зростанням.
    pub async fn txs_page(&self, kind: Kind, addr: &str, from_block: u64) -> Option<Vec<Tx>> {
        if matches!(self.backend, Backend::Node { .. }) {
            return self.node_txs_page(kind, addr, from_block).await;
        }
        let params = format!(
            "module=account&action={}&address={addr}&startblock={from_block}&endblock=99999999&page=1&offset={}&sort=asc",
            kind.action(), self.page);
        let v = self.es_call(&params).await?;
        if let Some(a) = v["result"].as_array() {
            return Some(a.iter().map(|r| Tx::from_es_row(addr, kind, r)).collect());
        }
        // «No transactions found» — це ВІДПОВІДЬ, а не збій: історія справді порожня.
        if v["message"].as_str().unwrap_or("").contains("No transactions found") {
            return Some(Vec::new());
        }
        None
    }

    /// ОСТАННІ `n` подій адреси (найновіші першими).
    ///
    /// Окремий метод, а не `txs_page` з іншим сортуванням: моніторам (`wakeup`,
    /// `livewatch`) і подієвим дослідженням (`rhythm`, `gasprint`, `eventstudy`,
    /// `sellstudy`) потрібен саме хвіст історії, і питати його повним проходом з
    /// нульового блоку — це спалена квота на кожному запуску.
    pub async fn latest_txs(&self, kind: Kind, addr: &str, n: usize) -> Option<Vec<Tx>> {
        let n = if n > 1000 { 1000 } else { n };
        if matches!(self.backend, Backend::Node { .. }) {
            // 0 = «від голови ланцюга назад»
            let v = self.rpc_call("ots_searchTransactionsBefore", json!([addr, 0, n])).await?;
            let txs = v["txs"].as_array()?;
            let mut out = Vec::with_capacity(txs.len());
            for t in txs {
                out.push(Tx {
                    w: addr.to_lowercase(), kind,
                    ts: 0,
                    block: t["blockNumber"].as_str().and_then(hex_u64).unwrap_or(0),
                    hash: t["hash"].as_str().unwrap_or("").to_string(),
                    from: t["from"].as_str().unwrap_or("").to_lowercase(),
                    to: t["to"].as_str().unwrap_or("").to_lowercase(),
                    value: t["value"].as_str().and_then(hex_u128).unwrap_or(0).to_string(),
                    token: String::new(), sym: String::new(), dec: String::new(),
                    gas: String::new(), gas_price: String::new(), nonce: String::new(),
                    err: "0".to_string(),
                });
            }
            return Some(out);
        }
        let params = format!(
            "module=account&action={}&address={addr}&page=1&offset={n}&sort=desc",
            kind.action());
        let v = self.es_call(&params).await?;
        if let Some(a) = v["result"].as_array() {
            return Some(a.iter().map(|r| Tx::from_es_row(addr, kind, r)).collect());
        }
        if v["message"].as_str().unwrap_or("").contains("No transactions found") {
            return Some(Vec::new());
        }
        None
    }

    /// ПОВНА історія гаманця по всіх заданих видах.
    ///
    /// Пагінація сторінками не працює: джерело віддає максимум 10 000 записів на
    /// адресу незалежно від `page`. Обхід — просування СТАРТОВОГО БЛОКУ. Граничний
    /// блок при цьому перечитується, тому тут же знімаються дублі.
    pub async fn txs_all(&self, addr: &str, kinds: &[Kind]) -> Pull {
        let mut out: Vec<Tx> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for k in kinds {
            let mut from_block = 0u64;
            for _win in 0..self.maxwin {
                let batch = match self.txs_page(*k, addr, from_block).await {
                    Some(b) => b,
                    None => return Pull { txs: out, complete: false },
                };
                if batch.is_empty() { break; }
                let n = batch.len();
                let mut last = from_block;
                for t in batch {
                    if t.block > last { last = t.block; }
                    if seen.insert(t.dedup_key()) { out.push(t); }
                }
                if n < self.page { break; }
                if last <= from_block { break; }   // захист від зациклення на одному блоці
                from_block = last;
            }
        }
        Pull { txs: out, complete: true }
    }

    // ─────────────────────────────── стан ───────────────────────────────

    /// Байткод адреси. `None` — не питали успішно; `Some("0x")` — EOA.
    pub async fn code(&self, addr: &str) -> Option<String> {
        match &self.backend {
            Backend::Node { .. } => {
                let v = self.rpc_call("eth_getCode", json!([addr, "latest"])).await?;
                v.as_str().map(|s| s.to_string())
            }
            _ => {
                let v = self.es_call(&format!(
                    "module=proxy&action=eth_getCode&address={addr}&tag=latest")).await?;
                let s = v["result"].as_str()?;
                // під лімітом сюди інколи потрапляє текст помилки — відсікаємо
                if !s.starts_with("0x") { return None; }
                Some(s.to_string())
            }
        }
    }

    pub async fn is_contract(&self, addr: &str) -> Option<bool> {
        Some(self.code(addr).await?.len() > 2)
    }

    /// Баланси ETH у wei. Розбиття на пачки — всередині (Etherscan бере до 20 адрес).
    /// Адреса, яку джерело не повернуло, просто відсутня у відповіді — викликач має
    /// звіряти довжину, а не вважати, що порядок збігається з входом.
    pub async fn eth_balances(&self, addrs: &[String]) -> Option<Vec<(String, u128)>> {
        let mut out = Vec::with_capacity(addrs.len());
        match &self.backend {
            Backend::Node { .. } => {
                for a in addrs {
                    let v = self.rpc_call("eth_getBalance", json!([a, "latest"])).await?;
                    let bal = v.as_str().and_then(hex_u128)?;
                    out.push((a.to_lowercase(), bal));
                }
            }
            _ => {
                for chunk in addrs.chunks(20) {
                    let v = self.es_call(&format!(
                        "module=account&action=balancemulti&address={}&tag=latest",
                        chunk.join(","))).await?;
                    let arr = v["result"].as_array()?;
                    for r in arr {
                        let a = r["account"].as_str().unwrap_or("").to_lowercase();
                        let b = r["balance"].as_str().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
                        if !a.is_empty() { out.push((a, b)); }
                    }
                }
            }
        }
        Some(out)
    }

    /// Баланс ERC-20 у сирих одиницях.
    pub async fn token_balance(&self, token: &str, addr: &str) -> Option<u128> {
        match &self.backend {
            Backend::Node { .. } => {
                let data = format!("0x70a08231{}", pad_addr(addr));   // balanceOf(address)
                hex_u128(&self.eth_call(token, &data).await?)
            }
            _ => {
                let v = self.es_call(&format!(
                    "module=account&action=tokenbalance&contractaddress={token}&address={addr}&tag=latest")).await?;
                // ЛИШЕ status=="1" — інакше під лімітом «0» піде в дані як факт
                if v["status"].as_str().unwrap_or("") != "1" { return None; }
                v["result"].as_str().and_then(|s| s.parse::<u128>().ok())
            }
        }
    }

    /// Загальна емісія токена у сирих одиницях.
    pub async fn token_supply(&self, token: &str) -> Option<u128> {
        match &self.backend {
            Backend::Node { .. } => hex_u128(&self.eth_call(token, "0x18160ddd").await?),   // totalSupply()
            _ => {
                let v = self.es_call(&format!(
                    "module=stats&action=tokensupply&contractaddress={token}")).await?;
                if v["status"].as_str().unwrap_or("") != "1" { return None; }
                v["result"].as_str().and_then(|s| s.parse::<u128>().ok())
            }
        }
    }

    pub async fn block_number(&self) -> Option<u64> {
        match &self.backend {
            Backend::Node { .. } => {
                let v = self.rpc_call("eth_blockNumber", json!([])).await?;
                v.as_str().and_then(hex_u64)
            }
            _ => {
                let v = self.es_call("module=proxy&action=eth_blockNumber").await?;
                v["result"].as_str().and_then(hex_u64)
            }
        }
    }

    pub async fn block(&self, n: u64, full: bool) -> Option<Value> {
        match &self.backend {
            Backend::Node { .. } => {
                self.rpc_call("eth_getBlockByNumber", json!([format!("0x{n:x}"), full])).await
            }
            _ => {
                let v = self.es_call(&format!(
                    "module=proxy&action=eth_getBlockByNumber&tag=0x{n:x}&boolean={full}")).await?;
                v.get("result").cloned()
            }
        }
    }

    /// Логи за діапазоном блоків. `topics` — до 4 позицій, `None` = будь-що.
    pub async fn logs(&self, from: u64, to: u64, topics: &[Option<String>]) -> Option<Vec<Value>> {
        match &self.backend {
            Backend::Node { .. } => {
                let t: Vec<Value> = topics.iter()
                    .map(|x| match x { Some(s) => Value::String(s.clone()), None => Value::Null })
                    .collect();
                let f = json!({
                    "fromBlock": format!("0x{from:x}"),
                    "toBlock": format!("0x{to:x}"),
                    "topics": t
                });
                let v = self.rpc_call("eth_getLogs", json!([f])).await?;
                v.as_array().cloned()
            }
            _ => {
                let mut p = format!("module=logs&action=getLogs&fromBlock={from}&toBlock={to}");
                for (i, t) in topics.iter().enumerate().take(4) {
                    if let Some(v) = t { p.push_str(&format!("&topic{i}={v}")); }
                }
                let v = self.es_call(&p).await?;
                v["result"].as_array().cloned()
            }
        }
    }

    // ─────────────────────── нода: історія по адресі ───────────────────────
    //
    // Сира нода НЕ вміє «дай усі транзакції адреси» — це вміння індексатора.
    // Тому кожен вид тягнеться своїм способом:
    //   ext — Otterscan (`ots_searchTransactionsAfter`), Erigon має його вбудованим;
    //   int — `trace_filter` (Erigon), обидва напрями окремими запитами;
    //   tok — `eth_getLogs` по topic0=Transfer з адресою в topic1/topic2.
    //
    // ⚠️ НЕ ПЕРЕВІРЕНО НА ЖИВІЙ НОДІ — своєї ноди ще немає (потрібен NVMe, ~2 ТБ).
    // Розбір полів тут за специфікаціями методів; перший запуск проти Erigon треба
    // звірити з Etherscan на десятку відомих адрес ПЕРЕД тим, як довіряти цифрам.
    // Відомі розбіжності, які треба буде закрити:
    //   · `ts` для int/tok нода не дає — час бере викликач із блоку;
    //   · `sym`/`dec` — метадані контракту, окремий виклик, а не поле логу.

    async fn node_txs_page(&self, kind: Kind, addr: &str, from_block: u64) -> Option<Vec<Tx>> {
        match kind {
            Kind::Ext => {
                let v = self.rpc_call("ots_searchTransactionsAfter",
                    json!([addr, from_block, self.page])).await?;
                let txs = v["txs"].as_array()?;
                let rcpts = v["receipts"].as_array().cloned().unwrap_or_default();
                let mut out = Vec::with_capacity(txs.len());
                for (i, t) in txs.iter().enumerate() {
                    let r = rcpts.get(i);
                    out.push(Tx {
                        w: addr.to_lowercase(),
                        kind,
                        ts: r.and_then(|x| x["timestamp"].as_str()).and_then(hex_u64).unwrap_or(0),
                        block: t["blockNumber"].as_str().and_then(hex_u64).unwrap_or(0),
                        hash: t["hash"].as_str().unwrap_or("").to_string(),
                        from: t["from"].as_str().unwrap_or("").to_lowercase(),
                        to: t["to"].as_str().unwrap_or("").to_lowercase(),
                        value: t["value"].as_str().and_then(hex_u128).unwrap_or(0).to_string(),
                        token: String::new(), sym: String::new(), dec: String::new(),
                        gas: t["gas"].as_str().and_then(hex_u64).unwrap_or(0).to_string(),
                        gas_price: t["gasPrice"].as_str().and_then(hex_u128).unwrap_or(0).to_string(),
                        nonce: t["nonce"].as_str().and_then(hex_u64).unwrap_or(0).to_string(),
                        err: match r.and_then(|x| x["status"].as_str()) {
                            Some("0x0") => "1".to_string(),
                            _ => "0".to_string(),
                        },
                    });
                }
                Some(out)
            }
            Kind::Int => {
                let mut out = Vec::new();
                for dir in ["fromAddress", "toAddress"] {
                    // ключ фільтра змінний, тому об'єкт збирається вручну:
                    // json! приймає лише сталі ключі
                    let mut f = serde_json::Map::new();
                    f.insert("fromBlock".to_string(), json!(format!("0x{from_block:x}")));
                    f.insert("toBlock".to_string(), json!("latest"));
                    f.insert(dir.to_string(), json!([addr]));
                    f.insert("count".to_string(), json!(self.page));
                    let v = self.rpc_call("trace_filter", json!([Value::Object(f)])).await?;
                    for t in v.as_array().cloned().unwrap_or_default() {
                        let a = &t["action"];
                        out.push(Tx {
                            w: addr.to_lowercase(),
                            kind,
                            ts: 0,   // trace_filter не дає часу — підставляє викликач
                            block: t["blockNumber"].as_u64().unwrap_or(0),
                            hash: t["transactionHash"].as_str().unwrap_or("").to_string(),
                            from: a["from"].as_str().unwrap_or("").to_lowercase(),
                            to: a["to"].as_str().unwrap_or("").to_lowercase(),
                            value: a["value"].as_str().and_then(hex_u128).unwrap_or(0).to_string(),
                            token: String::new(), sym: String::new(), dec: String::new(),
                            gas: a["gas"].as_str().and_then(hex_u64).unwrap_or(0).to_string(),
                            gas_price: String::new(), nonce: String::new(),
                            err: if t.get("error").is_some() { "1".to_string() } else { "0".to_string() },
                        });
                    }
                }
                Some(out)
            }
            Kind::Tok => {
                let head = self.block_number().await?;
                let padded = format!("0x{}", pad_addr(addr));
                let mut out = Vec::new();
                for pos in 1..=2usize {
                    let mut topics: Vec<Option<String>> =
                        vec![Some(TRANSFER_TOPIC.to_string()), None, None];
                    topics[pos] = Some(padded.clone());
                    for l in self.logs(from_block, head, &topics).await? {
                        let tp = l["topics"].as_array().cloned().unwrap_or_default();
                        // рівно 3 topic'и = ERC-20; 4 — це ERC-721 (id у topic3), не сума
                        if tp.len() != 3 { continue; }
                        let unpad = |x: &Value| {
                            let s = x.as_str().unwrap_or("").trim_start_matches("0x");
                            format!("0x{:0>40}", s.trim_start_matches('0'))
                        };
                        out.push(Tx {
                            w: addr.to_lowercase(),
                            kind,
                            ts: 0,
                            block: l["blockNumber"].as_str().and_then(hex_u64).unwrap_or(0),
                            hash: l["transactionHash"].as_str().unwrap_or("").to_string(),
                            from: unpad(&tp[1]),
                            to: unpad(&tp[2]),
                            value: l["data"].as_str().and_then(hex_u128).unwrap_or(0).to_string(),
                            token: l["address"].as_str().unwrap_or("").to_lowercase(),
                            sym: String::new(), dec: String::new(),
                            gas: String::new(), gas_price: String::new(), nonce: String::new(),
                            err: "0".to_string(),
                        });
                    }
                }
                Some(out)
            }
        }
    }
}
