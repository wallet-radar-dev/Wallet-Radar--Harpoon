//! net6kpnl — скільки група заробила за весь час, за СЕРЕДНІМИ ТОЧКАМИ входу й виходу.
//!
//! Метод (навмисно простий і перевірюваний):
//!   * кожен вхід токена в мережу оцінюється історичною ціною НА ДЕНЬ операції;
//!   * кожен вихід — так само;
//!   * середня точка входу = Σ(обсяг×ціна_дня) / Σ(обсяг) — тобто зважена за обсягом;
//!   * реалізований результат = обсяг_виходу × (середній_вихід − середній_вхід);
//!   * нереалізований = залишок × (ціна_зараз − середній_вхід).
//!
//! Чому не FIFO: FIFO вимагає впорядкованої черги лотів на КОЖЕН гаманець (тут їх 6864 ×
//! тисячі токенів) і при цьому так само не знає, що всередині мережі є внутрішні перекази.
//! Середня точка стійкіша до перестановок і не вигадує точності, якої в даних немає.
//!
//! ВАЖЛИВО ПРО ВНУТРІШНІ ПЕРЕКАЗИ: переказ між двома членами мережі — це НЕ купівля і НЕ
//! продаж, тож він виключається з обох плечей. Інакше та сама монета рахувалась би входом
//! і виходом багато разів (саме через це «оборот» доходив до 3103% емісії USDP).
//!
//! ПІК ВОЛОДІННЯ рахується хронологічно: події (час, дельта) на токен сортуються за часом,
//! і береться максимум накопиченого балансу. Сума входів піком НЕ є.
//!
//! Вихід: dataset/backfill/net6k-pnl.json. Args: ROOT [WALLET_FILE]. Env: CHAIN(1) TOP(30)
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":\"");
    let i = line.find(&pat)? + pat.len();
    let rest = &line[i..];
    let j = rest.find('"')?;
    Some(&rest[..j])
}

