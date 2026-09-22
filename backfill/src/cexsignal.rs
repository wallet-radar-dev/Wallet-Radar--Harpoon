//! cexsignal — сигнал ради #2 «кластер почав РОЗДАВАТИ», реалізований на живих даних.
//! Рада радила стежити за хабом 0x6fc48de8 → CEX, але хаб СПЛЯЧИЙ (останній вихідний ETH 2025-09-20),
//! тому спостережуване перенесено на ЖИВУ частину мережі — ядрові гаманці (+хаб/колектори/реле
//! у вотчлісті, раптом прокинуться). Дві ноги, бо rgflow каже, що мережа зливає через DEX:
//!   CEX-нога: вихідний переказ ядра на адресу з біржовою міткою (депозит = продаж/кешаут)
//!   DEX-нога: вихідний переказ priced-альта на КОНТРАКТ (своп = злив на ринок)
//! Поріг ради: вікно 24 год ≥ MULT(1.3) × середньодобової бази за BASE_DAYS(30) + абсолютна підлога.
//! Очікуваний лід за радою — 1–3 дні до просідання ціни (перевіряється біном leadlag).
//!
//! Стан: deploy/cexsignal.state.json {blocks:{addr:lastBlock}, daily:{день:{cex,dex}}, lastAlert}.
//! Перший запуск = БУТСТРАП: база береться з core-cex-events.jsonl (краул cexcrawl), алерт не шлеться.
//! Args: ROOT. Env: COV(5) MULT(1.3) BASE_DAYS(30) MIN_USD(200000) COOLDOWN_H(12) CONC(6) DRY(0)
use backfill::chain::{ChainSource, Kind};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// Інфраструктура мережі під наглядом: (адреса, підпис). У базі — `wallets`
/// з мітками, членство — тег `cexsignal:infra`.
fn infra() -> Vec<(String, String)> {
    let c = wr_store::open().expect("cexsignal: без сховища немає вотчліста");
    let mut q = c
        .prepare(
            "SELECT w.address, COALESCE(w.label, w.kind) FROM wallets w
             JOIN wallet_tags g ON g.chain=w.chain AND g.address=w.address
             WHERE g.tag='cexsignal:infra'",
        )
        .expect("запит вотчліста");
    let v: Vec<(String, String)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("читання")
        .filter_map(|r| r.ok())
        .collect();
    assert!(!v.is_empty(), "cexsignal: вотчліст порожній — спостерігати нема за ким");
    v
}

/// Пара, що утворює центр мережі.
fn treasury() -> Vec<String> {
    let c = wr_store::open().expect("cexsignal: без сховища немає казни");
    let mut q = c
        .prepare("SELECT address FROM wallet_tags WHERE tag='cexsignal:treasury'")
        .expect("запит казни");
    let v: Vec<String> =
        q.query_map([], |r| r.get(0)).expect("читання").filter_map(|r| r.ok()).collect();
    assert!(!v.is_empty(), "cexsignal: тег cexsignal:treasury порожній");
    v
}


/// інфраструктура мережі: хаб, колектори, реле (сплячі, але тримаємо в вотчлісті)

/// пара, що утворює центр мережі: рух тут = рух усієї мережі

const FAMS: [(&str, bool); 22] = [
    ("binance", false), ("kraken", false), ("okx", false), ("gopax", false), ("coinbase", false),
    ("bybit", false), ("kucoin", false), ("huobi", false), ("bitfinex", false), ("upbit", false),
    ("bithumb", false), ("mexc", false), ("bitget", false), ("paribu", false), ("btcturk", false),
    ("bingx", false), ("crypto.com", false), ("bitstamp", false), ("gemini", false),
    ("poloniex", false), ("gate", true), ("htx", true),
];
fn family(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    if n.contains("bridge") || n.contains("gateway") || n.contains("uniswap") || n.contains("pool") { return None; }
    for (t, prefix) in FAMS { if if prefix { n.starts_with(t) } else { n.contains(t) } { return Some(t); } }
    None
}
fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
// Протокол доступу до ланцюга (ротація ключів, ловля NOTOK у ТІЛІ, backoff, спільна квота)
// живе в backfill::chain. Тут лишається тільки логіка сигналу.
async fn tg(client: &reqwest::Client, token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() { eprintln!("[cexsignal] нема TELEGRAM креденшлів"); return false; }
    matches!(client.post(&format!("https://api.telegram.org/bot{token}/sendMessage"))
        .form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"), ("disable_web_page_preview", "true")]).send().await,
        Ok(r) if r.status().is_success())
}

