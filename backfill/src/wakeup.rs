//! wakeup — ДЕТЕКТОР ПРОБУДЖЕННЯ сплячих гаманців мережі.
//! 1427 гаманців ролі БЕЗ-ETH-РУХУ не рухали нативний ETH понад пів року і тримають ~97 000 ETH. Будь-який рух звідти —
//! це не робочий цикл (транзит/роздача), а справжній вихід або підготовка до нього. Хибних
//! спрацювань майже не буває за визначенням: подія = «мовчав місяцями, раптом переказ».
//! Перевіряє останню транзакцію кожного (txlist offset=1 sort=desc) і порівнює зі збереженою.
//! Стан: deploy/wakeup.state.json. Telegram при пробудженні. Запускати раз на добу (cron).
//! Args: ROOT. Env: CONC(8) MIN_ETH(1 — ігнорувати дрібні гаманці) TEST(0)
use backfill::chain::{ChainSource, Kind};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

fn envv(root: &str, k: &str) -> String {
    fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}="))).map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}
async fn tg(client: &reqwest::Client, token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() { return false; }
    matches!(client.post(&format!("https://api.telegram.org/bot{token}/sendMessage"))
        .form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"), ("disable_web_page_preview", "true")]).send().await,
        Ok(r) if r.status().is_success())
}

/// Пробудження: не лише «хто і скільки мовчав», а й чим саме ожив.
struct Woke {
    addr: String,
    bal: f64,
    prev: u64,
    last: u64,
    to: String,
    dest: String,
    amt: f64,
    hash: String,
}

fn human_usd(v: f64) -> String {
    if v >= 1e6 { format!("${:.2}млн", v / 1e6) }
    else if v >= 1e3 { format!("${:.1}тис", v / 1e3) }
    else { format!("${v:.0}") }
}

fn short(a: &str) -> String {
    if a.len() >= 42 { format!("{}\u{2026}{}", &a[0..8], &a[a.len() - 4..]) } else { a.to_string() }
}

fn addr_link(a: &str) -> String {
    format!("<a href=\"https://etherscan.io/address/{a}\">{}</a>", short(a))
}

