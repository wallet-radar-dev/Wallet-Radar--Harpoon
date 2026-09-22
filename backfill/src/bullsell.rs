//! bullsell — яку частку емісії групи встигли ПРОДАТИ до розвороту ціни (булран-2021).
//!
//! Питання: група сиділа в активі й тримала основну частку емісії — скільки вона злила
//! ДО того, як ціна розвернулась, і скільки лишилось у руках уже на падінні.
//!
//! ДВА РЕЖИМИ, і порядок між ними принциповий:
//!   MODE=scan — які активи група взагалі тримала у вікні й на яку частку емісії.
//!               Цін НЕ потребує. Саме цей крок задає список активів; якщо починати з
//!               наявних цін, всесвіт звужується до того, що хтось колись викачав, і у
//!               відповідь потрапляють випадкові стейбли, а не те, де група справді сиділа.
//!   MODE=full — метрики розвороту по активах, для яких уже є денні ціни.
//!
//! Кожен крок методу — щоб не збрехати:
//!   * Позиція = накопичений НЕТТО-потік гаманців групи по токену з першого дня історії.
//!     Внутрішньогрупові перекази викинуто з ОБОХ боків: рух між своїми — не купівля й не
//!     продаж, а при неповному покритті вони ще й ламають сальдо (одну ногу видно, другу ні).
//!   * Частка емісії — через `wr_dict::emission_pct`, єдина формула словника.
//!   * Розворот рахуємо ПОТОКЕННО: день максимуму ціни самого активу у вікні, а не спільну
//!     дату по ринку — альти розвертались у різні тижні.
//!   * Продано до розвороту = падіння позиції від власного МАКСИМУМУ до дня піку ціни.
//!     Це нетто-зменшення, тобто НИЖНЯ межа: продаж, перекритий одночасною докупкою, не видно.
//!   * Гроші — за щоденною історичною ціною дня продажу, не за ціною піку й не за сьогоднішньою.
//!
//! ЗНАМЕННИК. supply береться СЬОГОДНІШНІЙ. Для токена, чия емісія з 2021-го впала
//! (aUSDT та інші обгортки), частка виходить >100% — це не «володіли всім», а свідчення
//! що знаменник не той. Такі пари позначаються `supplyEraSuspect` і НЕ йдуть у середні.
//!
//! Покриття: за замовчуванням рахуємо лише по гаманцях із ПОВНІСТЮ докачаною історією
//! (`_progress-grp.csv`, статус done), бо на недокачаних сальдо з потоків бреше. ONLY_DONE=0
//! вимикає фільтр — різниця між прогонами показує, наскільки цифри ще зрушать.
//!
//! Вихід: dataset/backfill/bullsell-<TAG>.json (full) / bullsell-scan-<TAG>.json (scan)
//! Env: MODE(scan|full) BULL_START(2021-01-01) BULL_END(2022-01-31) MIN_SHARE(20) ONLY_DONE(1)
//!      DROP_CONTRACTS(1) EXCLUDE_GROUPS(INFRA,NODATA) POST_DAYS(90) TAG(2021) TOP(400)

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};

/// Контракт токена за символом із довідника (`tokens`).
fn token_by_symbol(sym: &str) -> String {
    let Ok(c) = wr_store::open() else { return String::new() };
    let Ok(mut q) =
        c.prepare("SELECT contract FROM tokens WHERE chain='eth' AND symbol=?1 LIMIT 1")
    else {
        return String::new();
    };
    q.query_row([sym], |r| r.get(0)).unwrap_or_default()
}

fn env(k: &str, d: &str) -> String {
    std::env::var(k).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| d.to_string())
}
fn envf(k: &str, d: f64) -> f64 { env(k, "").parse().unwrap_or(d) }

