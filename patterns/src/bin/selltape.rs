//! selltape — ЩО РЕАЛЬНО РОБИТЬСЯ В АКТИВАХ: купівлі й ПРОДАЖІ на біржах, з алертом у Telegram.
//!
//! ЧОМУ ОКРЕМИЙ БІН, А НЕ ЩЕ ОДНА КОЛОНКА В cexsignal. `cexsignal` бачить ончейн-ногу:
//! гаманець групи переказав токен на адресу з біржовою міткою. Це ЗАВЕЗЕННЯ запасу на біржу,
//! і воно ще не продаж — дослід cexemis показав, що медіана після депозиту −7.6%, тобто
//! завезення лише передує зливу, а сам злив відбувається в стакані й ончейн його не видно.
//! Тут друга нога: агресивний потік у самому стакані — скільки куплено ринковими ордерами,
//! скільки продано, і чи це сплеск проти власної норми активу.
//!
//! ДЖЕРЕЛА (усі публічні, без ключів):
//!   Binance klines 1h — єдине джерело з ЧЕСНИМ розрізом taker buy/sell за кожну годину історії
//!     (`takerBuyQuoteVolume` проти `quoteVolume`), тож для лістингів на Binance беремо його.
//!   OKX / Bybit / Gate `recent trades` — для активів, яких на Binance немає. Там віддають
//!     лише останні ~1000 угод, тобто одну-дві години; довга база набігає у власному CSV.
//!
//! ЧОГО ТУТ НЕ БУДЕ. Taker buy/sell — це агресор, а не «хто заробив». Кит може продавати
//! лімітками, і тоді його продаж порахується як чужа купівля. Тому сигнал читається як
//! «в активі йде роздача в стакан», а не «група продала рівно стільки».
//!
//! ВИХІД (диск D:, бо код на ноуті, а дані — на зовнішньому): $WR_OUT/tape.csv (append, дедуп
//! за ts+symbol+exchange), стан алертів $WR_OUT/selltape.state.json.
//!
//! КРОК ЧАСУ. Дефолт — 5 хвилин: ноут опитує біржі кожні 5 хв, і сенс у цьому є лише
//! тоді, коли відро теж 5-хвилинне (годинна свічка не міняється між запусками, тож
//! частіші проходи давали б ту саму цифру). Для кожного TF — свій файл і своя база,
//! бо змішати 5m і 1h в одному CSV означає порахувати сплеск проти чужої норми.
//!
//! ЛІНІЙКА — ЕМІСІЯ. Долари самі по собі брешуть про масштаб: $1 млн у SHIB це пил,
//! $1 млн у дрібному альті може бути кількома відсотками всього, що існує. Тому і поріг
//! алерту, і порядок у списках, і денний звіт рахуються від частки емісії активу
//! (`dataset/token-supply.json`), а частка групи показує, скільки це від ЇЇ позиції.
//!
//! Запуск:  selltape [SYMBOL ...]        — прохід стрічки + алерти
//!          selltape --daily             — денний звіт за останні 24 год у Telegram
//! Env: WR_OUT WR_DATASET WR_ROOT TF(5m) MULT(2.5) MIN_USD(25000) MIN_EMIS_PCT(0.05)
//!      SELL_PCT(55) COOLDOWN_MIN(0) DIGEST_H(6) TOP(12) LOOP_MIN(0) DRY(0) MAX_SYMBOLS(120)
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
/// Спільна основа стрічки (довідники бірж, свічки з розрізом агресора, емісія,
/// вотчліст) живе окремо: тим самим кодом користується ретро-реплей `sigretro`.
/// Якби в реплея була власна копія, його висновок не говорив би нічого про цей бін.
#[path = "../tape_core.rs"] mod tape_core;
use tape_core::*;





// ───────────────────────────── збір по одному активу ─────────────────────────────