/// Чи є мітка адреси біржею. Список той самий, що в cexsignal: розходження між
/// двома списками означало б, що один монітор бачить кешаут, а другий — ні.
fn is_exchange(name: &str) -> bool {
    let n = name.to_lowercase();
    ["binance", "coinbase", "okx", "kraken", "bybit", "kucoin", "gate", "mexc", "bitget",
     "huobi", "htx", "bitfinex", "upbit", "bithumb", "paribu", "btcturk", "bingx",
     "crypto.com", "bitstamp", "gemini", "poloniex"].iter().any(|c| n.contains(c))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let conc: usize = std::env::var("CONC").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let min_eth: f64 = std::env::var("MIN_ETH").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let test = std::env::var("TEST").map(|v| v == "1").unwrap_or(false);
    let bf = format!("{root}/dataset/backfill");
    let src = Arc::new(ChainSource::from_env(&root, 1));
    let (token, chat) = (envv(&root, "TELEGRAM_BOT_TOKEN"), envv(&root, "TELEGRAM_CHAT_ID"));

    // сплячі з ролями + їхній баланс і остання відома активність
    let roles: Value = serde_json::from_reader(BufReader::new(File::open(format!("{bf}/wallet-roles.json")).expect("wallet-roles — спершу walletrole"))).unwrap();
    let mut sleepers: Vec<(String, f64, u64)> = Vec::new();
    for x in roles["wallets"].as_array().cloned().unwrap_or_default() {
        if x["role"].as_str() != Some("БЕЗ-ETH-РУХУ") { continue; }
        let b = x["balEth"].as_f64().unwrap_or(0.0);
        if b < min_eth { continue; }
        sleepers.push((x["w"].as_str().unwrap_or("").to_lowercase(), b, x["lastTs"].as_u64().unwrap_or(0)));
    }
    sleepers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    eprintln!("[wakeup] сплячих із балансом ≥{min_eth} ETH: {} (разом {:.0} ETH)",
        sleepers.len(), sleepers.iter().map(|s| s.1).sum::<f64>());

    // мітки адрес і ціна ETH: без них алерт може сказати «ожив», але не «куди пішло»
    let lbl: Arc<serde_json::Map<String, Value>> = Arc::new(
        File::open(format!("{root}/dataset/labels-cache.json")).ok()
            .and_then(|f| serde_json::from_reader(BufReader::new(f)).ok()).unwrap_or_default());
    let eth_usd: f64 = File::open(format!("{bf}/token-prices.json")).ok()
        .and_then(|f| serde_json::from_reader::<_, Value>(BufReader::new(f)).ok())
        .and_then(|v| v["eth"].as_f64()).unwrap_or(0.0);

    let statef = format!("{root}/deploy/wakeup.state.json");
    let state: HashMap<String, u64> = fs::read_to_string(&statef).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    // ПЕРШИЙ ПРОГІН = ЛИШЕ БАЗА. Базою не може бути lastTs із walletrole: там «остання» рахується
    // тільки по переказах нативного ETH ≥0.0005, а перевірка дивиться БУДЬ-ЯКУ транзакцію
    // (токени, виклики контрактів). Через це на першому запуску 180 із 222 виглядали «пробудженими» —
    // це був артефакт різних визначень, а не подія.
    let bootstrap = state.is_empty();
    let known: Arc<HashMap<String, u64>> = Arc::new(state);

    let client = Arc::new(reqwest::Client::builder().timeout(std::time::Duration::from_secs(40)).build().unwrap());
    let sem = Arc::new(Semaphore::new(conc));
    let woke: Arc<Mutex<Vec<Woke>>> = Arc::new(Mutex::new(Vec::new()));
    let latest: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    let prog = Arc::new(AtomicUsize::new(0));
    let total = sleepers.len();

    let mut hs = Vec::new();
    for (w, bal, base_ts) in sleepers {
        let (src, sem, woke, latest, known, prog, lbl) = (src.clone(), sem.clone(), woke.clone(), latest.clone(), known.clone(), prog.clone(), lbl.clone());
        hs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.unwrap();
            // offset=25 і фільтр value>0: ловимо саме РУХ ETH, а не будь-яку транзакцію.
            // Інакше порівнюємо різне з різним: база (walletrole) рахує лише ETH-перекази,
            // а «остання транзакція» включає токени й виклики контрактів — ці гаманці ними
            // активні постійно, тож детектор спрацьовував би щодня без жодної події.
            //
            // `None` тут — не «нема руху», а «не спитали». Без цієї різниці невдалий запит
            // виглядав би як тиша, а наступний вдалий — як пробудження (21 хибне спрацювання
            // за секунди на контрольному прогоні).
            let Some(txs) = src.latest_txs(Kind::Ext, &w, 25).await else { return };
            let last = txs.iter().filter(|t| t.value != "0")
                .map(|t| t.ts).max().unwrap_or(0);
            let _ = base_ts;
            // СТРОГО ІНКРЕМЕНТАЛЬНО: без збереженого стану гаманець лише береться на облік.
            // Відкат на lastTs із walletrole давав хибні спрацювання щоразу, коли запит провалився
            // минулого разу й пройшов цього (8 «пробуджень» за секунди на контрольному прогоні).
            let Some(&prev) = known.get(&w) else {
                latest.lock().unwrap().insert(w, last);
                return;
            };
            // подія = рух ETH ПІСЛЯ справжньої тиші, а не будь-який новіший переказ
            let silence: u64 = std::env::var("MIN_SILENCE").ok().and_then(|s| s.parse().ok()).unwrap_or(90) * 86400;
            if last > prev && prev > 0 && last - prev >= silence {
                // Транзакція, якою гаманець ожив. Без неї алерт каже «ожив» і замовкає —
                // а вся суть події в тому, КУДИ пішли гроші: на біржу це підготовка продажу,
                // на свіжий гаманець — переміщення, і це різні новини.
                let wake = txs.iter().filter(|t| t.value != "0" && t.ts == last)
                    .max_by(|a, b| a.value.parse::<f64>().unwrap_or(0.0)
                        .partial_cmp(&b.value.parse::<f64>().unwrap_or(0.0)).unwrap());
                let (to, amt, hash) = match wake {
                    Some(t) => (t.to.clone(), t.value.parse::<f64>().unwrap_or(0.0) / 1e18, t.hash.clone()),
                    None => (String::new(), 0.0, String::new()),
                };
                let dest = lbl.get(&to).and_then(|x| x["name"].as_str()).unwrap_or("").to_string();
                woke.lock().unwrap().push(Woke { addr: w.clone(), bal, prev, last, to, dest, amt, hash });
            }
            latest.lock().unwrap().insert(w, last.max(prev));
            let n = prog.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 250 == 0 || n == total { eprintln!("[wakeup] {n}/{total}"); }
        }));
    }
    for h in hs { let _ = h.await; }

    let mut w = woke.lock().unwrap().drain(..).collect::<Vec<Woke>>();
    w.sort_by(|a, b| b.bal.partial_cmp(&a.bal).unwrap());
    // нормалізація нуля: сума порожнього списку друкувалась як «-0.0 ETH» —
    // дрібниця, але звіт із мінус-нулем читається як зламаний
    let eth_sum: f64 = w.iter().map(|x| x.bal).sum::<f64>().max(0.0);
    eprintln!("[wakeup] ПРОКИНУЛОСЬ: {} гаманців · {:.1} ETH", w.len(), eth_sum);
    for x in w.iter().take(10) {
        eprintln!("   {:.1} ETH · мовчав {:.0} днів · {} → {} {}",
            x.bal, (x.last - x.prev) as f64 / 86400.0, x.addr, x.to, x.dest);
    }
    if bootstrap { eprintln!("[wakeup] БУТСТРАП: база зафіксована, алерт не шлеться (перший прогін)"); }
    if (!w.is_empty() && !bootstrap) || test {
        let mut m = String::new();
        m.push_str(if test { "🧪 <b>ТЕСТ wakeup</b>" } else { "🌅 <b>ПРОБУДЖЕННЯ СПЛЯЧИХ</b>" });
        m.push_str(&format!("\nГаманців: {} · разом на них {:.0} ETH", w.len(), eth_sum));
        if eth_usd > 0.0 { m.push_str(&format!(" ({})", human_usd(eth_sum * eth_usd))); }
        m.push_str("\n");

        let to_cex: Vec<&Woke> = w.iter().filter(|x| is_exchange(&x.dest)).collect();

        m.push_str("\n<b>ХТО ОЖИВ І ЧИМ САМЕ</b>\n");
        for x in w.iter().take(8) {
            let days = (x.last - x.prev) as f64 / 86400.0;
            let dest = if !x.dest.is_empty() { x.dest.clone() }
                       else if x.to.is_empty() { "отримувача не видно".to_string() }
                       else { "немічена адреса".to_string() };
            m.push_str(&format!("\n• {} — тримає {:.0} ETH", addr_link(&x.addr), x.bal));
            if eth_usd > 0.0 { m.push_str(&format!(" ({})", human_usd(x.bal * eth_usd))); }
            m.push_str(&format!("\n  мовчав {days:.0} днів\n"));
            if !x.to.is_empty() {
                m.push_str(&format!("  ожив переказом {:.2} ETH", x.amt));
                if eth_usd > 0.0 { m.push_str(&format!(" ({})", human_usd(x.amt * eth_usd))); }
                m.push_str(&format!(" → {} <b>{}</b>\n", addr_link(&x.to), dest));
                if !x.hash.is_empty() {
                    m.push_str(&format!("  <a href=\"https://etherscan.io/tx/{}\">дивитись транзакцію</a>\n", x.hash));
                }
            }
        }
        if w.len() > 8 {
            let rest: f64 = w.iter().skip(8).map(|x| x.bal).sum();
            m.push_str(&format!("\n…і ще {} гаманців на {:.0} ETH\n", w.len() - 8, rest));
        }

        m.push_str("\n<b>ЩО ЦЕ ЗНАЧИТЬ</b>\n");
        m.push_str("• Сплячий гаманець ожив — це не робочий цикл, а рішення: після місяців тиші хтось узяв ключі\n");
        if to_cex.is_empty() {
            m.push_str("• Жоден переказ не пішов на біржу: гроші перемістились усередині мережі, у ринок вони поки не вийшли\n");
        } else {
            let cex_eth: f64 = to_cex.iter().map(|x| x.amt).sum();
            m.push_str(&format!("• НА БІРЖІ пішло {} із {} гаманців ({:.2} ETH", to_cex.len(), w.len(), cex_eth));
            if eth_usd > 0.0 { m.push_str(&format!(" = {}", human_usd(cex_eth * eth_usd))); }
            m.push_str(") — саме цей напрям означає підготовку продажу\n");
            for x in to_cex.iter().take(3) {
                m.push_str(&format!("  ↳ {} → <b>{}</b>\n", short(&x.addr), x.dest));
            }
            m.push_str("• За нашим виміром (cexemis) медіана ціни після заводів на біржу −7.6% до ринку\n");
        }
        m.push_str("• ⚠️ Пробудження показує НАМІР, а не результат: гаманець міг ожити й знову завмерти. \
            Вага події — у балансі, що стоїть за ним, і в напрямі переказу\n");
        let msg = m;
        if tg(&client, &token, &chat, &msg).await { eprintln!("[wakeup] Telegram надіслано"); }
    }
    let l = latest.lock().unwrap().clone();
    let _ = fs::write(&statef, serde_json::to_string(&json!(l)).unwrap());
    eprintln!("[wakeup] стан збережено ({} адрес)", l.len());
}