// ── календар без залежностей (Хіннант): доба = ціле число від 1970-01-01 ──────
fn days_from_civil(y: i64, m: i64, d: i64) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i32
}
fn civil_from_days(z: i32) -> (i64, i64, i64) {
    let z = z as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
fn dstr(z: i32) -> String {
    let (y, m, d) = civil_from_days(z);
    format!("{y:04}-{m:02}-{d:02}")
}
fn parse_day(ts: &str) -> Option<i32> {
    if ts.len() < 10 { return None; }
    let y: i64 = ts[0..4].parse().ok()?;
    let m: i64 = ts[5..7].parse().ok()?;
    let d: i64 = ts[8..10].parse().ok()?;
    Some(days_from_civil(y, m, d))
}
fn day_from_unix(t: i64) -> i32 { t.div_euclid(86400) as i32 }

/// FNV-1a: дешевий і стабільний між прогонами (DefaultHasher такої гарантії не дає).
fn hash_row(tx: &str, w: &str, dir: &str, tok: &str, amt: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for s in [tx, w, dir, tok, amt] {
        for byte in s.as_bytes() {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn cum_series(days: &BTreeMap<i32, (f64, f64)>) -> Vec<(i32, f64)> {
    let mut v = Vec::with_capacity(days.len());
    let mut c = 0.0f64;
    for (d, (i, o)) in days { c += i - o; v.push((*d, c)); }
    v
}
fn at(v: &[(i32, f64)], day: i32) -> f64 {
    match v.binary_search_by_key(&day, |x| x.0) {
        Ok(i) => v[i].1,
        Err(0) => 0.0,
        Err(i) => v[i - 1].1,
    }
}
fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0; }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}
fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".to_string());
    let b = format!("{root}/dataset/backfill");
    let mode = env("MODE", "scan");
    let scan = mode == "scan";
    let start = parse_day(&env("BULL_START", "2021-01-01")).expect("BULL_START");
    let end = parse_day(&env("BULL_END", "2022-01-31")).expect("BULL_END");
    let min_share = envf("MIN_SHARE", 20.0);
    let only_done = env("ONLY_DONE", "1") == "1";
    let drop_contracts = env("DROP_CONTRACTS", "1") == "1";
    let post_days = envf("POST_DAYS", 90.0) as i32;
    let tag = env("TAG", "2021");
    let top: usize = envf("TOP", 400.0) as usize;

    // ── членство: кожен гаманець рівно в одній групі ──────────────────────────
    let mem: Value = serde_json::from_reader(BufReader::new(
        File::open(format!("{b}/group-membership.json")).expect("group-membership.json"),
    )).expect("json membership");
    let excl: HashSet<String> = env("EXCLUDE_GROUPS", "INFRA,NODATA")
        .split(',').map(|s| s.trim().to_string()).collect();
    let mut gname: Vec<String> = vec![];
    let mut gidx: HashMap<String, u16> = HashMap::new();
    let mut wg: HashMap<String, u16> = HashMap::with_capacity(140_000);
    for (w, v) in mem["wallets"].as_object().expect("wallets") {
        let g = v["group"].as_str().unwrap_or("?");
        if excl.contains(g) { continue; }
        let n = gname.len() as u16;
        let id = *gidx.entry(g.to_string()).or_insert_with(|| { gname.push(g.to_string()); n });
        wg.insert(w.to_ascii_lowercase(), id);
    }
    let members_total = wg.len();

    let mut dropped_kind = 0usize;
    if drop_contracts {
        if let Ok(f) = File::open(format!("{b}/wallet-kind.json")) {
            let k: Value = serde_json::from_reader(BufReader::new(f)).unwrap_or(json!({}));
            if let Some(o) = k.as_object() {
                for (a, v) in o {
                    if matches!(v.as_str().unwrap_or(""), "contract" | "exchange" | "otc")
                        && wg.remove(&a.to_ascii_lowercase()).is_some() { dropped_kind += 1; }
                }
            }
        }
        // eth_getCode — єдина надійна ознака: мітки знають лише частину адрес,
        // а контракт у складі «групи» дав би нам пул/стейкінг замість чийогось капіталу.
        if let Ok(f) = File::open(format!("{b}/contract-flags.json")) {
            let k: Value = serde_json::from_reader(BufReader::new(f)).unwrap_or(json!({}));
            if let Some(o) = k.as_object() {
                for (a, v) in o {
                    if v.as_bool() == Some(true)
                        && wg.remove(&a.to_ascii_lowercase()).is_some() { dropped_kind += 1; }
                }
            }
        }
    }

    // ── гаманці з ПОВНОЮ докачаною історією ──────────────────────────────────
    let mut last: HashMap<(String, String), String> = HashMap::new();
    if let Ok(f) = File::open(format!("{b}/_progress-grp.csv")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let f: Vec<&str> = line.split(',').collect();
            if f.len() < 4 { continue; }
            last.insert((f[0].to_ascii_lowercase(), f[1].to_string()), f[3].to_string());
        }
    }
    let (mut good, mut bad): (HashSet<String>, HashSet<String>) = (HashSet::new(), HashSet::new());
    for ((w, _), st) in &last {
        if st == "done" { good.insert(w.clone()); } else { bad.insert(w.clone()); }
    }
    let complete: HashSet<String> = good.difference(&bad).cloned().collect();
    if only_done { wg.retain(|w, _| complete.contains(w)); }
    let members_used = wg.len();

    // ── емісія (обіг), скільки знаємо ────────────────────────────────────────
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    for src in ["token-supply.json"] {
        if let Ok(f) = File::open(format!("{b}/{src}")) {
            let v: Value = serde_json::from_reader(BufReader::new(f)).unwrap_or(json!({}));
            if let Some(o) = v.as_object() {
                for (c, x) in o {
                    let s = x["supply"].as_f64().unwrap_or(0.0);
                    if s > 0.0 {
                        supply.insert(c.to_ascii_lowercase(),
                            (s, x["symbol"].as_str().unwrap_or("?").to_string()));
                    }
                }
            }
        }
    }
    if let Ok(f) = File::open(format!("{b}/tokenpnl.jsonl")) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            let d: Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
            let (Some(c), Some(s)) = (d["contract"].as_str(), d["supply"].as_f64()) else { continue };
            if s > 0.0 {
                supply.entry(c.to_ascii_lowercase())
                    .or_insert((s, d["symbol"].as_str().unwrap_or("?").to_string()));
            }
        }
    }

    // ── ціни (потрібні лише в MODE=full) ─────────────────────────────────────
    let mut prices: HashMap<String, Vec<(i32, f64)>> = HashMap::new();
    if !scan {
        let ph: Value = serde_json::from_reader(BufReader::new(
            File::open(format!("{b}/token-prices-hist.json")).expect("token-prices-hist.json"),
        )).expect("json prices");
        if let Some(o) = ph["tokens"].as_object() {
            for (c, x) in o {
                let Some(arr) = x["prices"].as_array().or_else(|| x.as_array()) else { continue };
                let mut v: Vec<(i32, f64)> = arr.iter()
                    .filter_map(|p| {
                        let a = p.as_array()?;
                        Some((day_from_unix(a[0].as_i64()?), a[1].as_f64()?))
                    })
                    .filter(|(_, p)| *p > 0.0)
                    .collect();
                v.sort_by_key(|x| x.0);
                v.dedup_by_key(|x| x.0);
                if !v.is_empty() { prices.insert(c.to_ascii_lowercase(), v); }
            }
        }
    }

    // ── потік ────────────────────────────────────────────────────────────────
    // Індекс токенів динамічний: у scan беремо ВСЕ несміттєве, у full — лише те,
    // де є і ціна, і емісія (інакше метрику розвороту порахувати нічим).
    let mut tidx: HashMap<String, u16> = HashMap::new();
    let mut tcon: Vec<String> = vec![];
    let mut tsym: Vec<String> = vec![];
    let mut acc: HashMap<(u16, u16), BTreeMap<i32, (f64, f64)>> = HashMap::new();
    // TOKENS_FILE звужує всесвіт до заданих активів і вмикає облік ПО ГАМАНЦЯХ —
    // без цього нема кого віддати codecheck на перевірку «контракт чи людина».
    let tfile = env("TOKENS_FILE", "");
    let mut only_tokens: HashSet<String> = HashSet::new();
    if !tfile.is_empty() {
        for l in BufReader::new(File::open(&tfile).expect("TOKENS_FILE")).lines().map_while(Result::ok) {
            let s = l.trim().to_ascii_lowercase();
            if s.starts_with("0x") { only_tokens.insert(s); }
        }
        eprintln!("[bullsell] всесвіт обмежено списком: {} активів", only_tokens.len());
    }
    let per_wallet = !only_tokens.is_empty();
    let mut widx: HashMap<String, u32> = HashMap::new();
    let mut waddr: Vec<String> = vec![];
    let mut wacc: HashMap<(u32, u16), BTreeMap<i32, (f64, f64)>> = HashMap::new();
    let mut seen: HashSet<u64> = HashSet::with_capacity(8_000_000);
    let (mut rows_read, mut rows_used, mut rows_internal, mut rows_dup, mut rows_spam) = (0u64, 0u64, 0u64, 0u64, 0u64);

    for name in ["transfers.csv", "transfers-grp.csv", "transfers-es.csv"] {
        let path = format!("{b}/{name}");
        // Живий файл ПЛЮС .zst-архіви: після прокрутки старша історія лежить стисненою.
        let parts = backfill::csvin::readers(&path);
        if parts.is_empty() { eprintln!("[bullsell] нема {name}"); continue }
        let mut raw: Vec<u8> = Vec::with_capacity(256);
        let (mut fr, mut fw, mut fu) = (0u64, 0u64, 0u64);
        for mut rd in parts {
        loop {
            raw.clear();
            // Читаємо БАЙТАМИ: read_line вимагає коректний UTF-8 і на першому ж непридатному
            // байті повертає помилку, а код на ній зупиняв розбір УСЬОГО файлу — мовчки.
            match rd.read_until(10, &mut raw) { Ok(0) => break, Ok(_) => {}, Err(_) => continue }
            rows_read += 1; fr += 1;
            let line = String::from_utf8_lossy(&raw);
            let s = line.trim_end();
            if s.is_empty() || s.starts_with("wallet,") { continue; }
            // Колонки: wallet,chain,dir,ts,token,symbol,counterparty,amount,tx_hash.
            // symbol — сміттєве поле (буває з комами), тому ліві 5 читаємо зліва,
            // а три останні — справа: інакше назва-спам зсуває всі колонки.
            let mut it = s.splitn(6, ',');
            let (Some(w), Some(_chain), Some(dir), Some(ts), Some(tok)) =
                (it.next(), it.next(), it.next(), it.next(), it.next()) else { continue };
            let Some(rest) = it.next() else { continue };
            let mut rit = rest.rsplitn(4, ',');
            let (Some(txh), Some(amt), Some(cp), Some(sym)) =
                (rit.next(), rit.next(), rit.next(), rit.next()) else { continue };

            let Some(day) = parse_day(ts) else { continue };
            if day > end { continue; }
            let Some(&gid) = wg.get(w) else { continue };
            fw += 1;
            if wg.get(cp) == Some(&gid) { rows_internal += 1; continue; }

            if !only_tokens.is_empty() && !only_tokens.contains(tok) { continue; }
            let tid = match tidx.get(tok) {
                Some(&t) => t,
                None => {
                    if !only_tokens.is_empty() {
                        // список заданий явно — беремо як є
                    } else if scan {
                        // сміття не індексуємо: інакше мільйони airdrop-токенів зʼїдять памʼять
                        if wr_dict::is_spam_symbol(sym) || sym.len() > 24 || sym.is_empty() {
                            rows_spam += 1; continue;
                        }
                    } else if !(prices.contains_key(tok) && supply.contains_key(tok)) {
                        continue;
                    }
                    let n = tcon.len() as u16;
                    tidx.insert(tok.to_string(), n);
                    tcon.push(tok.to_string());
                    tsym.push(sym.to_string());
                    n
                }
            };
            let Ok(a) = amt.parse::<f64>() else { continue };
            if !a.is_finite() || a <= 0.0 { continue; }
            if !seen.insert(hash_row(txh, w, dir, tok, amt)) { rows_dup += 1; continue; }

            let e = acc.entry((gid, tid)).or_default().entry(day).or_insert((0.0, 0.0));
            if dir == "i" { e.0 += a } else { e.1 += a }
            if per_wallet {
                let n = waddr.len() as u32;
                let wi = *widx.entry(w.to_string()).or_insert_with(|| { waddr.push(w.to_string()); n });
                let we = wacc.entry((wi, tid)).or_default().entry(day).or_insert((0.0, 0.0));
                if dir == "i" { we.0 += a } else { we.1 += a }
            }
            rows_used += 1; fu += 1;
        }
        }
        eprintln!("[bullsell] {name}: рядків {fr}, зі своїх гаманців {fw}, узято {fu}");
    }
    eprintln!("[bullsell] груп {} | гаманців {members_used} з {members_total} (контрактів прибрано {dropped_kind}) | токенів {} | пар {}",
        gname.len(), tcon.len(), acc.len());

    // ── SCAN: де сиділи й на яку частку емісії ───────────────────────────────
    if scan {
        let mut rows: Vec<Value> = vec![];
        for ((gid, tid), days) in &acc {
            let cum = cum_series(days);
            let mut max_bal = at(&cum, start);
            let mut max_day = start;
            for &(d, c) in cum.iter() {
                if d >= start && d <= end && c > max_bal { max_bal = c; max_day = d; }
            }
            if max_bal <= 0.0 { continue; }
            let c = &tcon[*tid as usize];
            let sup = supply.get(c).map(|x| x.0);
            let share = sup.map(|s| wr_dict::emission_pct(max_bal, s));
            // Частка поза 0..100 фізично неможлива: або знаменник не з тієї епохи, або
            // це ребейз-обгортка (aUSDT, cDAI), де баланс змінюється без переказів.
            // Позначаємо ТУТ, у місці народження числа: скан — проміжний файл, але з нього
            // читають інші кроки, і неможливе значення без позначки мандрує далі мовчки.
            let impossible = share.map(|x| !(0.0..=100.0).contains(&x)).unwrap_or(false);
            rows.push(json!({
                "group": gname[*gid as usize],
                "symbol": tsym[*tid as usize],
                "contract": c,
                "shareImpossible": impossible,
                "supply": sup,
                "maxTokens": max_bal,
                "maxDay": dstr(max_day),
                "maxSharePct": share,
                "hasPriceHist": false,
                "days": days.len()
            }));
        }
        // сортуємо: спершу відома частка (за спаданням), далі — без емісії за розміром позиції
        rows.sort_by(|a, b| {
            let (x, y) = (a["maxSharePct"].as_f64(), b["maxSharePct"].as_f64());
            match (x, y) {
                (Some(x), Some(y)) => y.partial_cmp(&x).unwrap(),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b["maxTokens"].as_f64().unwrap_or(0.0)
                    .partial_cmp(&a["maxTokens"].as_f64().unwrap_or(0.0)).unwrap(),
            }
        });
        let known: Vec<&Value> = rows.iter().filter(|r| r["maxSharePct"].as_f64().is_some()).collect();
        let over = known.iter().filter(|r| r["maxSharePct"].as_f64().unwrap() >= min_share).count();
        // контракти, яким бракує емісії або цін — на добір
        let mut need_supply: Vec<String> = vec![];
        let mut seen_c: HashSet<&str> = HashSet::new();
        for r in rows.iter().take(top * 4) {
            let c = r["contract"].as_str().unwrap();
            if r["supply"].is_null() && seen_c.insert(c) { need_supply.push(c.to_string()); }
        }
        let doc = json!({
            "generated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            "window": {"start": dstr(start), "end": dstr(end)},
            "coverage": {"groups": gname.len(), "membersConsidered": members_used, "membersTotal": members_total,
                         "tokens": tcon.len(), "pairs": acc.len(), "rowsRead": rows_read, "rowsUsed": rows_used,
                         "rowsInternalSkipped": rows_internal, "rowsSpamSkipped": rows_spam, "rowsDuplicate": rows_dup},
            "pairsWithSupply": known.len(),
            "pairsOverThreshold": over,
            "needSupply": need_supply,
            "pairs": rows.iter().take(top).collect::<Vec<_>>()
        });
        let outp = format!("{b}/bullsell-scan-{tag}.json");
        File::create(&outp).unwrap().write_all(serde_json::to_string_pretty(&doc).unwrap().as_bytes()).unwrap();
        println!("\n=== SCAN {tag}: де сиділи групи у вікні {} .. {} ===", dstr(start), dstr(end));
        println!("пар (група×актив) {} | з відомою емісією {} | з них ≥{min_share}% емісії: {over}",
            acc.len(), known.len());
        println!("{:<10} {:<12} {:>10} {:>16} {:>12}", "група", "актив", "частка%", "токенів", "день макс");
        for r in rows.iter().filter(|r| r["maxSharePct"].as_f64().unwrap_or(0.0) >= min_share).take(45) {
            println!("{:<10} {:<12} {:>10.2} {:>16.0} {:>12}",
                r["group"].as_str().unwrap(), r["symbol"].as_str().unwrap(),
                r["maxSharePct"].as_f64().unwrap_or(0.0), r["maxTokens"].as_f64().unwrap_or(0.0),
                r["maxDay"].as_str().unwrap());
        }
        println!("\nбез відомої емісії (топ за розміром позиції) — на добір supply: {}", need_supply.len());
        for r in rows.iter().filter(|r| r["supply"].is_null()).take(20) {
            println!("  {:<10} {:<12} {:>18.0} {}", r["group"].as_str().unwrap(),
                r["symbol"].as_str().unwrap(), r["maxTokens"].as_f64().unwrap_or(0.0),
                r["contract"].as_str().unwrap());
        }
        // ── списки для добору: активи (емісія+ціни) і власники (перевірка на контракт) ──
        // «Справжність» токена беремо з DefiLlama (token-prices.json): у неї не потрапляє
        // спам-емісія, тож це дешевий фільтр проти airdrop-сміття з абсурдними сумами.
        let mut real: HashSet<String> = HashSet::new();
        if let Ok(f) = File::open(format!("{b}/token-prices.json")) {
            let v: Value = serde_json::from_reader(BufReader::new(f)).unwrap_or(json!({}));
            if let Some(o) = v["tokens"].as_object() {
                for (c, _) in o { real.insert(c.to_ascii_lowercase()); }
            }
        }
        let mut cand: Vec<String> = vec![];
        let mut seen_t: HashSet<String> = HashSet::new();
        for r in rows.iter() {
            let c = r["contract"].as_str().unwrap().to_string();
            let sh = r["maxSharePct"].as_f64();
            let take = sh.map(|s| s >= min_share).unwrap_or(false) || real.contains(&c);
            if take && seen_t.insert(c.clone()) { cand.push(c); }
        }
        let tokp = format!("{b}/bullsell-tokens-{tag}.txt");
        File::create(&tokp).unwrap().write_all(cand.join("\n").as_bytes()).unwrap();
        println!("кандидатів-активів (реальні або ≥{min_share}% емісії): {} -> {tokp}", cand.len());

        if per_wallet {
            let mut hrows: Vec<Value> = vec![];
            let mut wl: HashSet<String> = HashSet::new();
            for ((wi, tid), days) in &wacc {
                let cum = cum_series(days);
                let mut mx = at(&cum, start);
                let mut md = start;
                for &(d, c) in cum.iter() {
                    if d >= start && d <= end && c > mx { mx = c; md = d; }
                }
                if mx <= 0.0 { continue; }
                let c = &tcon[*tid as usize];
                let sup = supply.get(c).map(|x| x.0);
                let addr = &waddr[*wi as usize];
                wl.insert(addr.clone());
                hrows.push(json!({
                    "wallet": addr, "group": gname[*wg.get(addr).unwrap() as usize],
                    "symbol": tsym[*tid as usize], "contract": c,
                    "maxTokens": mx, "maxDay": dstr(md),
                    "maxSharePct": sup.map(|s| wr_dict::emission_pct(mx, s))
                }));
            }
            hrows.sort_by(|a, b| b["maxTokens"].as_f64().unwrap().partial_cmp(&a["maxTokens"].as_f64().unwrap()).unwrap());
            let hp = format!("{b}/bullsell-holders-{tag}.json");
            File::create(&hp).unwrap()
                .write_all(serde_json::to_string_pretty(&json!({"holders": hrows})).unwrap().as_bytes()).unwrap();
            let wp = format!("{b}/bullsell-wallets-{tag}.txt");
            let mut wv: Vec<String> = wl.into_iter().collect();
            wv.sort();
            File::create(&wp).unwrap().write_all(wv.join("\n").as_bytes()).unwrap();
            println!("власників цих активів: {} -> {wp}\nпо-гаманцеві позиції -> {hp}", wv.len());
        }
        println!("\nJSON: {outp}");
        return;
    }

    // ── FULL: метрики розвороту ──────────────────────────────────────────────
    let price_of = |c: &str, day: i32| -> f64 {
        let p = &prices[c];
        match p.binary_search_by_key(&day, |x| x.0) {
            Ok(i) => p[i].1,
            Err(0) => p[0].1,
            Err(i) => p[i - 1].1,
        }
    };
    // Емісія НА ДАТУ розвороту (supplyat): від сьогоднішньої вона відрізняється в рази
    // там, де токен палили або довипускали, і саме це давало частки >100%.
    // (емісія, чи були мінти/спалення після дати, чи обрізало вибірку, чи індекс взагалі бачить нуль-адресу)
    let supply_at: HashMap<String, (f64, bool, bool, bool)> = File::open(format!("{b}/token-supply-at.json"))
        .ok()
        .and_then(|f| serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(f)).ok())
        .map(|m| m.into_iter().map(|(k, v)| {
            (k, (v["supply"].as_f64().unwrap_or(0.0),
                 v["events"].as_u64().unwrap_or(0) > 0,
                 v["truncated"].as_bool().unwrap_or(false),
                 v["indexed"].as_bool().unwrap_or(false)))
        }).collect())
        .unwrap_or_default();
    if !supply_at.is_empty() { eprintln!("[bullsell] емісія на дату: {} записів", supply_at.len()); }
    // Токени, чия емісія змінюється БЕЗ переказів на нульову адресу: DSToken палить власною
    // подією Burn, тож хода назад бачить «змін не було» і мовчки лишає сьогоднішній обіг.
    // MKR: сьогодні 88 тис. після конвертації в SKY, у 2021-му ~1 млн — частка завищується
    // в 11 разів. Безкоштовного архівного вузла немає (перевірено llamarpc/publicnode/ankr/
    // cloudflare), CoinGecko віддає лише 365 днів — тому такі активи виключаємо, а не гадаємо.
    // MKR шукаємо за СИМВОЛОМ у таблиці токенів, а не за зашитою адресою:
    // контракт може змінитись (SKY), символ у довіднику лишається джерелом правди.
    let mkr = token_by_symbol("MKR");
    let denom_blacklist: HashSet<String> = env("DENOM_BLACKLIST", &mkr)
        .split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    let mut peaks_out: Vec<String> = vec![];

    let mut out: Vec<Value> = vec![];
    for ((gid, tid), days) in &acc {
        let c = &tcon[*tid as usize];
        let sym = &tsym[*tid as usize];
        let Some((sup_now, _)) = supply.get(c) else { continue };
        if *sup_now <= 0.0 { continue; }
        let p = &prices[c];
        // пік ціни у вікні
        let (mut peak, mut pk) = (start, 0.0f64);
        for &(d, pr) in p.iter().filter(|(d, _)| *d >= start && *d <= end) {
            if pr > pk { pk = pr; peak = d; }
        }
        if pk <= 0.0 { continue; }
        let pdate = dstr(peak);
        peaks_out.push(format!("{c},{pdate}"));
        // один знаменник на токен — емісія на день розвороту; так частки «на максимумі»
        // і «на піку» лишаються порівнянними між собою
        let (sup, era, denom_ok) = match supply_at.get(&format!("{c}|{pdate}")) {
            // вибірку подій обрізало — відновлена емісія була б заниженою, знаменнику не віримо
            Some((_, _, true, _)) => (*sup_now, "обрізано", false),
            Some((s, changed, _, indexed)) if *s > 0.0 => {
                if *changed { (*s, "відновлена на дату", true) }
                // «подій нема» доводить сталість емісії ЛИШЕ там, де індекс ці події взагалі
                // показує. BNT і LRC віддають нуль за всю історію, хоч емісія змінювалась —
                // для них тиша означає сліпоту, а не спокій.
                else if *indexed { (*s, "без змін (індекс бачить)", true) }
                else { (*s, "індекс сліпий", false) }
            }
            _ => (*sup_now, "сьогодні", false),
        };

        let cum = cum_series(days);
        let mut max_bal = at(&cum, start);
        let mut max_day = start;
        for &(d, c2) in cum.iter() {
            if d >= start && d <= peak && c2 > max_bal { max_bal = c2; max_day = d; }
        }
        if max_bal <= 0.0 { continue; }
        let bal_peak = at(&cum, peak).max(0.0);
        let bal_post = at(&cum, peak + post_days).max(0.0);

        let max_share = wr_dict::emission_pct(max_bal, sup);
        if max_share < min_share { continue; }
        let peak_share = wr_dict::emission_pct(bal_peak, sup);
        let post_share = wr_dict::emission_pct(bal_post, sup);
        let sold_pp = (max_share - peak_share).max(0.0);
        let sold_own = if max_share > 0.0 { 100.0 * sold_pp / max_share } else { 0.0 };

        let (mut usd_pre, mut usd_post) = (0.0, 0.0);
        for (d, (i, o)) in days {
            let net_out = o - i;
            if net_out <= 0.0 { continue; }
            if *d > max_day && *d <= peak { usd_pre += net_out * price_of(c, *d); }
            if *d > peak && *d <= peak + post_days { usd_post += net_out * price_of(c, *d); }
        }
        let after = price_of(c, peak + post_days);
        let drawdown = 100.0 * (1.0 - after / pk);
        // знаменник із іншої епохи: частка >100% фізично неможлива
        // непридатне для питання «частка емісії»: неможлива частка, ребейз-обгортка
        // (баланс міняється без переказів), або знаменник не з тієї епохи
        let suspect = max_share > 100.0 || wr_dict::is_stable_or_wrapped(sym) || !denom_ok
            || denom_blacklist.contains(c)
            || sym.starts_with('a') && sym.len() > 3 && sym.chars().nth(1).unwrap_or(' ').is_uppercase()
            || sym.starts_with('c') && sym.len() > 3 && sym.chars().nth(1).unwrap_or(' ').is_uppercase();

        out.push(json!({
            "group": gname[*gid as usize], "symbol": sym, "contract": c,
            "supplyUsed": sup, "supplyEra": era, "supplyToday": sup_now,
            "supplyEraSuspect": suspect,
            "peakPriceDay": dstr(peak), "peakPrice": pk, "drawdown90Pct": drawdown,
            "maxPositionDay": dstr(max_day), "maxSharePct": max_share,
            "shareAtPeakPct": peak_share, "sharePost90Pct": post_share,
            "soldBeforeReversalPP": sold_pp, "soldBeforeReversalOwnPct": sold_own,
            "usdSoldBeforeReversal": usd_pre, "usdSoldAfterReversal": usd_post,
            "positionUsdAtMax": max_bal * price_of(c, max_day),
            "tokensAtMax": max_bal, "tokensAtPeak": bal_peak
        }));
    }
    out.sort_by(|a, b| b["maxSharePct"].as_f64().unwrap().partial_cmp(&a["maxSharePct"].as_f64().unwrap()).unwrap());

    // список «контракт,дата розвороту» — вхід для supplyat, щоб знаменник був із тієї епохи
    peaks_out.sort();
    peaks_out.dedup();
    let pkp = format!("{b}/bullsell-peaks-{tag}.txt");
    File::create(&pkp).unwrap().write_all(peaks_out.join("\n").as_bytes()).unwrap();
    eprintln!("[bullsell] дати розворотів -> {pkp} ({} активів)", peaks_out.len());

    let getf = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    let clean: Vec<&Value> = out.iter().filter(|v| !v["supplyEraSuspect"].as_bool().unwrap_or(false)).collect();
    let mut own: Vec<f64> = clean.iter().map(|v| getf(v, "soldBeforeReversalOwnPct")).collect();
    let mut pp: Vec<f64> = clean.iter().map(|v| getf(v, "soldBeforeReversalPP")).collect();
    let mut shr: Vec<f64> = clean.iter().map(|v| getf(v, "maxSharePct")).collect();
    let sum_usd_pre: f64 = clean.iter().map(|v| getf(v, "usdSoldBeforeReversal")).sum();
    let sum_usd_post: f64 = clean.iter().map(|v| getf(v, "usdSoldAfterReversal")).sum();
    let sum_pos: f64 = clean.iter().map(|v| getf(v, "positionUsdAtMax")).sum();
    let w_own = if sum_pos > 0.0 {
        clean.iter().map(|v| getf(v, "soldBeforeReversalOwnPct") * getf(v, "positionUsdAtMax")).sum::<f64>() / sum_pos
    } else { 0.0 };

    let agg = json!({
        "pairsClean": clean.len(), "pairsSuspect": out.len() - clean.len(),
        "meanSoldOwnPct": mean(&own), "medianSoldOwnPct": median(&mut own),
        "usdWeightedSoldOwnPct": w_own,
        "meanSoldPP": mean(&pp), "medianSoldPP": median(&mut pp),
        "meanMaxSharePct": mean(&shr), "medianMaxSharePct": median(&mut shr),
        "usdSoldBeforeReversalTotal": sum_usd_pre, "usdSoldAfterReversalTotal": sum_usd_post,
        "positionUsdAtMaxTotal": sum_pos,
        "pairsHeldEverything": clean.iter().filter(|v| getf(v, "soldBeforeReversalOwnPct") < 1.0).count(),
        "pairsSoldHalfOrMore": clean.iter().filter(|v| getf(v, "soldBeforeReversalOwnPct") >= 50.0).count()
    });
    let doc = json!({
        "generated": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "dictVersion": wr_dict::dict_version(),
        "window": {"start": dstr(start), "end": dstr(end), "postDays": post_days},
        "params": {"minSharePct": min_share, "onlyCompleteHistory": only_done, "dropContracts": drop_contracts},
        "coverage": {"groups": gname.len(), "membersConsidered": members_used, "membersTotal": members_total,
                     "tokens": tcon.len(), "rowsRead": rows_read, "rowsUsed": rows_used,
                     "rowsInternalSkipped": rows_internal, "rowsDuplicate": rows_dup},
        "aggregate": agg, "assets": out
    });
    let outp = format!("{b}/bullsell-{tag}.json");
    File::create(&outp).unwrap().write_all(serde_json::to_string_pretty(&doc).unwrap().as_bytes()).unwrap();

    println!("\n=== БУЛРАН {tag} — продаж емісії до розвороту ===");
    println!("вікно {} .. {}, поріг {min_share}% емісії", dstr(start), dstr(end));
    println!("гаманців {members_used} із {members_total}; пар придатних {} (сумнівний знаменник: {})",
        clean.len(), out.len() - clean.len());
    println!("\n{:<10} {:<10} {:>8} {:>9} {:>9} {:>8} {:>13} {:>9}",
        "група", "актив", "макс%ем", "напіку%", "продано", "свого%", "$ до розв", "просад%");
    for v in clean.iter().take(40) {
        println!("{:<10} {:<10} {:>8.2} {:>9.2} {:>9.2} {:>8.1} {:>13.0} {:>9.1}",
            v["group"].as_str().unwrap_or(""), v["symbol"].as_str().unwrap_or(""),
            getf(v, "maxSharePct"), getf(v, "shareAtPeakPct"), getf(v, "soldBeforeReversalPP"),
            getf(v, "soldBeforeReversalOwnPct"), getf(v, "usdSoldBeforeReversal"), getf(v, "drawdown90Pct"));
    }
    println!("\n--- середні по {} парах ---", clean.len());
    println!("продано СВОЄЇ позиції до розвороту: середнє {:.1}%, медіана {:.1}%, зважене грошима {:.1}%",
        mean(&own), median(&mut own), w_own);
    println!("у пунктах ЕМІСІЇ: середнє {:.2} п.п., медіана {:.2} п.п.", mean(&pp), median(&mut pp));
    println!("тримали частку емісії: середнє {:.1}%, медіана {:.1}%", mean(&shr), median(&mut shr));
    println!("грошей вийнято ДО розвороту ${:.0}; ПІСЛЯ (за {post_days} дн) ${:.0}", sum_usd_pre, sum_usd_post);
    println!("\nJSON: {outp}");
}