/// Ціна токена на день `ts`: у ряді денних точок беремо найближчу зліва (ціну того дня).
fn price_at(series: &[(u64, f64)], ts: u64) -> Option<f64> {
    if series.is_empty() { return None }
    match series.binary_search_by_key(&ts, |p| p.0) {
        Ok(i) => Some(series[i].1),
        Err(0) => Some(series[0].1),            // раніше за перший запис — беремо найдавнішу
        Err(i) => Some(series[i - 1].1),
    }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let chain: u32 = envn("CHAIN", 1);
    let top: usize = envn("TOP", 30);
    let wfile = std::env::args().nth(2).unwrap_or_else(|| format!("{bf}/net-members.txt"));

    let members: HashSet<String> = BufReader::new(File::open(&wfile).expect("wallet file"))
        .lines().map_while(Result::ok)
        .map(|l| l.trim().to_lowercase())
        .filter(|l| l.len() == 42)
        .collect();
    eprintln!("[pnl] членів мережі: {}", members.len());

    // поточні ціни
    let cur: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices.json")).expect("token-prices.json"))).unwrap();
    let eth_now = cur["eth"].as_f64().unwrap_or(0.0);
    let mut price_now: HashMap<String, f64> = HashMap::new();
    let mut symbol: HashMap<String, String> = HashMap::new();
    if let Some(o) = cur["tokens"].as_object() {
        for (c, v) in o {
            price_now.insert(c.to_lowercase(), v["usd"].as_f64().unwrap_or(0.0));
            symbol.insert(c.to_lowercase(), v["symbol"].as_str().unwrap_or("").to_string());
        }
    }

    // історичні ряди
    let hist: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{bf}/token-prices-hist.json")).expect("token-prices-hist.json"))).unwrap();
    let mut series: HashMap<String, Vec<(u64, f64)>> = HashMap::new();
    // histprices пише {"span":N,"tokens":{контракт:{sym,prices}}} — ряди лежать під "tokens",
    // а не на верхньому рівні. Читання верхнього рівня давало 0 рядів, усі ціни падали на
    // поточну, і різниця вхід/вихід ставала нульовою — звідси «результат $0».
    let hist_root = if hist.get("tokens").is_some() { &hist["tokens"] } else { &hist };
    if let Some(o) = hist_root.as_object() {
        for (c, v) in o {
            let mut pts: Vec<(u64, f64)> = v["prices"].as_array().cloned().unwrap_or_default().iter()
                .filter_map(|p| Some((p[0].as_u64()?, p[1].as_f64()?)))
                .collect();
            pts.sort_by_key(|p| p.0);
            if pts.len() >= 10 { series.insert(c.to_lowercase(), pts); }
            if let Some(s) = v["sym"].as_str() {
                symbol.entry(c.to_lowercase()).or_insert_with(|| s.to_string());
            }
        }
    }
    let eth_series = series.get("eth").cloned().unwrap_or_default();
    eprintln!("[pnl] історичних рядів: {} (ETH-ряд: {} точок)", series.len(), eth_series.len());

    #[derive(Default)]
    struct Tok {
        in_amt: f64, in_usd: f64,      // купівлі (ззовні мережі)
        out_amt: f64, out_usd: f64,    // продажі (назовні)
        // зважені обсягом моменти часу: різниця середніх дає середній строк утримання.
        // Саме він відрізняє ПОЗИЦІЮ (купили, тримали, продали) від ТРАНЗИТУ (зайшло-вийшло
        // того ж дня) — при транзиті середні ціни входу й виходу збігаються не тому, що
        // трейдер вдалий, а тому що це та сама монета в тому самому дні.
        in_ts_w: f64, out_ts_w: f64,
        held: f64,
        ev: Vec<(u64, f64)>,           // хронологія для піку
        priced_in: f64, priced_out: f64, // яка частка обсягу отримала історичну ціну
    }
    let mut toks: HashMap<String, Tok> = HashMap::new();
    let (mut eth_in_usd, mut eth_out_usd, mut eth_in, mut eth_out) = (0f64, 0f64, 0f64, 0f64);

    let path = format!("{bf}/txfull-{chain}.jsonl");
    let rdr = BufReader::with_capacity(1 << 22, File::open(&path).expect("txfull"));
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur_w = String::new();
    let (mut n, mut kept, mut internal) = (0u64, 0u64, 0u64);

    for line in rdr.lines().map_while(Result::ok) {
        n += 1;
        if n % 10_000_000 == 0 { eprintln!("    ...{n} рядків, наших {kept}, внутрішніх {internal}"); }
        let Some(w) = field(&line, "w") else { continue };
        if !members.contains(w) { continue }
        if w != cur_w { cur_w = w.to_string(); seen.clear(); }
        let Ok(t) = serde_json::from_str::<Value>(&line) else { continue };

        let mut h: u64 = 1469598103934665603;
        for s in [t["h"].as_str().unwrap_or(""), t["k"].as_str().unwrap_or(""),
                  t["f"].as_str().unwrap_or(""), t["t"].as_str().unwrap_or(""),
                  t["v"].as_str().unwrap_or("")] {
            for b in s.as_bytes() { h ^= *b as u64; h = h.wrapping_mul(1099511628211); }
        }
        if !seen.insert(h) { continue }
        kept += 1;

        let kind = t["k"].as_str().unwrap_or("");
        let frm = t["f"].as_str().unwrap_or("").to_lowercase();
        let to = t["t"].as_str().unwrap_or("").to_lowercase();
        let ts: u64 = t["ts"].as_u64().unwrap_or(0);
        let raw: f64 = t["v"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if raw <= 0.0 || ts == 0 { continue }

        // Обидві сторони в мережі -> це переміщення всередині групи, а не угода з ринком.
        let inside = members.contains(frm.as_str()) && members.contains(to.as_str());
        if inside { internal += 1; }

        if kind == "tok" {
            let c = t["tok"].as_str().unwrap_or("").to_lowercase();
            if c.is_empty() { continue }
            let dec: i32 = t["dec"].as_str().and_then(|s| s.parse().ok()).unwrap_or(18);
            let amt = raw / 10f64.powi(dec);
            if amt <= 0.0 { continue }
            let e = toks.entry(c.clone()).or_default();
            let px = series.get(&c).and_then(|s| price_at(s, ts));
            if to == cur_w {
                e.held += amt;
                e.ev.push((ts, amt));
                if !inside {
                    e.in_amt += amt;
                    e.in_ts_w += amt * ts as f64;
                    if let Some(p) = px { e.in_usd += amt * p; e.priced_in += amt; }
                }
            } else if frm == cur_w {
                e.held -= amt;
                e.ev.push((ts, -amt));
                if !inside {
                    e.out_amt += amt;
                    e.out_ts_w += amt * ts as f64;
                    if let Some(p) = px { e.out_usd += amt * p; e.priced_out += amt; }
                }
            }
        } else if kind == "ext" || kind == "int" {
            if inside { continue }
            let v = raw / 1e18;
            let p = price_at(&eth_series, ts).unwrap_or(eth_now);
            if to == cur_w { eth_in += v; eth_in_usd += v * p }
            else if frm == cur_w { eth_out += v; eth_out_usd += v * p }
        }
    }
    eprintln!("[pnl] рядків {n}, наших {kept}, внутрішніх переказів {internal}");
    drop(seen);

    let mut rows: Vec<Value> = Vec::new();
    // ПІДТВЕРДЖЕНІ залишки з realbal. Потоковий залишок (входи − виходи) завищений:
    // історія неповна, частина виходів відсутня. На AAVE це дало нереалізовані $2.7e60.
    // Де підтвердженого балансу немає — нереалізований не рахуємо взагалі, бо вигадувати
    // його з потоків означає повторити ту саму помилку.
    let mut real_held: HashMap<String, f64> = HashMap::new();
    let mut have_real = false;
    if let Ok(f) = File::open(format!("{bf}/net6k-real-pairs.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                let tok = v["tok"].as_str().unwrap_or("").to_lowercase();
                let r = v["real"].as_f64().unwrap_or(0.0);
                if r > 0.0 { *real_held.entry(tok).or_insert(0.0) += r; }
            }
        }
        have_real = !real_held.is_empty();
        eprintln!("[pnl] підтверджених залишків: {} токенів", real_held.len());
    }
    if !have_real {
        eprintln!("[pnl] УВАГА: net6k-real-pairs.jsonl немає — нереалізований буде пропущено");
    }

    let (mut real_total, mut unreal_total) = (0f64, 0f64);
    for (c, mut e) in toks {
        if e.in_amt <= 0.0 && e.out_amt <= 0.0 { continue }
        let avg_in = if e.priced_in > 0.0 { e.in_usd / e.priced_in } else { 0.0 };
        let avg_out = if e.priced_out > 0.0 { e.out_usd / e.priced_out } else { 0.0 };
        // пік володіння — хронологічно, а не сумою входів
        e.ev.sort_by_key(|x| x.0);
        let (mut run, mut peak) = (0f64, 0f64);
        for (_, d) in &e.ev { run += *d; if run > peak { peak = run } }
        // середній строк утримання = різниця зважених за обсягом моментів виходу і входу
        let hold_days = if e.in_amt > 0.0 && e.out_amt > 0.0 {
            (e.out_ts_w / e.out_amt - e.in_ts_w / e.in_amt) / 86400.0
        } else { 0.0 };
        let pnow = price_now.get(&c).copied().unwrap_or(0.0);
        // тільки підтверджений баланс; потоковий не використовуємо навіть як запасний
        let held = real_held.get(&c).copied().unwrap_or(0.0);
        let held_flow = if e.held > 0.0 { e.held } else { 0.0 };
        // реалізовано лише там, де обидві точки відомі
        let realized = if avg_in > 0.0 && avg_out > 0.0 { e.priced_out.min(e.out_amt) * (avg_out - avg_in) } else { 0.0 };
        let unrealized = if avg_in > 0.0 && pnow > 0.0 { held * (pnow - avg_in) } else { 0.0 };
        real_total += realized;
        unreal_total += unrealized;
        rows.push(json!({
            "token": c, "symbol": symbol.get(&c).cloned().unwrap_or_default(),
            "avgEntryUsd": avg_in, "avgExitUsd": avg_out, "priceNow": pnow,
            "boughtAmt": e.in_amt, "soldAmt": e.out_amt, "heldNow": held,
            "heldFlowEstimate": held_flow, "peakHeld": peak,
            "investedUsd": e.in_usd, "proceedsUsd": e.out_usd,
            "realizedUsd": realized, "unrealizedUsd": unrealized,
            "totalUsd": realized + unrealized,
            "pricedCoverIn": if e.in_amt > 0.0 { e.priced_in / e.in_amt } else { 0.0 },
            "holdDays": hold_days,
        }));
    }
    rows.sort_by(|a, b| b["totalUsd"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["totalUsd"].as_f64().unwrap_or(0.0)).unwrap());

    let eth_avg_in = if eth_in > 0.0 { eth_in_usd / eth_in } else { 0.0 };
    let eth_avg_out = if eth_out > 0.0 { eth_out_usd / eth_out } else { 0.0 };
    let eth_pnl = eth_out_usd - eth_in_usd + (eth_in - eth_out).max(0.0) * eth_now;

    let out = json!({
        "members": members.len(),
        "ethIn": eth_in, "ethOut": eth_out,
        "ethInUsd": eth_in_usd, "ethOutUsd": eth_out_usd,
        "ethAvgEntry": eth_avg_in, "ethAvgExit": eth_avg_out, "ethNowPrice": eth_now,
        "ethResultUsd": eth_pnl,
        "tokenRealizedUsd": real_total, "tokenUnrealizedUsd": unreal_total,
        "grandTotalUsd": real_total + unreal_total + eth_pnl,
        "rows": rows,
    });
    let outp = format!("{bf}/net6k-pnl.json");
    let _ = File::create(&outp).unwrap().write_all(serde_json::to_string(&out).unwrap().as_bytes());

    eprintln!("\n=== РЕЗУЛЬТАТ ГРУПИ ЗА ВЕСЬ ЧАС ===");
    // Межа здорового глузду. Обіг понад емісію ETH означає, що ті самі монети пройшли
    // по колу багато разів (циркуляція доведена взаємними парами з симетрією 67-91%),
    // і кожен оберт зарахувався як нова купівля. Без цього попередження результат по ETH
    // читається як прибуток, хоча є артефактом обліку.
    if let Some(w) = backfill::known::sanity_volume(eth_in, backfill::known::ETH_SUPPLY, "ETH") {
        eprintln!("[pnl] УВАГА · {w}");
        eprintln!("[pnl] результат по ETH нижче НЕ є прибутком — це артефакт багаторазового обліку");
    }
    eprintln!("ETH: куплено {:.0} (сер. ${:.0}) · продано {:.0} (сер. ${:.0}) · зараз ${:.0}",
        eth_in, eth_avg_in, eth_out, eth_avg_out, eth_now);
    eprintln!("  результат по ETH: ${:.0}", eth_pnl);
    eprintln!("ТОКЕНИ: реалізовано ${:.0} · нереалізовано ${:.0}", real_total, unreal_total);
    eprintln!("РАЗОМ: ${:.0}", real_total + unreal_total + eth_pnl);
    eprintln!("\n=== ТОП-{top} ЗА РЕЗУЛЬТАТОМ ===");
    for r in rows.iter().take(top) {
        eprintln!("  {:<10} разом ${:>13.0} · вхід ${:.4} вихід ${:.4} · тримали {:>6.1} дн · продано {:.0}% від купленого",
            r["symbol"].as_str().unwrap_or(""), r["totalUsd"].as_f64().unwrap_or(0.0),
            r["avgEntryUsd"].as_f64().unwrap_or(0.0), r["avgExitUsd"].as_f64().unwrap_or(0.0),
            r["holdDays"].as_f64().unwrap_or(0.0),
            100.0 * r["soldAmt"].as_f64().unwrap_or(0.0) / r["boughtAmt"].as_f64().unwrap_or(1.0));
    }
    eprintln!("\n[pnl] збережено {outp}");
}