/// Одна зафіксована подія: хто, кому, скільки, чого і коли.
///
/// Раніше подія була кортежем із шести полів і несла лише «нога + сімейство біржі».
/// Алерт із такої події міг сказати «CEX→binance $3М» — і не міг сказати, НА ЯКУ адресу,
/// СКІЛЬКИ токенів і за ЯКОЮ ціною. Саме цих трьох відповідей і бракувало, щоб з алерту
/// можна було діяти, а не лише хвилюватись.
#[derive(Clone)]
struct Ev {
    from: String,
    label: String,
    leg: String,            // CEX / DEX / КАЗНА / МІСТ / КОНТРАКТ
    asset: String,          // символ
    amount: f64,            // у токенах, не в доларах
    token: String,          // контракт (порожньо для ETH)
    usd: f64,
    to: String,             // адреса-отримувач
    dest_name: String,      // її мітка з labels-cache, якщо є
    venue: &'static str,    // сімейство біржі / «своп» / «міст» / «мережа»
    ts: u64,
    hash: String,
}

/// Ринковий контекст токена: ціна, ліквідність пулів, добовий обіг, капіталізація.
/// Без нього «$3М на біржу» — це число без масштабу: для LINK це шум, для дрібного
/// альта — весь денний обіг.
#[derive(Clone, Copy, Default)]
struct Mkt {
    price: f64,
    reserve_usd: f64,
    vol24: f64,
    mcap: f64,
}

/// Оцінка просідання ціни, якщо суму `usd` продати в пул із TVL `reserve_usd`.
///
/// Модель — константний добуток (x·y=k), як у Uniswap V2: продаж на суму X у пул,
/// де котирувальна сторона тримає R, лишає ціну на рівні (R/(R+X))² від початкової.
/// `reserve_usd` — TVL обох сторін, тож котирувальна половина ≈ TVL/2.
///
/// Це ОЦІНКА ЗВЕРХУ по одному свопу: реальний продаж дроблять, частина йде через V3
/// з концентрованою ліквідністю, частина — поза пулом. Але порядок величини вона дає
/// правильний, і саме він відрізняє «продадуть непомітно» від «обвалять на третину».
fn pool_impact_pct(usd: f64, reserve_usd: f64) -> Option<f64> {
    if reserve_usd <= 0.0 || usd <= 0.0 { return None; }
    let r = reserve_usd / 2.0;
    let ratio = r / (r + usd);
    Some(100.0 * (1.0 - ratio * ratio))
}

fn human_usd(v: f64) -> String {
    if v >= 1e9 { format!("${:.2}млрд", v / 1e9) }
    else if v >= 1e6 { format!("${:.2}млн", v / 1e6) }
    else if v >= 1e3 { format!("${:.1}тис", v / 1e3) }
    else { format!("${v:.0}") }
}

fn human_amt(v: f64) -> String {
    if v >= 1e9 { format!("{:.2} млрд", v / 1e9) }
    else if v >= 1e6 { format!("{:.2} млн", v / 1e6) }
    else if v >= 1e3 { format!("{:.1} тис", v / 1e3) }
    else if v >= 1.0 { format!("{v:.2}") }
    else { format!("{v:.6}") }
}

/// Посилання на адресу в Etherscan: щоб з алерту можна було відразу перейти й дивитись.
fn addr_link(a: &str, text: &str) -> String {
    format!("<a href=\"https://etherscan.io/address/{a}\">{text}</a>")
}

/// Скорочення адреси до впізнаваного вигляду. Повна адреса в тексті алерту нечитабельна,
/// але за посиланням вона є цілком — тож звірити завжди можна.
fn short(a: &str) -> String {
    if a.len() >= 42 { format!("{}…{}", &a[0..8], &a[a.len() - 4..]) } else { a.to_string() }
}

/// Час події у UTC. Крейт зібрано з chrono без фічі `clock`, тож годинник беремо із
/// системи, а chrono лише форматує — так само, як у fwdindex.
fn ts_human(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| d.format("%d.%m %H:%M UTC").to_string())
        .unwrap_or_else(|| "час невідомий".to_string())
}

/// Ціна з розрядністю під масштаб: $2426.95 для ETH, але $0.00001183 для мемкоїна.
/// Фіксовані два знаки перетворювали б усі дрібні токени на «$0.00» — тобто на нуль
/// інформації саме там, де концентрація найбільша.
/// Відсоток із розрядністю під величину: «14%», але «0.3%» замість «0%».
/// Округлення до цілого перетворювало все дрібніше за піввідсотка на нуль — тобто
/// стирало саме ту межу, за якою обсяг перестає бути помітним.
fn pct(v: f64) -> String {
    if v >= 10.0 { format!("{v:.0}%") }
    else if v >= 1.0 { format!("{v:.1}%") }
    else { format!("{v:.2}%") }
}