/// Останні угоди → відра по годинах. Джерело коротке (одна-дві години), але це
/// єдиний спосіб побачити сторону там, де свічки її не віддають.
fn bucket_trades(sym: &str, ex: &str, pair: &str, bs: i64, trades: Vec<(i64, f64, f64, bool)>) -> Vec<Row> {
    let mut by_h: HashMap<i64, (f64, f64, f64, i64)> = HashMap::new(); // ts -> (buy, sell, last_px, n)
    for (ts_ms, px, qty, is_buy) in trades {
        let h = (ts_ms / 1000) / bs * bs;
        let usd = px * qty;
        let e = by_h.entry(h).or_insert((0.0, 0.0, px, 0));
        if is_buy { e.0 += usd } else { e.1 += usd }
        e.2 = px;
        e.3 += 1;
    }
    by_h.into_iter().map(|(ts, (b, s, px, n))| Row {
        ts, symbol: sym.to_string(), exchange: ex.to_string(), pair: pair.to_string(),
        close: px, vol_usd: b + s, buy_usd: b, sell_usd: s, n, src: "trades".into(),
    }).collect()
}

fn from_okx(sym: &str, pair: &str, bs: i64) -> Vec<Row> {
    let Some(v) = get_json(&format!("https://www.okx.com/api/v5/market/trades?instId={pair}&limit=500")) else { return vec![] };
    let Some(arr) = v.get("data").and_then(|x| x.as_array()) else { return vec![] };
    let t = arr.iter().filter_map(|x| {
        let ts: i64 = x.get("ts")?.as_str()?.parse().ok()?;
        Some((ts, f(x.get("px")?), f(x.get("sz")?), x.get("side")?.as_str()? == "buy"))
    }).collect();
    bucket_trades(sym, "okx", pair, bs, t)
}

fn from_bybit(sym: &str, pair: &str, bs: i64) -> Vec<Row> {
    let Some(v) = get_json(&format!("https://api.bybit.com/v5/market/recent-trade?category=spot&symbol={pair}&limit=1000")) else { return vec![] };
    let Some(arr) = v.pointer("/result/list").and_then(|x| x.as_array()) else { return vec![] };
    let t = arr.iter().filter_map(|x| {
        let ts: i64 = x.get("time")?.as_str()?.parse().ok()?;
        Some((ts, f(x.get("price")?), f(x.get("size")?), x.get("side")?.as_str()? == "Buy"))
    }).collect();
    bucket_trades(sym, "bybit", pair, bs, t)
}

fn from_gate(sym: &str, pair: &str, bs: i64) -> Vec<Row> {
    let Some(v) = get_json(&format!("https://api.gateio.ws/api/v4/spot/trades?currency_pair={pair}&limit=1000")) else { return vec![] };
    let Some(arr) = v.as_array() else { return vec![] };
    let t = arr.iter().filter_map(|x| {
        let ts: i64 = x.get("create_time_ms")?.as_str()?.parse::<f64>().ok()? as i64;
        Some((ts, f(x.get("price")?), f(x.get("amount")?), x.get("side")?.as_str()? == "buy"))
    }).collect();
    bucket_trades(sym, "gate", pair, bs, t)
}




// ───────────────────────────── стан і CSV ─────────────────────────────

fn load_csv(path: &str) -> Vec<Row> {
    let mut out = Vec::new();
    let Ok(fh) = std::fs::File::open(path) else { return out };
    for line in std::io::BufReader::new(fh).lines().map_while(Result::ok).skip(1) {
        let c: Vec<&str> = line.split(',').collect();
        if c.len() < 11 { continue }
        let Ok(ts) = c[0].parse::<i64>() else { continue };
        out.push(Row {
            ts, symbol: c[1].to_string(), exchange: c[2].to_string(), pair: c[3].to_string(),
            close: c[4].parse().unwrap_or(0.0), vol_usd: c[5].parse().unwrap_or(0.0),
            buy_usd: c[6].parse().unwrap_or(0.0), sell_usd: c[7].parse().unwrap_or(0.0),
            n: c[8].parse().unwrap_or(0), src: c[10].to_string(),
        });
    }
    out
}

fn tg(token: &str, chat: &str, text: &str) -> bool {
    if token.is_empty() || chat.is_empty() { eprintln!("[selltape] нема TELEGRAM креденшлів — алерт нікуди слати"); return false }
    match ureq::post(&format!("https://api.telegram.org/bot{token}/sendMessage"))
        .set("User-Agent", UA).timeout(std::time::Duration::from_secs(25))
        .send_form(&[("chat_id", chat), ("text", text), ("parse_mode", "HTML"), ("disable_web_page_preview", "true")]) {
        Ok(_) => true,
        Err(e) => { eprintln!("[telegram] не відправлено: {e}"); false }
    }
}

