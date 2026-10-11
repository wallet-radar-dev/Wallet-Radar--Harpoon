//! sigretro — ретро-реплей сигналу `selltape` по історії біржі: присуд сьогодні, а не за три місяці.
//!
//! ІДЕЯ. Живий журнал чесний, але повільний: щоб побачити 30-денний горизонт, треба
//! прожити 30 днів. Водночас та сама подія повністю відтворювана заднім числом —
//! свічки Binance віддають `takerBuyQuoteVolume`, тобто ту саму сторону агресора, на
//! якій тримається весь сигнал. Тож ми проганяємо ТОЙ САМИЙ детектор (`tape_core`,
//! спільний із живим біном) по історії й одразу бачимо, що сталося далі.
//!
//! ЧЕСНІСТЬ: три перекоси, яких у ретро не уникнути, і які мусять стояти в звіті.
//!  1. ВИЖИВАННЯ. Список спостереження — активи, які групи тримають СЬОГОДНІ. Токени,
//!     з яких вони вийшли (чи які померли), у вибірку не потрапляють. Ретро тому дивиться
//!     на світ, де вибір уже зроблено вдало.
//!  2. ЗАЗИРАННЯ ВПЕРЕД у частці емісії. `emis_pct` рахується з сьогоднішнього випуску й
//!     сьогоднішньої частки групи — рік тому вони були інші. Саме тому зріз за емісією в
//!     ретро слабший за живий: там ці числа були відомі на момент сигналу.
//!  3. ВИЖИВАННЯ ЛІСТИНГУ. Пари, делістингнуті з Binance, історії не віддають зовсім.
//!
//! Результати пишуться тим самим журналом (`wr_store::journal`), але під ОКРЕМИМ джерелом
//! `retro-<tf>`: змішувати їх у одну купку з живими не можна — у живих цих перекосів немає.
//!
//! Ціни для дозрівання беруться з ТОГО САМОГО ряду свічок, що й сигнал: обидва кінці вікна
//! з одного джерела, тож різниця між біржами не підмішується в дохідність. Базовий актив —
//! ETHUSDT, теж свічками.
//!
//! Args: ROOT. Env: TF(1h) SPAN_D(540) HORIZONS(1,7,30,90) MULT(2.5) MIN_USD(25000)
//!       SELL_PCT(55) MIN_EMIS_PCT(0.05) BASE_N(144) MAX_SYMBOLS(120) SLEEP_MS(120)
//!       ONLY(sym,sym) DRY(0)

/// Спільна основа: ті самі довідники, свічки, емісія й вотчліст, що в живому `selltape`.
#[path = "../tape_core.rs"] mod tape_core;
use tape_core::*;

/// Одна знахідка детектора в історії.
struct Found {
    sym: String,
    w: Watch,
    row: Row,
    side: &'static str,
    spike: f64,
    sell_share: f64,
    emis_pct: f64,
    emis_usd: f64,
    flow_usd: f64,
    note: &'static str,
}

/// Ціна найближчого відра НЕ РАНІШЕ за `ts`, якщо воно не далі за `tol`.
///
/// Чому з допуском: у рядку бувають діри (біржа стояла, пара не торгувалась). Мовчки
/// взяти сусіда за тиждень — це підмінити результат; краще не дати відповіді зовсім.
fn close_at(series: &[Row], ts: i64, tol: i64) -> Option<f64> {
    let i = series.partition_point(|r| r.ts < ts);
    let cand = match (i.checked_sub(1).and_then(|k| series.get(k)), series.get(i)) {
        (_, Some(a)) if (a.ts - ts).abs() <= tol => Some(a),
        (Some(b), _) if (ts - b.ts).abs() <= tol => Some(b),
        _ => None,
    }?;
    if cand.close > 0.0 { Some(cand.close) } else { None }
}

