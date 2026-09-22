//! campwatch — монітор концентрованих позицій: чи почали виводити, розмивати чи додавати ліквідність.
//!
//! Привід: адреса `0x02339afc…` тримає **76.8% емісії CAMP** (3.78 млрд токенів, $1.44 млн),
//! зібрані з 340 адрес за рік, і за весь час вивела рівно **1 CAMP**. Версія «готують памп»
//! не підтверджується ліквідністю: у всіх пулах разом **$7.4 тис**, обіг за добу **$93**,
//! а ціна вже впала на 99.7% від початку збору. Вийти з такої позиції в ринок неможливо.
//!
//! Але дві речі перевіряються лише спостереженням, і саме їх ми й стежимо:
//!   1. ПОЧАЛИ ВИВОДИТИ — баланс холдера впав більше ніж на `DROP_PCT`;
//!   2. РОЗМИВАЮТЬ СЛІДИ — з'явились нові адреси-отримувачі (розподіл по гаманцях);
//!   3. ГОТУЮТЬ ВИХІД — ліквідність пулів зросла в рази (без неї памп безглуздий).
//!
//! Стан між запусками — у `campwatch-state.json`; алерт іде в Telegram (як `cexsignal`).
//! Args: ROOT. Env: TOKENS(контракт:символ,…) HOLDERS(адреси через кому)
//!       DROP_PCT(1.0) LIQ_MULT(2.0) DRY(0)
use backfill::chain::{ChainSource, Kind};
use std::collections::HashMap;
use serde_json::{json, Value};
use std::fs::File;
use std::io::BufReader;

fn envs(k: &str, d: &str) -> String { std::env::var(k).unwrap_or_else(|_| d.to_string()) }
fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn envv(root: &str, k: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string()).unwrap_or_default()
}

async fn tg(c: &reqwest::Client, token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() {
        eprintln!("[campwatch] нема TELEGRAM-креденшлів — алерт лише в лог");
        return false;
    }
    matches!(c.post(format!("https://api.telegram.org/bot{token}/sendMessage"))
        .form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"),
                ("disable_web_page_preview", "true")]).send().await,
        Ok(r) if r.status().is_success())
}

// Баланс токена бере backfill::chain (token_balance): `None` = не дізнались.
// Порівнювати при `None` НЕ можна — збій API виглядав би як «вивели все».