fn fmt_price(p: f64) -> String {
    if p <= 0.0 { "?".to_string() }
    else if p >= 100.0 { format!("{p:.2}") }
    else if p >= 1.0 { format!("{p:.3}") }
    else if p >= 0.01 { format!("{p:.5}") }
    else { format!("{p:.8}") }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let cov: u64 = std::env::var("COV").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let mult: f64 = std::env::var("MULT").ok().and_then(|s| s.parse().ok()).unwrap_or(1.3);
    let base_days: i64 = std::env::var("BASE_DAYS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    let min_usd: f64 = std::env::var("MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(200_000.0);
    let cooldown_h: u64 = std::env::var("COOLDOWN_H").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let dry = std::env::var("DRY").map(|v| v == "1").unwrap_or(false);
    let bf = format!("{root}/dataset/backfill");
    // Джерело даних ланцюга. Ключі, ліміти й ретраї — його турбота; тут про них не знають.
    let src = Arc::new(ChainSource::from_env(&root, 1));
    let (token, chat) = (envv(&root, "TELEGRAM_BOT_TOKEN"), envv(&root, "TELEGRAM_CHAT_ID"));

    // --- вотчліст ---
    let lead: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/rglead.json")).expect("rglead"))).expect("rglead parse");
    let mut watch: Vec<(String, String)> = Vec::new();
    for w in lead["wallets"].as_array().cloned().unwrap_or_default() {
        if w["coverage"].as_u64().unwrap_or(0) >= cov && w["kind"].as_str() != Some("contract") {
            if let Some(a) = w["w"].as_str() {
                watch.push((a.to_lowercase(), format!("ядро ({} активів, {})", w["coverage"].as_u64().unwrap_or(0), w["tier"].as_str().unwrap_or(""))));
            }
        }
    }
    for (a, l) in infra() { watch.push((a, l)); }
    // РОЛЬОВИЙ ВОТЧЛІСТ (walletrole): не лише ядро rglead, а всі, чия ФУНКЦІЯ значуща для сигналу —
    // кого озброюють (отримувач/циркулятор), хто зливає (продавець) і де лежать гроші (холдер).
    let roles_want: HashSet<String> = std::env::var("ROLES")
        .unwrap_or_else(|_| "ОТРИМУВАЧ,ЦИРКУЛЯТОР,ПРОДАВЕЦЬ,ХОЛДЕР".into())
        .split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let seen_w: HashSet<String> = watch.iter().map(|(a, _)| a.clone()).collect();
    if let Ok(f) = File::open(format!("{bf}/wallet-roles.json")) {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            let mut added = 0;
            for x in v["wallets"].as_array().cloned().unwrap_or_default() {
                let r = x["role"].as_str().unwrap_or("");
                if !roles_want.contains(r) { continue; }
                let a = x["w"].as_str().unwrap_or("").to_lowercase();
                if a.len() != 42 || seen_w.contains(&a) { continue; }
                watch.push((a, format!("{r} ({:.0} ETH)", x["balEth"].as_f64().unwrap_or(0.0))));
                added += 1;
            }
            eprintln!("[cexsignal] додано за роллю: {added}");
        }
    }

    // --- довідники ---
    let lbl: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(File::open(format!("{root}/dataset/labels-cache.json")).unwrap())).unwrap_or_default();
    let mut ex: HashMap<String, &'static str> = HashMap::new();
    for (a, x) in &lbl { if let Some(f) = family(x["name"].as_str().unwrap_or("")) { ex.insert(a.to_lowercase(), f); } }
    // МОНІТОР РАНІШЕ ЗАВИЩУВАВ ПРОДАЖІ ВДЕСЯТЕРО: будь-який вихід токена на контракт рахувався
    // зливом. Розбір на рівні транзакцій (flowclass) показав, що з $93.4М такого виходу продажем
    // є лише $9.06М — решта ліквідність, мости й немічені контракти. Тому тут розділяємо за
    // природою призначення й тривожимо лише на СПРАВЖНІЙ своп.
    let swap_venue = |n: &str| { let l = n.to_lowercase();
        ["uniswap","sushiswap","1inch","paraswap","curve","balancer","router","pool","cow","kyber",
         "dodo","pancake","matcha","odos","swap","aggregator"].iter().any(|t| l.contains(t)) };
    let bridge_venue = |n: &str| { let l = n.to_lowercase();
        ["bridge","arbitrum","optimism","zksync","polygon","hop","across","orbiter","stargate","synapse","celer","wormhole","gateway"].iter().any(|t| l.contains(t)) };
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/wallet-kind.json")).unwrap())) {
        for (a, k) in &v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    // РИНКОВИЙ КОНТЕКСТ (tokliq/GeckoTerminal): ліквідність пулів, добовий обіг, капіталізація.
    // Без нього сума в доларах не має масштабу — «$3М на біржу» для LINK і для дрібного
    // альта означають протилежні речі. Файл необовʼязковий: якщо його нема, алерт просто
    // втрачає розділ наслідків, але лишається конкретним щодо адрес і сум.
    let mut mkt: HashMap<String, Mkt> = HashMap::new();
    match File::open(format!("{bf}/token-liquidity.json")) {
        Ok(f) => {
            if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
                for (k, x) in v["tokens"].as_object().cloned().unwrap_or_default() {
                    mkt.insert(k.to_lowercase(), Mkt {
                        price: x["price"].as_f64().unwrap_or(0.0),
                        reserve_usd: x["reserveUsd"].as_f64().unwrap_or(0.0),
                        vol24: x["vol24"].as_f64().unwrap_or(0.0),
                        mcap: x["mcap"].as_f64().unwrap_or(0.0),
                    });
                }
            }
        }
        Err(_) => eprintln!("[cexsignal] ⚠ нема token-liquidity.json — алерт без розділу наслідків (запусти tokliq)"),
    }
    eprintln!("[cexsignal] ринковий контекст: {} токенів", mkt.len());

    // ціни: ринкові (tokprices/DefiLlama) + осудний відкіт на ончейн-своп-ціну.
    // ЧОМУ не onchain-prices напряму: там COMP $41046 замість $17 → 1552 COMP «коштували» $63.7М.
    let tp: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/token-prices.json")).expect("token-prices.json — спершу tokprices"))).expect("token-prices parse");
    let mut px: HashMap<String, f64> = tp["tokens"].as_object().map(|m| m.iter().filter_map(|(k, v)| v["usd"].as_f64().map(|u| (k.to_lowercase(), u))).collect()).unwrap_or_default();
    if let Ok(oc) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(File::open(format!("{bf}/onchain-prices.json")).unwrap())) {
        for (k, v) in &oc {
            let k = k.to_lowercase();
            if px.contains_key(&k) { continue; }
            if let Some(u) = v["usd"].as_f64() { if u > 0.0 && u < 10_000.0 { px.insert(k, u); } }
        }
    }
    let eth_usd: f64 = tp["eth"].as_f64().filter(|e| *e > 0.0).unwrap_or_else(|| {
        serde_json::from_reader::<_, Value>(BufReader::new(File::open(format!("{bf}/capital-summary.json")).unwrap())).ok()
            .and_then(|v| v["eth_price"].as_f64()).unwrap_or(1860.0)
    });

    // --- стан ---
    let statef = format!("{root}/deploy/cexsignal.state.json");
    let mut state: Value = fs::read_to_string(&statef).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    let bootstrap = state.get("blocks").is_none();
    let mut blocks: HashMap<String, u64> = state["blocks"].as_object().map(|m| m.iter().filter_map(|(k, v)| v.as_u64().map(|b| (k.clone(), b))).collect()).unwrap_or_default();
    let mut daily: HashMap<String, (f64, f64)> = state["daily"].as_object().map(|m| m.iter()
        .map(|(k, v)| (k.clone(), (v["cex"].as_f64().unwrap_or(0.0), v["dex"].as_f64().unwrap_or(0.0)))).collect()).unwrap_or_default();
    let last_alert = state["lastAlert"].as_u64().unwrap_or(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    // клієнт лишається лише для Telegram — до ланцюга ходить src
    let client = Arc::new(reqwest::Client::builder().user_agent("wallet-radar-cexsignal/0.1").pool_max_idle_per_host(conc).timeout(std::time::Duration::from_secs(40)).build().unwrap());

    // спільний «поточний блок»: потрібен і для бутстрапу, і для НОВИХ адрес у вотчлісті —
    // інакше щойно додана адреса сканувалась би з першого блока (годинний прогін + хибні лічильники)
    // 0 = не дізнались. Нижче це означає «нові адреси скануються з початку», що для
    // монітора гірше за пропуск прогону, але поведінка та сама, що була.
    let bnow = src.block_number().await.unwrap_or(0);
    // БУТСТРАП: історія з краулу cexcrawl (CEX-нога), блоки = поточний → без ретро-алертів
    if bootstrap {
        if let Ok(f) = File::open(format!("{bf}/core-cex-events.jsonl")) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["dir"] != "out" { continue; }
                let ts = v["ts"].as_u64().unwrap_or(0); let usd = v["usd"].as_f64().unwrap_or(0.0);
                if ts == 0 || usd <= 0.0 { continue; }
                let d = (ts / 86400).to_string();
                daily.entry(d).or_insert((0.0, 0.0)).0 += usd;
            }
        }
        for (a, _) in &watch { blocks.insert(a.clone(), bnow); }
        eprintln!("[cexsignal] БУТСТРАП: база {} днів з core-cex-events.jsonl, стартовий блок {}", daily.len(), bnow);
    }

    // --- інкрементальний скан ---
    let sem = Arc::new(Semaphore::new(conc));
    let found: Arc<Mutex<Vec<Ev>>> = Arc::new(Mutex::new(Vec::new()));
    let newblocks: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    let unchecked = Arc::new(AtomicUsize::new(0));   // адреси, які цього прогону перевірити не вдалось
    let ex = Arc::new(ex); let kind = Arc::new(kind); let px = Arc::new(px); let lbl2 = Arc::new(lbl.clone());
    let mut hs = Vec::new();
    for (addr, label) in watch.clone() {
        let from = blocks.get(&addr).copied().unwrap_or(bnow) + 1;   // нова адреса → з поточного блока
        let (src, sem, found, newblocks, ex, kind, px, lbl2, unchecked) =
            (src.clone(), sem.clone(), found.clone(), newblocks.clone(), ex.clone(), kind.clone(), px.clone(), lbl2.clone(), unchecked.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            let mut maxb = from.saturating_sub(1);
            for k in [Kind::Ext, Kind::Tok] {
                // При стійкому ліміті курсор НЕ зсуваємо: адреса не потрапляє в newblocks,
                // тож наступний прогін перечитає той самий діапазон і подій не загубить.
                let Some(txs) = src.txs_page(k, &addr, from).await else {
                    unchecked.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                for t in &txs {
                    if t.block > maxb { maxb = t.block; }
                    if t.err == "1" { continue; }
                    if t.from != addr { continue; }   // лише ВИХІДНІ
                    let to = t.to.clone();
                    if to.is_empty() { continue; }
                    let (asset, amount, usd) = if k == Kind::Ext {
                        let v: f64 = t.value.parse::<f64>().unwrap_or(0.0) / 1e18;
                        if v <= 0.0 { continue; }
                        ("ETH".to_string(), v, v * eth_usd)
                    } else {
                        let dec: i32 = t.dec.parse().unwrap_or(18);
                        let raw: f64 = t.value.parse::<f64>().unwrap_or(0.0);
                        let amt = raw / 10f64.powi(dec);
                        if amt <= 0.0 { continue; }
                        let sym = if t.sym.is_empty() { "?" } else { t.sym.as_str() };
                        (sym.to_string(), amt, amt * px.get(&t.token).copied().unwrap_or(0.0))
                    };
                    // КАЗНА: для пари концентратор/дистрибʼютор важливий БУДЬ-ЯКИЙ вихід, навіть на
                    // звичайні гаманці — саме так виглядає роздача мережі, а не продаж на біржу
                    let dest_name = lbl2.get(&to).and_then(|x| x["name"].as_str()).unwrap_or("").to_string();
                    let (leg, venue) = if treasury().iter().any(|t| t == &addr) {
                        ("КАЗНА", "мережа")
                    } else if let Some(fam) = ex.get(&to) {
                        ("CEX", *fam)
                    } else if kind.get(&to).map(|v| v == "contract").unwrap_or(false) && k == Kind::Tok && usd > 0.0 {
                        if bridge_venue(&dest_name) { ("МІСТ", "міст") }
                        else if swap_venue(&dest_name) { ("DEX", "своп") }
                        else { ("КОНТРАКТ", "немічений") }
                    } else { continue };
                    found.lock().unwrap().push(Ev {
                        from: addr.clone(), label: label.clone(), leg: leg.to_string(),
                        asset, amount, token: t.token.clone(), usd,
                        to: to.clone(), dest_name, venue, ts: t.ts, hash: t.hash.clone(),
                    });
                }
            }
            newblocks.lock().unwrap().insert(addr, maxb);
        }));
    }
    for h in hs { let _ = h.await; }

    // --- зведення за сьогодні ---
    let evs = found.lock().unwrap().clone();
    let today = (now / 86400).to_string();
    let (mut cex_new, mut dex_new, mut treas_new, mut other_new) = (0.0, 0.0, 0.0, 0.0);
    // Подія лягає в день САМОЇ ПОДІЇ, а не в день, коли її помітили.
    //
    // У штатному режимі (прогін раз на 20 хв) різниці нема — усе знайдене й так сьогоднішнє.
    // Різниця з'являється після простою монітора, і вона руйнівна: з 19.08 по 26.08 бін не
    // працював (ключі переїхали в deploy/backfill.env), і перший же прогін зсипав би тижневий
    // хвіст — $404М — в один сьогоднішній кошик. Це і хибний алерт ×26, і зіпсована 30-денна
    // база на наступний місяць, бо завтра цей кошик стане частиною бази.
    for e in &evs {
        let (leg, usd, ts) = (&e.leg, &e.usd, &e.ts);
        let day = (if *ts > 0 { *ts } else { now } / 86400).to_string();
        {
            let e = daily.entry(day.clone()).or_insert((0.0, 0.0));
            match leg.as_str() { "CEX" => e.0 += usd, "DEX" => e.1 += usd, _ => {} }
        }
        if day != today { continue; }   // пороги рахуються лише по СЬОГОДНІШНІХ подіях
        match leg.as_str() { "CEX" => cex_new += usd, "DEX" => dex_new += usd, "КАЗНА" => treas_new += usd, _ => other_new += usd }
    }
    let _ = (cex_new, dex_new);   // у daily вже враховані вище
    for (a, b) in newblocks.lock().unwrap().iter() { if *b > 0 { blocks.insert(a.clone(), *b); } }

    // база: середньодобове за BASE_DAYS днів ДО сьогодні
    let today_i: i64 = today.parse().unwrap();
    let mut base_cex = 0.0; let mut base_dex = 0.0;
    for d in (today_i - base_days)..today_i {
        if let Some((c, x)) = daily.get(&d.to_string()) { base_cex += c; base_dex += x; }
    }
    base_cex /= base_days as f64; base_dex /= base_days as f64;
    let (cur_cex, cur_dex) = daily.get(&today).copied().unwrap_or((0.0, 0.0));
    let ratio = |cur: f64, base: f64| if base > 0.0 { cur / base } else if cur > 0.0 { f64::INFINITY } else { 0.0 };
    let (rc, rd) = (ratio(cur_cex, base_cex), ratio(cur_dex, base_dex));
    let fire_cex = cur_cex >= min_usd && rc >= mult;
    let fire_dex = cur_dex >= min_usd && rd >= mult;
    // казна: поріг абсолютний (бази нема — рухи рідкі, але кожен значущий)
    let treas_min: f64 = std::env::var("TREASURY_MIN_USD").ok().and_then(|s| s.parse().ok()).unwrap_or(2_000_000.0);
    let fire_treas = treas_new >= treas_min;

    eprintln!("[cexsignal] спостережень: {} · CEX ${:.0} (×{:.2}) · DEX-своп ${:.0} (×{:.2}) · КАЗНА ${:.0} · інші контракти/мости ${:.0} (не тривожать)",
        evs.len(), cur_cex, rc, cur_dex, rd, treas_new, other_new);
    // Тиша при неповному покритті — не сигнал. Без цього рядка залімічений прогін виглядав
    // би як «спокійний день»: адреси не перевірені, а звіт однаково рапортує нулі.
    let unck = unchecked.load(Ordering::Relaxed);
    if unck > 0 {
        eprintln!("[cexsignal] ⚠ НЕПОВНЕ ПОКРИТТЯ: {unck} із {} адрес не перевірено (ліміт Etherscan) — курсор їм не зсунуто, наступний прогін перечитає той самий діапазон. Нулі нижче стосуються лише перевірених.",
            watch.len());
    }
    if std::env::var("DEBUG").map(|v| v == "1").unwrap_or(false) {
        let mut ev = evs.clone();
        ev.sort_by(|a, b| b.usd.partial_cmp(&a.usd).unwrap());
        // DEBUG_N — скільки подій друкувати; DEBUG_LEG — фільтр ноги (CEX/DEX/КАЗНА),
        // бо великі перекази казни інакше витісняють із топу все інше
        let dn: usize = std::env::var("DEBUG_N").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
        let dleg = std::env::var("DEBUG_LEG").unwrap_or_default();
        for e in ev.iter().filter(|e| dleg.is_empty() || e.leg == dleg).take(dn) {
            eprintln!("   {:<8} ${:>14.0} {:<8} {} → {} [{}]  {}", e.leg, e.usd, e.asset, &e.from[0..12], e.venue, e.label, &e.to[0..12]);
        }
    }

    let cooled = now.saturating_sub(last_alert) >= cooldown_h * 3600;
    let test = std::env::var("TEST").map(|v| v == "1").unwrap_or(false);  // разова перевірка доставки
    // ПОПЕРЕДНІЙ ПЕРЕГЛЯД: за DRY поріг рахується як завжди, повідомлення складається
    // повністю, але йде в stderr, а не в канал. Без цього налаштувати текст алерту можна
    // було б лише розсилаючи його собі — тобто привчаючи себе гортати алерти не читаючи.
    let fired = fire_cex || fire_dex || fire_treas;
    let will_send = (fired && !bootstrap && cooled && !dry) || test;
    let preview = dry && fired && !test;
    if will_send || preview {
        // Події СЬОГОДНІШНЬОГО дня — саме вони перейшли поріг. Учорашні в алерт не
        // потрапляють: інакше після простою монітора лист розпухає тижневим хвостом.
        let mut day_evs: Vec<&Ev> = evs.iter().filter(|e| (e.ts / 86400).to_string() == today).collect();
        day_evs.sort_by(|a, b| b.usd.partial_cmp(&a.usd).unwrap());

        let head = if test { "🧪 <b>ТЕСТ ДОСТАВКИ cexsignal</b> (не сигнал — перевірка каналу)" }
        else if fire_treas { "🏦 <b>РУХ КАЗНИ МЕРЕЖІ</b> — концентратор/дистрибʼютор рухає всю мережу" }
        else { match (fire_cex, fire_dex) {
            (true, true) => "🔴 <b>РОЗДАЧА: ОБИДВІ НОГИ</b> — злив у пули + завід на біржі",
            (true, false) => "🔴 <b>ЗАВІД НА БІРЖІ</b> — пізня стадія: готують кешаут",
            _ => "🟠 <b>ЗЛИВ У ПУЛИ</b> — рання стадія: продають на ринку",
        }};

        let mut m = String::new();
        m.push_str(head);
        m.push_str(&format!("\n<i>{}</i>\n", ts_human(now)));

        // ── 1. ПОІМЕННО: хто, кому, скільки, за якою ціною ──
        let show: usize = std::env::var("ALERT_N").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
        m.push_str("\n<b>ХТО І КУДИ ПЕРЕВІВ</b>\n");
        if day_evs.is_empty() {
            m.push_str("• сьогоднішніх переказів нема — спрацював абсолютний поріг казни\n");
        }
        for (i, e) in day_evs.iter().take(show).enumerate() {
            let dest = if !e.dest_name.is_empty() { e.dest_name.clone() }
                       else if e.venue != "немічений" { e.venue.to_string() }
                       else { "немічена адреса".to_string() };
            let price = if e.amount > 0.0 { e.usd / e.amount } else { 0.0 };
            m.push_str(&format!(
                "\n{}. <b>{}</b> · {} · {}\n   від {}\n   → {} <b>{}</b>\n   {} {} = {} · ціна ${}\n   <a href=\"https://etherscan.io/tx/{}\">дивитись транзакцію</a>\n",
                i + 1, e.leg, e.label, ts_human(e.ts),
                addr_link(&e.from, &short(&e.from)),
                addr_link(&e.to, &short(&e.to)), dest,
                human_amt(e.amount), e.asset, human_usd(e.usd), fmt_price(price),
                e.hash));
        }
        if day_evs.len() > show {
            let rest: f64 = day_evs.iter().skip(show).map(|e| e.usd).sum();
            m.push_str(&format!("\n…і ще {} переказів на {}\n", day_evs.len() - show, human_usd(rest)));
        }

        // ── 2. КУДИ САМЕ: зведення по адресах-отримувачах ──
        let mut by_dest: HashMap<String, (f64, usize, String)> = HashMap::new();
        for e in &day_evs {
            let name = if !e.dest_name.is_empty() { e.dest_name.clone() } else { e.venue.to_string() };
            let ent = by_dest.entry(e.to.clone()).or_insert((0.0, 0, name));
            ent.0 += e.usd;
            ent.1 += 1;
        }
        let mut dv: Vec<(String, (f64, usize, String))> = by_dest.into_iter().collect();
        dv.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap());
        if !dv.is_empty() {
            m.push_str("\n<b>КУДИ ПІШЛО (за адресами-отримувачами)</b>\n");
            for (addr, (usd, n, name)) in dv.iter().take(6) {
                m.push_str(&format!("• {} <b>{}</b> — {} за {} переказ(ів)\n",
                    addr_link(addr, &short(addr)), name, human_usd(*usd), n));
            }
        }

        // ── 3. НАСЛІДКИ: масштаб суми відносно ринку саме цього активу ──
        let mut by_asset: HashMap<String, (f64, f64, String)> = HashMap::new();
        for e in &day_evs {
            let ent = by_asset.entry(e.asset.clone()).or_insert((0.0, 0.0, e.token.clone()));
            ent.0 += e.usd;
            ent.1 += e.amount;
        }
        let mut av: Vec<(String, (f64, f64, String))> = by_asset.into_iter().collect();
        av.sort_by(|a, b| b.1.0.partial_cmp(&a.1.0).unwrap());
        let mut impact_lines: Vec<String> = Vec::new();
        for (sym, (usd, amt, tok)) in av.iter().take(5) {
            // ЦІНА — ОДНА НА ВЕСЬ ЛИСТ. Раніше подія показувала ціну з token-prices
            // (DefiLlama), а розділ масштабу — з token-liquidity (GeckoTerminal), і ті самі
            // ENA виходили і по $0.136, і по $0.156. Лист, що сам собі суперечить, не
            // читають — беремо ту саму ціну, з якої порахований долар у самій події.
            let px_now = if *amt > 0.0 { usd / amt } else { 0.0 };

            // ETH — базовий актив: у token-liquidity його нема, та й глибина тут не
            // обмеження. Мовчати про найбільшу позицію листа («ринкових даних нема»)
            // гірше, ніж сказати прямо, що обмеження не в ній.
            if tok.is_empty() || sym == "ETH" {
                impact_lines.push(format!(
                    "• <b>ETH</b> {}: ціна ${} · глибина ринку ETH такий обсяг не помічає — обмеження не тут",
                    human_usd(*usd), fmt_price(px_now)));
                continue;
            }

            let Some(mk) = mkt.get(tok.as_str()) else {
                impact_lines.push(format!("• <b>{sym}</b> {}: ціна ${} · ліквідності й обігу нема в даних — масштаб не оцінити",
                    human_usd(*usd), fmt_price(px_now)));
                continue;
            };
            // якщо кількість невідома (нуль), ціну беремо з ринкових даних — інакше
            // рядок показав би «ціна $?» там, де вона насправді відома
            let px_now = if px_now > 0.0 { px_now } else { mk.price };
            let mut parts: Vec<String> = vec![format!("ціна ${}", fmt_price(px_now))];
            if mk.vol24 > 0.0 {
                parts.push(format!("{} денного обігу ({})", pct(100.0 * usd / mk.vol24), human_usd(mk.vol24)));
            }
            if mk.reserve_usd > 0.0 {
                parts.push(format!("{} ліквідності пулів ({})", pct(100.0 * usd / mk.reserve_usd), human_usd(mk.reserve_usd)));
            }
            // частку капіталізації показуємо лише коли вона не нуль після округлення:
            // «0.00% капіталізації» — рядок, що не несе нічого
            if mk.mcap > 0.0 && 100.0 * usd / mk.mcap >= 0.01 {
                parts.push(format!("{} капіталізації", pct(100.0 * usd / mk.mcap)));
            }
            let mut line = format!("• <b>{sym}</b> {}: {}", human_usd(*usd), parts.join(" · "));
            if let Some(imp) = pool_impact_pct(*usd, mk.reserve_usd) {
                // Завід на біржу тисне на книгу заявок, а не на AMM. Тому формулювання
                // умовне: це відповідь на «а якби цей обсяг пішов у пул одним свопом».
                if imp >= 5.0 {
                    line.push_str(&format!("\n   ↳ у пул одним свопом це НЕ продати: просідання <b>~{imp:.0}%</b> (оцінка x·y=k)"));
                } else {
                    line.push_str(&format!("\n   ↳ пул такий обсяг тримає: просідання ~{imp:.1}% (оцінка x·y=k)"));
                }
            }
            impact_lines.push(line);
        }
        if !impact_lines.is_empty() {
            m.push_str("\n<b>МАСШТАБ ВІДНОСНО РИНКУ</b>\n");
            m.push_str(&impact_lines.join("\n"));
            m.push_str("\n");
        }

        // ── 4. ІНТЕРПРЕТАЦІЯ — на ВИМІРЯНИХ базах, не на здогадах ──
        m.push_str("\n<b>ЩО ЦЕ ЗНАЧИТЬ</b>\n");
        m.push_str(&format!("• На біржі сьогодні: {} (×{:.2} до бази {}/д за {base_days} днів)\n",
            human_usd(cur_cex), rc, human_usd(base_cex)));
        m.push_str(&format!("• У пули/свопи сьогодні: {} (×{:.2} до бази {}/д)\n",
            human_usd(cur_dex), rd, human_usd(base_dex)));
        if fire_treas {
            m.push_str(&format!("• Казна рухнула {} — це роздача членам мережі, а не продаж: отримувачі здебільшого звичайні гаманці, не біржі\n",
                human_usd(treas_new)));
        }
        if fire_cex {
            m.push_str("• Завід на біржу = токени стали доступні до продажу, але це НЕ доказ продажу. За нашим власним виміром (cexemis) медіана ціни після таких заводів −7.6% відносно ринку: запас частіше тисне, ніж підіймає\n");
        }
        if fire_dex {
            m.push_str("• Злив у пул = продаж уже стався, ціна вже його всмоктала\n");
        }
        m.push_str("• ⚠️ ЧЕСНО ПРО МЕЖІ: потік на біржі НЕ передбачає напрям ціни — на 145 тис. спостережень (flowbt) AUC 0.49, рівень монетки. Це діагноз стану мережі, а не прогноз. Діяти варто від концентрації та ліквідності вище, а не від самого факту заводу\n");
        m.push_str(&format!("\n<i>поріг: ×{mult} за 24 год і не менше {}; перевірено {} із {} адрес</i>",
            human_usd(min_usd), watch.len() - unck, watch.len()));

        if preview {
            eprintln!("[cexsignal] ПЕРЕГЛЯД АЛЕРТУ ({} символів, {} подій) — не надіслано:\n{m}",
                m.chars().count(), day_evs.len());
        } else if tg(&client, &token, &chat, &m).await {
            state["lastAlert"] = json!(now);
            eprintln!("[cexsignal] АЛЕРТ надіслано ({} символів, {} подій)", m.chars().count(), day_evs.len());
        } else {
            eprintln!("[cexsignal] ⚠ АЛЕРТ НЕ надіслано (Telegram відмовив)");
        }
    } else if fired {
        eprintln!("[cexsignal] поріг перейдено, але алерт пропущено (бутстрап={bootstrap}, кулдаун={})", !cooled);
    }

    // DRY — діагностика: стан НЕ чіпаємо. Інакше перескан із відмотаними блоками записує
    // денний лічильник, і паралельний cron-запуск рахує ті самі події вдруге (звідси роздуті добові суми).
    if dry { eprintln!("[cexsignal] DRY: стан не змінено"); return; }
    // обрізаємо історію до 120 днів
    daily.retain(|d, _| d.parse::<i64>().map(|x| today_i - x <= 120).unwrap_or(false));
    state["blocks"] = json!(blocks);
    state["daily"] = json!(daily.iter().map(|(d, (c, x))| (d.clone(), json!({"cex": c, "dex": x}))).collect::<serde_json::Map<String, Value>>());
    state["watch"] = json!(watch.len());
    state["updated"] = json!(now);
    let _ = fs::write(&statef, serde_json::to_string_pretty(&state).unwrap());
    eprintln!("[cexsignal] стан збережено ({} адрес у вотчлісті, {} днів історії)", watch.len(), daily.len());
}