fn main() {
    let out_dir = env_s("WR_OUT", "D:/wr-live");
    let ds = env_s("WR_DATASET", "D:/wallet-radar-backup/current/dataset");
    let tf = env_s("TF", "1h");
    let bs = tf_secs(&tf);
    let span_d = env_u("SPAN_D", 540) as i64;
    let horizons: Vec<u32> = env_s("HORIZONS", "1,7,30,90").split(',')
        .filter_map(|s| s.trim().parse().ok()).filter(|h| *h > 0).collect();
    let mult = env_f("MULT", 2.5);
    let min_usd = env_f("MIN_USD", 25_000.0);
    let sell_pct = env_f("SELL_PCT", 55.0);
    let min_emis_pct = env_f("MIN_EMIS_PCT", 0.05);
    let base_n = env_u("BASE_N", 144) as usize;
    let max_syms = env_u("MAX_SYMBOLS", 120) as usize;
    let sleep_ms = env_u("SLEEP_MS", 120);
    let dry = env_u("DRY", 0) == 1;
    let only: Vec<String> = env_s("ONLY", "").split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect();

    let now = now_s();
    let from = now - span_d * 86_400;
    let source = format!("retro-{tf}");
    // Тікер базового активу — його сигнали у вибірку не йдуть (див. цикл нижче).
    let bench_sym = env_s("BENCH_SYM", "ETH").to_uppercase();

    let ven = load_venues(&out_dir);
    let wl = watchlist(&ds);
    let supply = load_supply(&ds);

    let mut syms: Vec<(String, Watch)> = if only.is_empty() {
        wl.into_iter().collect()
    } else {
        only.iter().map(|s| { let w = wl.get(s).cloned().unwrap_or_default(); (s.clone(), w) }).collect()
    };
    // спершу ті, де частка емісії більша — там сигнал значить найбільше
    syms.sort_by(|a, b| b.1.share.partial_cmp(&a.1.share).unwrap_or(std::cmp::Ordering::Equal));
    syms.truncate(max_syms);

    // Базовий актив: без нього надлишок нема з чим рахувати, тож це відмова, а не попередження.
    eprintln!("[sigretro] тягну базовий ряд ETHUSDT ({tf}, {span_d} д)");
    let Some(bench) = binance_range("ETH", "ETHUSDT", &tf, from, now, sleep_ms) else {
        eprintln!("[sigretro] базовий ряд не прийшов — без нього надлишок не порахувати");
        std::process::exit(2)
    };
    if bench.len() < 100 {
        eprintln!("[sigretro] базовий ряд закороткий ({} відер) — рахувати нема від чого", bench.len());
        std::process::exit(2)
    }

    let jdir = std::env::var("SIGNAL_JOURNAL_DIR").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| ds.clone());
    let mut jrn = match wr_store::journal::Journal::open_at(&jdir) {
        Ok(j) => j,
        Err(e) => { eprintln!("[sigretro] журнал не відкрився: {e}"); std::process::exit(2) }
    };

    let tol = bs * 3;
    // sym -> горизонт -> {n, median_excess, share_up} для випадкового моменту входу
    let mut nulls: std::collections::HashMap<String, serde_json::Map<String, serde_json::Value>> = std::collections::HashMap::new();
    let mut found_total = 0usize;
    let mut written = 0usize;
    let mut outcomes = 0usize;
    let mut no_history = 0usize;
    let mut net_fail = 0usize;
    let mut symbols_used = 0usize;

    for (sym, w) in &syms {
        // Базовий актив не можна міряти проти самого себе: надлишок вийшов би тотожно
        // нульовим, і кожен такий запис тягнув би всю статистику до нуля, вдаючи
        // «сигнал нічого не дає». ETH у вотчлісті є як мажор мережі — саме тому явно.
        if sym == &bench_sym {
            eprintln!("[sigretro] {sym} — базовий актив, пропускаю (надлишок проти себе тотожно нульовий)");
            continue
        }
        let Some(pair) = ven.binance.get(sym) else { continue };
        let series = match binance_range(sym, pair, &tf, from, now, sleep_ms) {
            Some(s) => s,
            None => { net_fail += 1; eprintln!("[sigretro] {sym}: історія не прийшла — пропускаю (це не «сигналів немає»)"); continue }
        };
        if series.len() < base_n + 2 { no_history += 1; continue }
        symbols_used += 1;

        // ── детектор: точно та сама арифметика, що в живому проході ──
        let mut found: Vec<Found> = Vec::new();
        for i in base_n..series.len() {
            let cur = &series[i];
            if cur.vol_usd <= 0.0 { continue }
            let prev = &series[i - base_n..i];
            let mut ps: Vec<f64> = prev.iter().map(|r| r.sell_usd).collect();
            let mut pb: Vec<f64> = prev.iter().map(|r| r.buy_usd).collect();
            let base_s = median(&mut ps);
            let base_b = median(&mut pb);
            let sell_share = 100.0 * cur.sell_usd / cur.vol_usd;
            let spike_s = if base_s > 0.0 { cur.sell_usd / base_s } else { 0.0 };
            let spike_b = if base_b > 0.0 { cur.buy_usd / base_b } else { 0.0 };

            let (units, note) = supply.units(w, sym, cur.vol_usd, cur.close);
            let emis_usd = units.map(|u| u * cur.close).unwrap_or(0.0);
            let pct = |flow: f64| if emis_usd > 0.0 { 100.0 * flow / emis_usd } else { 0.0 };

            let big_sell = cur.sell_usd >= min_usd || pct(cur.sell_usd) >= min_emis_pct;
            let big_buy = cur.buy_usd >= min_usd || pct(cur.buy_usd) >= min_emis_pct;
            if big_sell && spike_s >= mult && sell_share >= sell_pct {
                found.push(Found { sym: sym.clone(), w: w.clone(), row: cur.clone(), side: "sell",
                    spike: spike_s, sell_share, emis_pct: pct(cur.sell_usd), emis_usd, flow_usd: cur.sell_usd, note });
            } else if big_buy && spike_b >= mult && sell_share <= 100.0 - sell_pct {
                found.push(Found { sym: sym.clone(), w: w.clone(), row: cur.clone(), side: "buy",
                    spike: spike_b, sell_share, emis_pct: pct(cur.buy_usd), emis_usd, flow_usd: cur.buy_usd, note });
            }
        }
        found_total += found.len();

        // ── запис у журнал + дозрівання з того самого ряду ──
        // Складаємо пачку на актив і пишемо одним рухом: по рядку з синхронізацією
        // диска десятки тисяч записів писалися б хвилинами.
        let mut batch_sig: Vec<wr_store::journal::Signal> = Vec::new();
        let mut batch_out: Vec<wr_store::journal::Outcome> = Vec::new();
        for fd in &found {
            let mut rec = wr_store::journal::signal(
                &source, fd.side, &fd.sym, fd.row.ts, now,
                &fd.row.exchange, &fd.row.pair, fd.row.close,
                serde_json::json!({
                    "tf": tf,
                    "spike": fd.spike,
                    "sell_share": fd.sell_share,
                    "flow_usd": fd.flow_usd,
                    "vol_usd": fd.row.vol_usd,
                    "emis_pct": fd.emis_pct,
                    "emis_usd": fd.emis_usd,
                    "group": fd.w.group,
                    "group_share": fd.w.share,
                    "src": fd.row.src,
                    // Позначка, що числа про емісію взяті з СЬОГОДНІШНЬОГО випуску:
                    // без неї через півроку ніхто не згадає, що тут зазирання вперед.
                    "retro": true,
                }),
            );
            rec.contract = fd.w.contract.clone();
            rec.chain = if fd.w.contract.is_empty() { String::new() } else { "ethereum".into() };
            rec.note = fd.note.to_string();
            if dry { continue }
            if jrn.has_signal(&rec.id) { continue }   // вже записаний раніше — і результати теж

            for &h in &horizons {
                let t1 = fd.row.ts + h as i64 * 86_400;
                if t1 > now { continue }               // ще не дозріло — дорахує sigcheck
                let o = match (close_at(&series, fd.row.ts, tol), close_at(&series, t1, tol),
                               close_at(&bench, fd.row.ts, tol), close_at(&bench, t1, tol)) {
                    (Some(p0), Some(p1), Some(b0), Some(b1)) if p0 > 0.0 && b0 > 0.0 => {
                        let ret = 100.0 * (p1 - p0) / p0;
                        let bret = 100.0 * (b1 - b0) / b0;
                        wr_store::journal::Outcome {
                            v: wr_store::journal::SCHEMA, id: rec.id.clone(), horizon_d: h, checked_at: now,
                            price0: Some(p0), price1: Some(p1),
                            ret_pct: Some(ret), bench_ret_pct: Some(bret), excess_pct: Some(ret - bret),
                            status: "ok".into(), reason: String::new(),
                        }
                    }
                    _ => wr_store::journal::Outcome::failed(&rec.id, h, now, "no-price",
                            &format!("{}: у ряді свічок немає відра ближче за {} с", fd.sym, tol)),
                };
                batch_out.push(o);
            }
            batch_sig.push(rec);
        }

        // Спершу сигнали, потім результати: якщо процес загине між цими двома записами,
        // лишаться сигнали без результатів (їх дорахує наступний прогін), а не навпаки —
        // результати-сироти, що посилаються на неіснуючі твердження.
        match jrn.append_many(&batch_sig) {
            Ok(k) => written += k,
            Err(e) => { eprintln!("[sigretro] {sym}: пачку сигналів не записано: {e}"); continue }
        }
        match jrn.append_outcomes_many(&batch_out) {
            Ok(k) => outcomes += k,
            Err(e) => eprintln!("[sigretro] {sym}: пачку результатів не записано: {e}"),
        }
        // ── НУЛЬОВА БАЗА ──
        // Найважливіша частина реплею. Без неї «після сигналу актив обігнав ETH на 9%»
        // нічого не означає: якщо актив обганяв ETH усі три місяці, те саме дала б
        // БУДЬ-ЯКА дата входу. Тому для кожного горизонту рахуємо те саме вікно з
        // КОЖНОГО відра ряду — і далі сигнал порівнюється не з нулем, а з цим.
        for &h in &horizons {
            let mut ex: Vec<f64> = Vec::new();
            let mut up = 0usize;
            for r in &series {
                let t1 = r.ts + h as i64 * 86_400;
                if t1 > now { break }
                let (Some(p0), Some(p1), Some(b0), Some(b1)) =
                    (close_at(&series, r.ts, tol), close_at(&series, t1, tol),
                     close_at(&bench, r.ts, tol), close_at(&bench, t1, tol)) else { continue };
                if p0 <= 0.0 || b0 <= 0.0 { continue }
                let e = 100.0 * (p1 - p0) / p0 - 100.0 * (b1 - b0) / b0;
                if e > 0.0 { up += 1 }
                ex.push(e);
            }
            if ex.len() < 50 { continue }
            let n = ex.len();
            nulls.entry(sym.clone()).or_default().insert(h.to_string(), serde_json::json!({
                "n": n,
                "median_excess": median(&mut ex),
                "share_up": 100.0 * up as f64 / n as f64,
            }));
        }

        // Нульова база лягає на диск ПІСЛЯ КОЖНОГО АКТИВА, а не в кінці прогону.
        // Причина побутова: ця машина гасне від зникнення живлення, а повний прохід
        // триває години. Записана в кінці база означала б, що після кожного збою
        // сигнали є, а порівнювати їх нема з чим — тобто журнал без присуду.
        if !dry {
            let path = format!("{jdir}/retro-null.json");
            // Ключ включає ТАЙМФРЕЙМ: нуль для годинних відер за 540 днів і для
            // п'ятихвилинних за 120 — різні величини. Спільний ключ означав би, що
            // пізніший прогін тихо підміняє базу ранішого, і присуд за годинами
            // раптом порівнювався б із хвилинами.
            let mut all = read_json(&path).unwrap_or_else(|| serde_json::json!({"by_tf": {}}));
            if all["by_tf"].as_object().is_none() { all["by_tf"] = serde_json::json!({}); }
            if let Some(per_h) = nulls.get(sym) {
                for (h, v) in per_h { all["by_tf"][&tf][sym][h] = v.clone(); }
            }
            all["asof"] = serde_json::json!(now);
            all["tf"] = serde_json::json!(tf);
            all["span_d"] = serde_json::json!(span_d);
            if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(&all).unwrap_or_default()) {
                eprintln!("[sigretro] {sym}: нульову базу НЕ записано: {e} — без неї присуд неповний");
            }
        }

        eprintln!("[sigretro] {sym}: відер {} · знахідок {} · записано {written} · результатів {outcomes}",
            series.len(), found.len());
    }

    eprintln!("[sigretro] ── підсумок ──");
    eprintln!("[sigretro] активів оброблено {symbols_used} з {} · без історії {no_history} · мережевих відмов {net_fail}", syms.len());
    eprintln!("[sigretro] знахідок {found_total} · нових записів {written} · результатів {outcomes}");
    if written == 0 && !dry {
        // Нуль записів при непорожньому списку активів означає або що все вже пораховано
        // раніше, або що детектор нічого не бачить. Мовчазний нуль тут неприпустимий.
        eprintln!("[sigretro] УВАГА: жодного НОВОГО запису — або реплей уже проганявся, або пороги нічого не ловлять");
    }
    eprintln!("[sigretro] звіт: sigcheck (джерело «{source}»)");
}