/// Сумарна ліквідність усіх пулів токена (GeckoTerminal). Зростання в рази — ознака
/// підготовки виходу: без ліквідності продати концентровану позицію нікому.
async fn liquidity(c: &reqwest::Client, tok: &str) -> Option<(f64, f64, usize, f64)> {
    let url = format!("https://api.geckoterminal.com/api/v2/networks/eth/tokens/{tok}/pools?page=1");
    let r = c.get(&url).send().await.ok()?;
    let v: Value = r.json().await.ok()?;
    let pools = v["data"].as_array()?;
    let mut liq = 0.0;
    let mut vol = 0.0;
    // Ціну беремо з НАЙГЛИБШОГО пулу, а не з першого-ліпшого: у дрібному пулі одна
    // угода на $50 ставить будь-яку ціну, і саме такі пули дають «COMP по $41046».
    let mut price = 0.0;
    let mut deepest = 0.0;
    for p in pools {
        let a = &p["attributes"];
        let r = a["reserve_in_usd"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        liq += r;
        vol += a["volume_usd"]["h24"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        if r > deepest {
            if let Some(px) = a["base_token_price_usd"].as_str().and_then(|s| s.parse::<f64>().ok()) {
                if px > 0.0 { price = px; deepest = r; }
            }
        }
    }
    Some((liq, vol, pools.len(), price))
}

/// Куди саме пішли токени: адреса-отримувач, її мітка й сума.
struct Moved {
    to: String,
    name: String,
    amount: f64,
}

fn human_amt(v: f64) -> String {
    if v >= 1e9 { format!("{:.2} млрд", v / 1e9) }
    else if v >= 1e6 { format!("{:.2} млн", v / 1e6) }
    else if v >= 1e3 { format!("{:.1} тис", v / 1e3) }
    else { format!("{v:.2}") }
}

fn human_usd(v: f64) -> String {
    if v >= 1e9 { format!("${:.2}млрд", v / 1e9) }
    else if v >= 1e6 { format!("${:.2}млн", v / 1e6) }
    else if v >= 1e3 { format!("${:.1}тис", v / 1e3) }
    else { format!("${v:.0}") }
}

fn fmt_price(p: f64) -> String {
    if p <= 0.0 { "?".to_string() }
    else if p >= 1.0 { format!("{p:.4}") }
    else if p >= 0.01 { format!("{p:.6}") }
    else { format!("{p:.10}") }
}

fn short(a: &str) -> String {
    if a.len() >= 42 { format!("{}…{}", &a[0..8], &a[a.len() - 4..]) } else { a.to_string() }
}

fn addr_link(a: &str) -> String {
    format!("<a href=\"https://etherscan.io/address/{a}\">{}</a>", short(a))
}

/// Оцінка просідання ціни при продажі `usd` у пул із TVL `reserve`.
/// Константний добуток (x·y=k): котирувальна сторона ≈ TVL/2, ціна після продажу
/// падає до (R/(R+X))² від початкової. Оцінка ЗВЕРХУ по одному свопу — реальний
/// продаж дроблять, але порядок величини правильний.
fn pool_impact_pct(usd: f64, reserve: f64) -> Option<f64> {
    if reserve <= 0.0 || usd <= 0.0 { return None; }
    let r = reserve / 2.0;
    let ratio = r / (r + usd);
    Some(100.0 * (1.0 - ratio * ratio))
}

/// Вихідні перекази токена з адреси після моменту `since`.
///
/// НАВІЩО ОКРЕМИЙ ЗАПИТ: монітор бачив лише БАЛАНСИ, тож міг сказати «вивів 89%», але не
/// міг сказати куди. А саме це й вирішує, чи подія значуща: вихід на біржу — підготовка
/// продажу, вихід на свіжий гаманець — переміщення, вихід у пул — продаж уже стався.
async fn outflows(src: &ChainSource, holder: &str, tok: &str, since: u64,
                  lbl: &serde_json::Map<String, Value>) -> Vec<Moved> {
    let Some(txs) = src.latest_txs(Kind::Tok, holder, 500).await else { return Vec::new() };
    let mut by: HashMap<String, f64> = HashMap::new();
    for t in &txs {
        if t.token != tok || t.from != holder { continue; }
        if since > 0 && t.ts < since { continue; }
        let dec: i32 = t.dec.parse().unwrap_or(18);
        let amt = t.value.parse::<f64>().unwrap_or(0.0) / 10f64.powi(dec);
        if amt <= 0.0 { continue; }
        *by.entry(t.to.clone()).or_insert(0.0) += amt;
    }
    let mut v: Vec<Moved> = by.into_iter().map(|(to, amount)| {
        let name = lbl.get(&to).and_then(|x| x["name"].as_str()).unwrap_or("").to_string();
        Moved { to, name, amount }
    }).collect();
    v.sort_by(|a, b| b.amount.partial_cmp(&a.amount).unwrap());
    v
}


/// Токени з тегу: (контракт, символ).
fn tagged_tokens(tag: &str) -> Vec<(String, String)> {
    let Ok(c) = wr_store::open() else { return Vec::new() };
    let Ok(mut q) = c.prepare(
        "SELECT t.contract, t.symbol FROM token_tags g
         JOIN tokens t ON t.chain=g.chain AND t.contract=g.contract WHERE g.tag=?1",
    ) else {
        return Vec::new();
    };
    q.query_map([tag], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|it| it.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
}

/// Гаманці з тегу.
fn tagged_wallets(tag: &str) -> Vec<String> {
    let Ok(c) = wr_store::open() else { return Vec::new() };
    let Ok(mut q) = c.prepare("SELECT address FROM wallet_tags WHERE tag=?1") else {
        return Vec::new();
    };
    q.query_map([tag], |r| r.get(0))
        .map(|it| it.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let drop_pct: f64 = envn("DROP_PCT", 1.0);
    let liq_mult: f64 = envn("LIQ_MULT", 2.0);
    let dry = envs("DRY", "0") == "1";

    // Предмети спостереження — у базі (теги `campwatch:tokens` / `campwatch:holders`),
    // а не в коді: це адреси активного розслідування. Env досі перекриває вибір.
    let tokens: Vec<(String, String)> = match std::env::var("TOKENS") {
        Ok(v) if !v.is_empty() => v
            .split(',')
            .filter_map(|s| {
                let mut it = s.split(':');
                Some((it.next()?.trim().to_lowercase(), it.next().unwrap_or("?").trim().to_string()))
            })
            .collect(),
        _ => tagged_tokens("campwatch:tokens"),
    };
    let holders: Vec<String> = match std::env::var("HOLDERS") {
        Ok(v) if !v.is_empty() => {
            v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| s.len() == 42).collect()
        }
        _ => tagged_wallets("campwatch:holders"),
    };
    if tokens.is_empty() || holders.is_empty() {
        eprintln!("[campwatch] порожній вотчліст — спостерігати нема за чим (перевір базу)");
        return;
    }

    // Ключі й ліміти — турбота шару; тут про них не знають.
    let src = ChainSource::from_env(&root, 1);
    let tg_token = envv(&root, "TELEGRAM_BOT_TOKEN");
    let tg_chat = envv(&root, "TELEGRAM_CHAT_ID");

    // мітки адрес: без них отримувач лишається шістнадцятковим рядком
    let lbl: serde_json::Map<String, Value> = File::open(format!("{root}/dataset/labels-cache.json")).ok()
        .and_then(|f| serde_json::from_reader(BufReader::new(f)).ok()).unwrap_or_default();

    let statef = format!("{bf}/campwatch-state.json");
    let prev: Value = File::open(&statef).ok()
        .and_then(|f| serde_json::from_reader(BufReader::new(f)).ok())
        .unwrap_or_else(|| json!({}));
    // від якого моменту шукати вихідні перекази: стан несе час свого запису
    let since = prev["asOf"].as_u64().unwrap_or(0);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(40)).build().unwrap();
    let mut state = serde_json::Map::new();
    let mut alerts: Vec<String> = Vec::new();

    for (tok, sym) in tokens.iter() {
        let mut total = 0.0;
        let mut per: serde_json::Map<String, Value> = serde_json::Map::new();
        let mut unknown = 0;
        // хто просів: (холдер, % падіння, було, стало). Розбір «куди саме» — нижче,
        // коли вже відома ціна: без неї сума в токенах ні про що не говорить.
        let mut moved_out: Vec<(String, f64, f64, f64)> = Vec::new();
        for h in holders.iter() {
            match src.token_balance(tok, h).await.map(|raw| raw as f64 / 1e18) {
                Some(b) => {
                    total += b;
                    per.insert(h.clone(), json!(b));
                    let key = format!("{tok}|{h}");
                    if let Some(old) = prev["holders"][&key].as_f64() {
                        if old > 0.0 {
                            let d = 100.0 * (old - b) / old;
                            if d >= drop_pct {
                                moved_out.push((h.clone(), d, old, b));
                            }
                        }
                    }
                }
                None => { unknown += 1 }
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        // ліквідність, обіг і ціна
        let (liq, vol, pools, price) = liquidity(&client, tok).await.unwrap_or((0.0, 0.0, 0, 0.0));

        // ── розгорнутий алерт про виведення позиції ──
        for (h, d, old, now_bal) in &moved_out {
            let gone = old - now_bal;
            let gone_usd = gone * price;
            let mut a = String::new();
            a.push_str(&format!("⚠️ <b>{sym}: виведено {d:.1}% позиції</b>\n"));
            a.push_str(&format!("Холдер {}\n", addr_link(h)));
            a.push_str(&format!("Було {} → стало {} (пішло {} = {})\n",
                human_amt(*old), human_amt(*now_bal), human_amt(gone), human_usd(gone_usd)));
            a.push_str(&format!("Ціна {sym}: ${}\n", fmt_price(price)));

            // КУДИ САМЕ
            let dests = outflows(&src, h, tok, since, &lbl).await;
            if dests.is_empty() {
                a.push_str("\n<b>КУДИ:</b> вихідних переказів за період не знайдено — \
                    можливо, баланс змінився спаленням або спливом, а не переказом\n");
            } else {
                a.push_str("\n<b>КУДИ ПІШЛО</b>\n");
                for m in dests.iter().take(5) {
                    let who = if !m.name.is_empty() { m.name.clone() } else { "немічена адреса".to_string() };
                    a.push_str(&format!("• {} <b>{}</b> — {} {} ({})\n",
                        addr_link(&m.to), who, human_amt(m.amount), sym, human_usd(m.amount * price)));
                }
                if dests.len() > 5 {
                    a.push_str(&format!("• …і ще {} адрес\n", dests.len() - 5));
                }
            }

            // НАСЛІДКИ
            a.push_str("\n<b>ЩО ЦЕ ЗНАЧИТЬ</b>\n");
            a.push_str(&format!("• Ліквідність усіх пулів: {} · обіг за добу: {}\n",
                human_usd(liq), human_usd(vol)));
            let impact = if gone_usd > 0.0 { pool_impact_pct(gone_usd, liq) } else { None };
            if gone_usd > 0.0 && liq > 0.0 {
                a.push_str(&format!("\u{2022} Виведене — {:.0}% від ліквідності пулів\n", 100.0 * gone_usd / liq));
            }
            let days = if vol > 0.0 { gone_usd / vol } else { 0.0 };
            // ВЕРДИКТ ОДИН, і він від просідання, а не від частки обігу. Ці дві міри легко
            // суперечать одна одній: обсяг може бути чвертю денного обігу (наче дрібниця)
            // і водночас чвертю котирувальної сторони пулу — тобто обвалом на третину.
            // Вирішує глибина пулу: саме в неї впирається продаж.
            match impact {
                Some(imp) if imp >= 10.0 => {
                    a.push_str(&format!("\u{2022} Продати це в пул НЕ ВИЙДЕ без обвалу: просідання ~{imp:.0}% (оцінка x\u{b7}y=k)\n"));
                    if days >= 1.0 {
                        a.push_str(&format!("\u{2022} Навіть дробленням це ~{days:.0} днів УСЬОГО обігу ринку, і то якщо обіг не впаде\n"));
                    }
                }
                Some(imp) => {
                    a.push_str(&format!("\u{2022} Продаж у пул коштував би ~{imp:.0}% просідання — такий обсяг ринок тримає\n"));
                    if vol > 0.0 {
                        a.push_str(&format!("\u{2022} Це ~{:.0}% денного обігу\n", days * 100.0));
                    }
                }
                None => a.push_str("\u{2022} Ліквідності пулів не видно — оцінити наслідки продажу нема на чому\n"),
            }
            let to_cex = dests.iter().any(|m| {
                let n = m.name.to_lowercase();
                ["binance","coinbase","okx","kraken","bybit","kucoin","gate","mexc","bitget","huobi","htx","bithumb","upbit"]
                    .iter().any(|c| n.contains(c))
            });
            if to_cex {
                a.push_str("• Отримувач — БІРЖА: токени стали доступні до продажу. За нашим виміром (cexemis) \
                    медіана ціни після таких заводів −7.6% до ринку\n");
            } else if !dests.is_empty() {
                a.push_str("• Отримувачі — не біржі: це переміщення позиції, а не вихід у гроші. \
                    Позиція нікуди не поділась, змінився лише гаманець\n");
            }
            alerts.push(a);
        }

        if let Some(old) = prev["liq"][tok].as_f64() {
            if old > 100.0 && liq > old * liq_mult {
                let mut a = format!("🔴 <b>{sym}: ЛІКВІДНІСТЬ зросла ×{:.1}</b>\n{} → {}\n",
                    liq / old, human_usd(old), human_usd(liq));
                a.push_str(&format!("Ціна {sym}: ${} · обіг за добу {}\n", fmt_price(price), human_usd(vol)));
                a.push_str(&format!("Холдери тримають {} {sym} = {}\n", human_amt(total), human_usd(total * price)));
                a.push_str("\n<b>ЩО ЦЕ ЗНАЧИТЬ</b>\n• Ліквідність — єдине, чого бракувало для виходу: \
                    продати концентровану позицію нікому, поки пули порожні. Різке наливання пулів \
                    передує можливості вийти\n");
                if let Some(imp) = pool_impact_pct(total * price, liq) {
                    a.push_str(&format!("• Навіть тепер продаж усієї позиції в пул дав би просідання ~{imp:.0}%\n"));
                }
                alerts.push(a);
            }
        }
        eprintln!("[campwatch] {sym}: холдери тримають {:.0} · ліквідність ${:.0} · обіг24г ${:.0} · пулів {} · невідомо {}",
            total, liq, vol, pools, unknown);
        state.insert(format!("liq|{tok}"), json!(liq));
        state.insert(format!("vol|{tok}"), json!(vol));
        for (k, v) in per { state.insert(format!("{tok}|{k}"), v); }
    }

    // ПЕРЕЗАПИС СТАНУ лише якщо всі баланси відомі — інакше наступний запуск порівняє
    // з діркою і вирішить, що позицію вивели
    let out = json!({
        "asOf": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "holders": state.iter().filter(|(k, _)| k.contains('|') && !k.starts_with("liq|") && !k.starts_with("vol|"))
            .map(|(k, v)| (k.clone(), v.clone())).collect::<serde_json::Map<String, Value>>(),
        "liq": tokens.iter().map(|(t, _)| (t.clone(),
            state.get(&format!("liq|{t}")).cloned().unwrap_or(json!(0)))).collect::<serde_json::Map<String, Value>>(),
        "vol": tokens.iter().map(|(t, _)| (t.clone(),
            state.get(&format!("vol|{t}")).cloned().unwrap_or(json!(0)))).collect::<serde_json::Map<String, Value>>(),
    });
    if !dry { std::fs::write(&statef, serde_json::to_string_pretty(&out).unwrap()).ok(); }

    if alerts.is_empty() {
        eprintln!("[campwatch] змін не виявлено");
    } else {
        let msg = format!("<b>МОНІТОР КОНЦЕНТРОВАНИХ ПОЗИЦІЙ</b>\n\n{}", alerts.join("\n\n"));
        eprintln!("[campwatch] АЛЕРТ:\n{}", alerts.join("\n\n"));
        if !dry { tg(&client, &tg_token, &tg_chat, &msg).await; }
    }
}