fn envv(root: &str, k: &str) -> String {
    std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default().lines()
        .find(|l| l.starts_with(&format!("{k}=")))
        .map(|l| l.splitn(2, '=').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

struct Sig {
    sym: String, w: Watch, cur: Row, base: f64, spike: f64, sell_share: f64,
    dh: f64, d24: f64, bs: i64,
    /// уся емісія активу в грошах за поточною ціною; 0 = емісія невідома
    emis_usd: f64,
    /// скільки відсотків УСІЄЇ емісії пройшло через стакан у цьому вікні (за стороною)
    emis_pct: f64,
    /// непорожнє = з випуском щось не так, і читач має це бачити
    emis_note: &'static str,
}

/// Назва групи в тексті: «NET» → «група NET», а вже названу («мажор мережі»,
/// «🎯 SPELL — контроль 21%») лишаємо як є, інакше виходить «група група капіталу».
fn group_label(g: &str) -> String {
    if g.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-') {
        format!("група {g}")
    } else { g.to_string() }
}

fn alert_text(s: &Sig, side: &str) -> String {
    let icon = if side == "sell" { "🔴 РОЗДАЧА В СТАКАН" } else { "🟢 ВИКУП" };
    let net = s.cur.buy_usd - s.cur.sell_usd;
    let mut t = format!(
        "{icon} — <b>{}</b>  ({} {})\nвікно {}–{} UTC\nпродано {} · куплено {} · нетто {}\n\
         частка продажів {:.0}% · сплеск ×{:.1} до норми ({} за вікно)\n\
         ціна {} ({:+.1}% за вікно, {:+.1}% за добу)\nобсяг вікна {} у {} угодах",
        s.sym, s.cur.exchange, s.cur.pair,
        hhmm(s.cur.ts), hhmm(s.cur.ts + s.bs),
        usd(s.cur.sell_usd), usd(s.cur.buy_usd), usd(net),
        s.sell_share, s.spike, usd(s.base),
        price_fmt(s.cur.close), s.dh, s.d24,
        usd(s.cur.vol_usd), s.cur.n);

    // ЕМІСІЯ — головна лінійка. $1 млн у SHIB і $1 млн у дрібному альті — різні події:
    // у першому випадку це проценти від тисячної частки емісії, у другому може бути
    // половина всього, що взагалі існує.
    if s.emis_usd > 0.0 {
        let word = if side == "sell" { "продано" } else { "куплено" };
        t.push_str(&format!("\n<b>{word} {:.3}% усієї емісії</b> (емісія {})", s.emis_pct, usd(s.emis_usd)));
        if s.w.share > 0.0 {
            // скільки це від того шматка, який тримає група
            let of_group = if s.w.share > 0.0 { 100.0 * s.emis_pct / s.w.share } else { 0.0 };
            t.push_str(&format!("\nпід наглядом: {} тримає {:.1}% емісії — вікно = {:.2}% її позиції",
                group_label(&s.w.group), s.w.share, of_group));
        }
    } else if s.w.share > 0.0 {
        t.push_str(&format!("\nпід наглядом: {} тримає {:.1}% емісії <i>(емісія в монетах невідома)</i>",
            group_label(&s.w.group), s.w.share));
    }
    if !s.emis_note.is_empty() {
        t.push_str(&format!("\n<i>⚠ {}</i>", s.emis_note));
    }
    if s.cur.src == "trades" {
        t.push_str("\n<i>джерело — стрічка угод біржі (коротка база)</i>");
    }
    t
}

fn plain(t: &str) -> String {
    t.replace("<b>", "").replace("</b>", "").replace("<i>", "").replace("</i>", "")
}

// ───────────────────────────── один прохід ─────────────────────────────

fn pass(out_dir: &str, ds: &str, root: &str, only: &[String]) {
    let tf = env_s("TF", "5m");
    let bs = tf_secs(&tf);                 // секунд у відрі
    let mult = env_f("MULT", 2.5);
    let min_usd = env_f("MIN_USD", 25_000.0);
    let sell_pct = env_f("SELL_PCT", 55.0);
    // 0 = слати кожен сплеск у кожному новому вікні (саме цього ми хочемо);
    // ненульове значення проріджує алерти по одному активу
    let cooldown = env_u("COOLDOWN_MIN", 0) as i64 * 60;
    let min_emis_pct = env_f("MIN_EMIS_PCT", 0.05);
    let digest_h = env_u("DIGEST_H", 6) as i64 * 3600;
    let top = env_u("TOP", 12) as usize;
    let dry = env_u("DRY", 0) == 1;
    let max_syms = env_u("MAX_SYMBOLS", 120) as usize;

    // окремий файл на кожен крок часу: база 5m і база 1h — різні величини
    let csv_path = format!("{out_dir}/tape-{tf}.csv");
    let state_path = format!("{out_dir}/selltape.state.json");
    let _ = std::fs::create_dir_all(out_dir);

    let ven = load_venues(out_dir);
    let wl = watchlist(ds);
    let supply = load_supply(ds);

    // спершу ті, де частка емісії більша: там злив значить найбільше
    let mut syms: Vec<(String, Watch)> = if only.is_empty() {
        wl.into_iter().collect()
    } else {
        only.iter().map(|s| { let u = s.to_uppercase(); let w = wl.get(&u).cloned().unwrap_or_default(); (u, w) }).collect()
    };
    syms.sort_by(|a, b| b.1.share.partial_cmp(&a.1.share).unwrap_or(std::cmp::Ordering::Equal));
    syms.truncate(max_syms);

    let hist = load_csv(&csv_path);
    let mut seen: HashSet<(i64, String, String)> = hist.iter().map(|r| (r.ts, r.symbol.clone(), r.exchange.clone())).collect();

    let mut fresh: Vec<Row> = Vec::new();
    let mut unlisted = 0usize;
    for (sym, _) in &syms {
        // 288 відер = доба на 5m; на 1h це 12 діб — в обох випадках база з запасом
        let rows = if let Some(p) = ven.binance.get(sym) { from_binance(sym, p, &tf, 288) }
            else if let Some(p) = ven.okx.get(sym) { from_okx(sym, p, bs) }
            else if let Some(p) = ven.bybit.get(sym) { from_bybit(sym, p, bs) }
            else if let Some(p) = ven.gate.get(sym) { from_gate(sym, p, bs) }
            else { unlisted += 1; continue };
        for r in rows {
            if r.vol_usd <= 0.0 { continue }
            if seen.insert((r.ts, r.symbol.clone(), r.exchange.clone())) { fresh.push(r) }
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
    }

    // дописуємо: файл росте тільки новими годинами
    if !fresh.is_empty() {
        let is_new = !std::path::Path::new(&csv_path).exists();
        if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&csv_path) {
            if is_new { let _ = writeln!(fh, "ts,symbol,exchange,pair,close,vol_usd,buy_usd,sell_usd,n_trades,buy_pct,src"); }
            for r in &fresh { let _ = writeln!(fh, "{}", r.csv()); }
        }
    }
    eprintln!("[selltape] активів {} (не лістингованих {unlisted}), нових відер {}", syms.len(), fresh.len());

    // ── рахуємо сигнал на повній історії (стара + свіжа) ──
    let mut all = hist;
    all.extend(fresh.iter().cloned());
    let mut by_sym: HashMap<String, Vec<Row>> = HashMap::new();
    for r in all { by_sym.entry(r.symbol.clone()).or_default().push(r) }

    let now = now_s();
    let last_closed = (now / bs) * bs - bs; // останнє ЗАКРИТЕ відро

    let mut sells: Vec<Sig> = Vec::new();
    let mut buys: Vec<Sig> = Vec::new();
    let mut board: Vec<Sig> = Vec::new();

    for (sym, mut rows) in by_sym {
        rows.sort_by_key(|r| r.ts);
        let Some(cur) = rows.iter().rev().find(|r| r.ts == last_closed).cloned() else { continue };
        if cur.vol_usd <= 0.0 { continue }
        let w = syms.iter().find(|(s, _)| s == &sym).map(|(_, w)| w.clone()).unwrap_or_default();

        let mut prev_sell: Vec<f64> = rows.iter().filter(|r| r.ts < cur.ts && r.ts >= cur.ts - 144 * bs).map(|r| r.sell_usd).collect();
        let mut prev_buy: Vec<f64> = rows.iter().filter(|r| r.ts < cur.ts && r.ts >= cur.ts - 144 * bs).map(|r| r.buy_usd).collect();
        let samples = prev_sell.len();
        let base_s = median(&mut prev_sell);
        let base_b = median(&mut prev_buy);

        let prev_close = rows.iter().rev().find(|r| r.ts < cur.ts).map(|r| r.close).unwrap_or(cur.close);
        let close_24 = rows.iter().find(|r| r.ts >= cur.ts - 86_400).map(|r| r.close).unwrap_or(cur.close);
        let dh = if prev_close > 0.0 { 100.0 * (cur.close - prev_close) / prev_close } else { 0.0 };
        let d24 = if close_24 > 0.0 { 100.0 * (cur.close - close_24) / close_24 } else { 0.0 };
        let sell_share = 100.0 * cur.sell_usd / cur.vol_usd;

        let spike_s = if base_s > 0.0 { cur.sell_usd / base_s } else { 0.0 };
        let spike_b = if base_b > 0.0 { cur.buy_usd / base_b } else { 0.0 };

        // емісія в грошах за поточною ціною — знаменник для «скільки це насправді»
        let (units, emis_note) = supply.units(&w, &sym, cur.vol_usd, cur.close);
        let emis_usd = units.map(|u| u * cur.close).unwrap_or(0.0);
        let pct = |flow: f64| if emis_usd > 0.0 { 100.0 * flow / emis_usd } else { 0.0 };

        board.push(Sig { sym: sym.clone(), w: w.clone(), cur: cur.clone(), base: base_s, spike: spike_s,
            sell_share, dh, d24, bs, emis_usd, emis_pct: pct(cur.sell_usd), emis_note });

        // База з менш ніж 12 відер — це не база. Рядок пишемо, алерт не шлемо.
        if samples < 12 { continue }
        // Поріг у грошах АБО в частці емісії: дрібний альт може прогнати через стакан
        // відсоток усього випуску на смішну суму — для нас це подія, а $25 тис. — ні.
        let big_sell = cur.sell_usd >= min_usd || pct(cur.sell_usd) >= min_emis_pct;
        let big_buy = cur.buy_usd >= min_usd || pct(cur.buy_usd) >= min_emis_pct;
        if big_sell && spike_s >= mult && sell_share >= sell_pct {
            sells.push(Sig { sym: sym.clone(), w: w.clone(), cur: cur.clone(), base: base_s, spike: spike_s,
                sell_share, dh, d24, bs, emis_usd, emis_pct: pct(cur.sell_usd), emis_note });
        } else if big_buy && spike_b >= mult && sell_share <= 100.0 - sell_pct {
            buys.push(Sig { sym: sym.clone(), w, cur: cur.clone(), base: base_b, spike: spike_b,
                sell_share, dh, d24, bs, emis_usd, emis_pct: pct(cur.buy_usd), emis_note });
        }
    }

    // Порядок — за часткою емісії, а не за доларами: ми відштовхуємось від емісії.
    // Активи з невідомою емісією (emis_pct = 0) опускаються вниз, але не зникають.
    let by_emission = |a: &Sig, b: &Sig| b.emis_pct.partial_cmp(&a.emis_pct).unwrap_or(std::cmp::Ordering::Equal);
    sells.sort_by(by_emission);
    buys.sort_by(by_emission);

    // ── стан ──
    // Раніше тут була пауза в годинах, і вона з'їдала саме те, заради чого все робиться:
    // сплеск ловився, а повідомлення не йшло. Тепер ключ — НОМЕР ВІКНА: кожен новий
    // сплеск у новому вікні шлеться, а повтор того самого вікна (два проходи поспіль,
    // ручний запуск) — ні. COOLDOWN_MIN лишається як запобіжник і за замовчуванням 0.
    let mut state: Value = read_json(&state_path).unwrap_or_else(|| serde_json::json!({"alerts":{}, "lastDigest":0}));
    let token = envv(root, "TELEGRAM_BOT_TOKEN");
    let chat = envv(root, "TELEGRAM_CHAT_ID");

    // ── журнал сигналів ──
    // Пишемо ДО рішення про відправку і незалежно від нього: сигнал — це те, що знайшов
    // детектор, а не те, що долетіло в Telegram. Інакше пауза між алертами тихо вирізала б
    // з вибірки частину спостережень, і підсумкова статистика брехала б на власну користь.
    // Збій журналу не глушить алерти (це головна функція біна), але волає в лог.
    // Тека журналу — та сама, з якої взято дані сигналу (`WR_DATASET`), а не поточна:
    // інакше запис лягав би в робочий каталог того, хто запустив бін, і вибірка
    // розповзлася б по кількох файлах.
    let jdir = std::env::var("SIGNAL_JOURNAL_DIR").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| ds.to_string());
    let mut jrn = match wr_store::journal::Journal::open_at(&jdir) {
        Ok(j) => Some(j),
        Err(e) => { eprintln!("[selltape] ЖУРНАЛ НЕ ВІДКРИВСЯ: {e} — сигнали цього проходу не перевіриш"); None }
    };
    let mut logged = 0usize;

    let mut sent = 0;
    for (list, side) in [(&sells, "sell"), (&buys, "buy")] {
        for s in list.iter().take(top) {
            if let Some(j) = jrn.as_mut() {
                let flow = if side == "sell" { s.cur.sell_usd } else { s.cur.buy_usd };
                let mut rec = wr_store::journal::signal(
                    "selltape", side, &s.sym, s.cur.ts, now,
                    &s.cur.exchange, &s.cur.pair, s.cur.close,
                    serde_json::json!({
                        "tf": tf,
                        "spike": s.spike,
                        "sell_share": s.sell_share,
                        "flow_usd": flow,
                        "vol_usd": s.cur.vol_usd,
                        // частка емісії — наша лінійка; 0 = емісія невідома, і в звіті
                        // такі спостереження мусять лишатись окремою купкою, а не змішуватись
                        "emis_pct": s.emis_pct,
                        "emis_usd": s.emis_usd,
                        "group": s.w.group,
                        "group_share": s.w.share,
                        "src": s.cur.src,
                    }),
                );
                // Контракт — єдиний надійний ключ до ціни: тикерів-двійників у наших
                // даних доведено вистачає, і дозрівання за тикером підставляло б чужу ціну.
                rec.contract = s.w.contract.clone();
                rec.chain = if s.w.contract.is_empty() { String::new() } else { "ethereum".into() };
                rec.note = s.emis_note.to_string();
                match j.append(&rec) {
                    Ok(true) => logged += 1,
                    Ok(false) => {}
                    Err(e) => eprintln!("[selltape] сигнал {} не записано: {e}", rec.id),
                }
            }

            let key = format!("{}:{}", s.sym, side);
            let prev = state.pointer(&format!("/alerts/{key}")).and_then(|v| v.as_i64()).unwrap_or(0);
            if prev == s.cur.ts { continue }                      // це вікно вже відправлено
            if cooldown > 0 && s.cur.ts - prev < cooldown { continue }
            let text = alert_text(s, side);
            println!("{}", plain(&text));
            if !dry && tg(&token, &chat, &text) {
                state["alerts"][&key] = serde_json::json!(s.cur.ts);
                sent += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
    if let Some(j) = jrn.as_ref() {
        if logged > 0 { eprintln!("[selltape] у журнал записано сигналів: {logged} ({})", j.signals_path().display()) }
        if j.bad_lines() > 0 { eprintln!("[selltape] УВАГА: у журналі биті рядки: {}", j.bad_lines()) }
    }

    // ── дайджест: рівна картина по всіх активах, а не лише сплески ──
    let last_digest = state.get("lastDigest").and_then(|v| v.as_i64()).unwrap_or(0);
    if digest_h > 0 && now - last_digest >= digest_h && !board.is_empty() {
        // сортуємо за часткою емісії, що пішла в продаж — це наша лінійка
        board.sort_by(|a, b| {
            let na = if a.emis_usd > 0.0 { (a.cur.sell_usd - a.cur.buy_usd) / a.emis_usd } else { 0.0 };
            let nb = if b.emis_usd > 0.0 { (b.cur.sell_usd - b.cur.buy_usd) / b.emis_usd } else { 0.0 };
            nb.partial_cmp(&na).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut t = format!("📊 СТРІЧКА АКТИВІВ — вікно {}–{} UTC ({tf})\n<i>нетто = купівлі мінус продажі агресором</i>\n",
            hhmm(last_closed), hhmm(last_closed + bs));
        for s in board.iter().take(top) {
            let net = s.cur.buy_usd - s.cur.sell_usd;
            let mark = if net < 0.0 { "🔴" } else { "🟢" };
            t.push_str(&format!("\n{mark} <b>{}</b> {} · нетто {} · прод. {:.0}% · {:+.1}%",
                s.sym, price_fmt(s.cur.close), usd(net), s.sell_share, s.dh));
        }
        let quiet = board.len().saturating_sub(top);
        if quiet > 0 { t.push_str(&format!("\n\nще {quiet} активів — без помітного перекосу")); }
        println!("{}", plain(&t));
        if !dry && tg(&token, &chat, &t) { state["lastDigest"] = serde_json::json!(now); }
    }

    let _ = std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap_or_default());
    eprintln!("[selltape] сплесків продажу {} · викупу {} · надіслано {sent}", sells.len(), buys.len());
}

// ───────────────────────────── денний звіт ─────────────────────────────

/// Доба одним листом: скільки в кожному активі пройшло через стакан, скільки це від
/// емісії, куди зсунулась ціна. Рахується з власного CSV, тобто з того, що ми справді
/// бачили, — без дозбирування історії з бірж і без задніх чисел.
fn daily_report(out_dir: &str, ds: &str, root: &str) {
    let tf = env_s("TF", "5m");
    let top = env_u("TOP", 20) as usize;
    let dry = env_u("DRY", 0) == 1;
    let hours = env_u("REPORT_H", 24) as i64;

    let rows = load_csv(&format!("{out_dir}/tape-{tf}.csv"));
    if rows.is_empty() { eprintln!("[selltape] звіт: стрічка порожня"); return }
    let now = now_s();
    let from = now - hours * 3600;

    let wl = watchlist(ds);
    let supply = load_supply(ds);

    struct Day { buy: f64, sell: f64, vol: f64, first: f64, last: f64, first_ts: i64, last_ts: i64, wins: usize }
    let mut agg: HashMap<String, Day> = HashMap::new();
    let mut sells_by_win: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    for r in rows.iter().filter(|r| r.ts >= from) {
        let d = agg.entry(r.symbol.clone()).or_insert(Day {
            buy: 0.0, sell: 0.0, vol: 0.0, first: r.close, last: r.close,
            first_ts: r.ts, last_ts: r.ts, wins: 0 });
        d.buy += r.buy_usd; d.sell += r.sell_usd; d.vol += r.vol_usd; d.wins += 1;
        if r.ts <= d.first_ts { d.first_ts = r.ts; d.first = r.close }
        if r.ts >= d.last_ts { d.last_ts = r.ts; d.last = r.close }
        sells_by_win.entry(r.symbol.clone()).or_default().push((r.sell_usd, r.vol_usd));
    }
    // скільки вікон за добу були сплеском проти власної медіани
    // «Сплесків N» означає рівно одне: стільки разів за добу спрацював би алерт —
    // ті самі пороги, що й у живому проході. Інакше число ні з чим не звіряється:
    // сама лише кратність до медіани в тонкому активі робить сплеском кожну дрібну
    // угоду. Рахуємо нижче, коли вже відома емісія (вона входить у поріг).
    let mult = env_f("MULT", 2.5);
    let min_usd = env_f("MIN_USD", 25_000.0);
    let sell_pct = env_f("SELL_PCT", 55.0);
    let min_emis_pct = env_f("MIN_EMIS_PCT", 0.05);

    struct Line { sym: String, net: f64, sell: f64, buy: f64, vol: f64, emis_pct: f64, dp: f64, spikes: usize, share: f64, group: String, note: &'static str }
    let mut lines: Vec<Line> = agg.into_iter().map(|(sym, d)| {
        let w = wl.get(&sym).cloned().unwrap_or_default();
        let (units, note) = supply.units(&w, &sym, d.vol, d.last);
        let emis_usd = units.map(|u| u * d.last).unwrap_or(0.0);
        let net = d.buy - d.sell;

        let spikes = sells_by_win.get(&sym).map(|v| {
            let mut only_sell: Vec<f64> = v.iter().map(|(s, _)| *s).collect();
            let med = median(&mut only_sell);
            if med <= 0.0 { return 0 }
            v.iter().filter(|(sell, vol)| {
                let big = *sell >= min_usd
                    || (emis_usd > 0.0 && 100.0 * *sell / emis_usd >= min_emis_pct);
                *sell >= mult * med && big && *vol > 0.0 && 100.0 * *sell / *vol >= sell_pct
            }).count()
        }).unwrap_or(0);
        Line {
            sym, net, sell: d.sell, buy: d.buy, vol: d.vol,
            emis_pct: if emis_usd > 0.0 { 100.0 * d.vol / emis_usd } else { 0.0 },
            dp: if d.first > 0.0 { 100.0 * (d.last - d.first) / d.first } else { 0.0 },
            spikes, share: w.share, group: w.group, note,
        }
    }).collect();

    // порядок — від емісії: скільки відсотків випуску прогнали через стакан
    lines.sort_by(|a, b| b.emis_pct.partial_cmp(&a.emis_pct).unwrap_or(std::cmp::Ordering::Equal));

    let tot_sell: f64 = lines.iter().map(|l| l.sell).sum();
    let tot_buy: f64 = lines.iter().map(|l| l.buy).sum();
    let mut t = format!(
        "🗓 <b>ДЕННИЙ ЗВІТ</b> — {hours} год, {} активів
продано {} · куплено {} · нетто {}
         <i>порядок за часткою емісії, що пройшла через стакан</i>
",
        lines.len(), usd(tot_sell), usd(tot_buy), usd(tot_buy - tot_sell));
    for l in lines.iter().take(top) {
        let mark = if l.net < 0.0 { "🔴" } else { "🟢" };
        let em = if l.emis_pct > 0.0 { format!("{:.2}% емісії", l.emis_pct) } else { "емісія невідома".into() };
        let grp = if l.share > 0.0 { format!(" · {} {:.1}%", group_label(&l.group), l.share) } else { String::new() };
        let sp = if l.spikes > 0 { format!(" · сплесків {}", l.spikes) } else { String::new() };
        let nt = if l.note.is_empty() { "" } else { " ⚠" };
        t.push_str(&format!("
{mark} <b>{}</b> оборот {} = {em}{nt} · нетто {} · ціна {:+.1}%{sp}{grp}",
            l.sym, usd(l.vol), usd(l.net), l.dp));
    }
    let rest = lines.len().saturating_sub(top);
    if rest > 0 { t.push_str(&format!("

ще {rest} активів — нижче за часткою емісії")); }

    println!("{}", plain(&t));
    if dry { return }
    let token = envv(root, "TELEGRAM_BOT_TOKEN");
    let chat = envv(root, "TELEGRAM_CHAT_ID");
    // Telegram ріже на 4096 — б'ємо по рядках, щоб не загубити хвіст звіту
    for part in chunks(&t, 3900) { tg(&token, &chat, &part); std::thread::sleep(std::time::Duration::from_millis(400)); }
    eprintln!("[selltape] денний звіт надіслано: {} активів", lines.len());
}

/// Ріже текст по рядках так, щоб жоден шматок не перевищив ліміт Telegram.
fn chunks(t: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in t.lines() {
        if cur.len() + line.len() + 1 > limit && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push_str("\n");
    }
    if !cur.trim().is_empty() { out.push(cur) }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let only: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with("--")).collect();
    let out_dir = env_s("WR_OUT", "D:/wr-live");
    let ds = env_s("WR_DATASET", "D:/wallet-radar-backup/current/dataset");
    let root = env_s("WR_ROOT", "C:/Users/StarOsta/wallet-tracker-CODE-backup-2026-09-08");
    let loop_min = env_u("LOOP_MIN", 0);

    if args.iter().any(|a| a == "--daily") {
        daily_report(&out_dir, &ds, &root);
        return
    }

    loop {
        pass(&out_dir, &ds, &root, &only);
        if loop_min == 0 { break }
        eprintln!("[selltape] наступний прохід через {loop_min} хв");
        std::thread::sleep(std::time::Duration::from_secs(loop_min * 60));
    }
}
