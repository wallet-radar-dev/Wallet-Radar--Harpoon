//! cexemis — чи росте ціна активу, коли група капіталу заводить емісію на біржу.
//!
//! Питання юзера: актив, де викуплено 60% емісії й вона лежить на тисячах гаманців —
//! чи починається зростання ціни, коли група ПОЧИНАЄ заводити цю емісію на біржу?
//! І від чого взагалі залежить зростання ціни? Ретро 2021-2023, більшість активів.
//!
//! Чому саме так рахується:
//!   * Обсяг заведення нормується на SUPPLY токена — тоді це «% емісії, заведений на
//!     біржу», однакова величина для великого й дрібного активу. amount у transfers.csv
//!     у людських одиницях (перевірено), тож ділення на supply коректне.
//!   * Дохідність міряється ВІДНОСНО РИНКУ (медіана по всіх токенах того дня). Без цього
//!     будь-який замір у 2021-му покаже «зростання», а в 2022-му — «падіння», і це буде
//!     властивість ринку, а не сигналу.
//!   * Разом із форвардною міряється й ЗВОРОТНА дохідність: якщо ціна росла ДО заведення,
//!     то група продає в силу, а не створює її. Це різні висновки, і плутати їх не можна.
//!
//! Вхід: transfers.csv, group-membership.json, mm-wallets.txt, wallet-patterns.jsonl (CEX),
//!       tokenpnl.jsonl (supply), token-prices-hist.json (денні ціни).
//! Вихід: dataset/backfill/cexemis.json + звіт у stderr. Args: ROOT. Env: Y0(2021) Y1(2023).
use backfill::known::pat;
use backfill::patterns::PatternDb;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

/// Номер доби від епохи для дати "YYYY-MM-DD" (алгоритм Хіннанта).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn day_of_iso(s: &str) -> Option<i64> {
    if s.len() < 10 {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: i64 = s[5..7].parse().ok()?;
    let d: i64 = s[8..10].parse().ok()?;
    Some(days_from_civil(y, m, d))
}

fn civil_of_day(mut z: i64) -> (i64, i64, i64) {
    z += 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[derive(Default, Clone, Copy)]
struct DayAgg {
    dep: f64,
    wd: f64,
    net: f64,
}

/// Одне спостереження: стан активу в день d і те, що з ціною сталося далі.
struct Obs {
    day: i64,
    year: i64,
    tok: String,
    sym: String,
    dep_pk: f64,  // % ВЛАСНОГО запасу групи, заведений на біржу за 30 днів
    wd_pk: f64,   // % власного запасу, докуплений через біржу за 30 днів
    dep30: f64,   // % емісії, заведений на біржу за 30 днів (лише де відома supply)
    wd30: f64,    // % емісії, виведений з біржі за 30 днів
    net30: f64,   // чиста зміна позиції групи, % емісії
    held: f64,    // скільки емісії група тримає зараз, %
    distr: f64,   // % роздано від піку
    mom30: f64,   // дохідність за попередні 30 днів
    fwd7: f64,
    fwd30: f64,
    fwd90: f64,
    rel30: f64, // fwd30 мінус ринок (заповнюється пізніше)
    rel90: f64,
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Медіана копії: у крипті хвости такі, що середнє описує один памп, а не вибірку.
fn med(v: &[f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut c = v.to_vec();
    c.sort_by(|a, b| a.partial_cmp(b).unwrap());
    c[c.len() / 2]
}

/// Вінзоризоване середнє: хвости підрізаються до 5/95 перцентилів, а не викидаються.
fn wmean(v: &[f64]) -> f64 {
    if v.len() < 20 {
        return med(v);
    }
    let mut c = v.to_vec();
    c.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let k = c.len() / 20;
    let (lo, hi) = (c[k], c[c.len() - 1 - k]);
    mean(&c.iter().map(|x| x.clamp(lo, hi)).collect::<Vec<f64>>())
}

/// t-статистика середнього проти нуля: без неї «середнє +3%» нічого не означає.
fn tstat(v: &[f64]) -> f64 {
    let n = v.len();
    if n < 3 {
        return 0.0;
    }
    let m = mean(v);
    let sd = (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt();
    if sd == 0.0 {
        0.0
    } else {
        m / (sd / (n as f64).sqrt())
    }
}

/// AUC (Манн-Уітні): 0.5 — монета, тобто предиктор не працює.
fn auc(pairs: &mut Vec<(f64, bool)>) -> f64 {
    let (np, nn) = (
        pairs.iter().filter(|p| p.1).count(),
        pairs.iter().filter(|p| !p.1).count(),
    );
    if np == 0 || nn == 0 {
        return 0.5;
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    // середні ранги для однакових значень, інакше маса нулів дає зсув
    let mut rank_sum = 0.0;
    let mut i = 0;
    while i < pairs.len() {
        let mut j = i;
        while j + 1 < pairs.len() && pairs[j + 1].0 == pairs[i].0 {
            j += 1;
        }
        let r = (i + j) as f64 / 2.0 + 1.0;
        for k in i..=j {
            if pairs[k].1 {
                rank_sum += r;
            }
        }
        i = j + 1;
    }
    (rank_sum - np as f64 * (np as f64 + 1.0) / 2.0) / (np as f64 * nn as f64)
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let y0: i64 = std::env::var("Y0").ok().and_then(|s| s.parse().ok()).unwrap_or(2021);
    let y1: i64 = std::env::var("Y1").ok().and_then(|s| s.parse().ok()).unwrap_or(2023);
    let d0 = days_from_civil(y0, 1, 1);
    let d1 = days_from_civil(y1, 12, 31);

    // ── кити: члени капітал-груп, без інфраструктури і ММ-churn ──────────────
    let mut whale: HashSet<String> = HashSet::new();
    if let Ok(s) = std::fs::read_to_string(format!("{bf}/group-membership.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let Some(o) = v["wallets"].as_object() {
                for (a, x) in o {
                    let g = x["group"].as_str().unwrap_or("");
                    if !g.is_empty() && g != "INFRA" && g != "OTHER" {
                        whale.insert(a.to_lowercase());
                    }
                }
            }
        }
    }
    if let Ok(s) = std::fs::read_to_string(format!("{bf}/mm-wallets.txt")) {
        for l in s.lines() {
            whale.remove(&l.trim().to_lowercase());
        }
    }
    eprintln!("[cexemis] китів (без INFRA/ММ): {}", whale.len());

    // ── біржові адреси: мітки + депозитні адреси клієнтів ────────────────────
    let db = PatternDb::load(&bf);
    let mut cex: HashSet<String> = HashSet::new();
    for (a, recs) in &db.map {
        if recs.iter().any(|r| r.pat == pat::CEX_LABELED || r.pat == "CEX-DEPOSIT-ADDR") {
            cex.insert(a.clone());
        }
    }
    eprintln!("[cexemis] біржових адрес: {}", cex.len());

    // ── supply: без нього обсяг не перевести у % емісії ──────────────────────
    let mut supply: HashMap<String, (f64, String)> = HashMap::new();
    if let Ok(f) = File::open(format!("{bf}/tokenpnl.jsonl")) {
        for line in BufReader::with_capacity(1 << 22, f).lines().map_while(Result::ok) {
            let Ok(d) = serde_json::from_str::<Value>(&line) else { continue };
            let t = d["contract"].as_str().unwrap_or("").to_lowercase();
            let s = d["supply"].as_f64().unwrap_or(0.0);
            if !t.is_empty() && s > 0.0 {
                supply.insert(t, (s, d["symbol"].as_str().unwrap_or("").to_string()));
            }
        }
    }
    eprintln!("[cexemis] токенів із supply: {}", supply.len());

    // ── денні ціни ───────────────────────────────────────────────────────────
    let mut px: HashMap<String, HashMap<i64, f64>> = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(format!("{bf}/token-prices-hist.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let Some(o) = v["tokens"].as_object() {
                for (t, x) in o {
                    let mut m: HashMap<i64, f64> = HashMap::new();
                    for p in x["prices"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                        let (Some(ts), Some(pr)) = (p[0].as_f64(), p[1].as_f64()) else { continue };
                        if pr > 0.0 {
                            m.insert((ts as i64) / 86400, pr);
                        }
                    }
                    if m.len() > 100 {
                        px.insert(t.to_lowercase(), m);
                    }
                }
            }
        }
    }
    eprintln!("[cexemis] токенів із ціновою історією: {}", px.len());

    // всесвіт: є і ціна, і supply
    // Вимога supply різала всесвіт із 246 токенів до 94 — а без неї сигнал можна
    // нормувати на власний пік позиції групи, і це навіть чесніше: «яку частку СВОГО
    // запасу вони завели на біржу» не залежить від того, чи знаємо ми емісію.
    let universe: HashSet<String> = px.keys().cloned().collect();
    eprintln!("[cexemis] всесвіт (є ціна): {} токенів, із них із supply {}",
        universe.len(), universe.iter().filter(|t| supply.contains_key(*t)).count());

    // ── один прохід по transfers.csv ─────────────────────────────────────────
    let mut agg: HashMap<String, HashMap<i64, DayAgg>> = HashMap::new();
    let mut symbols: HashMap<String, String> = HashMap::new();
    let f = File::open(format!("{bf}/transfers.csv")).expect("нема transfers.csv");
    let (mut rows, mut hits) = (0u64, 0u64);
    for line in BufReader::with_capacity(1 << 24, f).lines().map_while(Result::ok) {
        rows += 1;
        if rows % 5_000_000 == 0 {
            eprintln!("[cexemis] {rows} рядків…");
        }
        let mut it = line.splitn(9, ',');
        let w = it.next().unwrap_or("");
        if !whale.contains(w) {
            continue;
        }
        let _chain = it.next();
        let dir = it.next().unwrap_or("");
        let ts = it.next().unwrap_or("");
        let tok = it.next().unwrap_or("").to_lowercase();
        if !universe.contains(&tok) {
            continue;
        }
        let sym_csv = it.next().unwrap_or("");
        let cp = it.next().unwrap_or("");
        let amt: f64 = it.next().unwrap_or("0").parse().unwrap_or(0.0);
        if amt <= 0.0 {
            continue;
        }
        let Some(day) = day_of_iso(ts) else { continue };
        hits += 1;
        if !sym_csv.is_empty() {
            symbols.entry(tok.clone()).or_insert_with(|| sym_csv.to_string());
        }
        let e = agg.entry(tok).or_default().entry(day).or_default();
        if dir == "i" {
            e.net += amt;
            if cex.contains(cp) {
                e.wd += amt;
            }
        } else {
            e.net -= amt;
            if cex.contains(cp) {
                e.dep += amt;
            }
        }
    }
    eprintln!("[cexemis] рядків {rows}, влучень {hits}, токенів із подіями {}", agg.len());

    // ── спостереження по днях ────────────────────────────────────────────────
    let mut obs: Vec<Obs> = Vec::new();
    for (tok, days) in &agg {
        let sup = supply.get(tok).map(|x| x.0).unwrap_or(0.0);
        let sym = supply
            .get(tok)
            .map(|x| x.1.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| symbols.get(tok).cloned())
            .unwrap_or_else(|| tok[..8].to_string());
        let Some(prices) = px.get(tok) else { continue };
        let mut ds: Vec<i64> = days.keys().copied().collect();
        ds.sort();
        let (first, last) = (ds[0], *ds.last().unwrap());
        // кумулятивна позиція групи і пік — по всій історії, а не лише у вікні
        let mut cum = 0.0f64;
        let mut peak = 0.0f64;
        let mut cum_at: HashMap<i64, f64> = HashMap::new();
        let mut peak_at: HashMap<i64, f64> = HashMap::new();
        for d in first..=last {
            if let Some(a) = days.get(&d) {
                cum += a.net;
                if cum > peak {
                    peak = cum;
                }
            }
            cum_at.insert(d, cum);
            peak_at.insert(d, peak);
        }
        let p = |d: i64| -> Option<f64> {
            for k in 0..4 {
                if let Some(v) = prices.get(&(d - k)) {
                    return Some(*v);
                }
            }
            None
        };
        for d in d0.max(first)..=d1.min(last + 90) {
            let (Some(p0), Some(pm)) = (p(d), p(d - 30)) else { continue };
            let (Some(p7), Some(p30)) = (p(d + 7), p(d + 30)) else { continue };
            let p90 = p(d + 90);
            let win = |from: i64, to: i64, f: fn(&DayAgg) -> f64| -> f64 {
                (from..=to).filter_map(|x| days.get(&x)).map(f).sum::<f64>()
            };
            let pk = peak_at.get(&d).copied().unwrap_or(0.0);
            if pk <= 0.0 {
                continue; // група ще нічого не набрала — нормувати нема на що
            }
            let (dep_raw, wd_raw, net_raw) = (
                win(d - 29, d, |a| a.dep),
                win(d - 29, d, |a| a.wd),
                win(d - 29, d, |a| a.net),
            );
            // пік береться СТАНОМ НА день d, не з майбутнього — інакше це заглядання вперед
            let dep_pk = 100.0 * dep_raw / pk;
            let wd_pk = 100.0 * wd_raw / pk;
            let (dep30, wd30, net30, held) = if sup > 0.0 {
                (100.0 * dep_raw / sup, 100.0 * wd_raw / sup, 100.0 * net_raw / sup,
                 100.0 * cum_at.get(&d).copied().unwrap_or(0.0) / sup)
            } else {
                (-1.0, -1.0, -1.0, -1.0)
            };
            let distr = if pk > 0.0 {
                (100.0 * (pk - cum_at.get(&d).copied().unwrap_or(0.0)) / pk).clamp(0.0, 100.0)
            } else {
                -1.0
            };
            let (_, _) = (first, last);
            let yr = civil_of_day(d).0;
            obs.push(Obs {
                day: d,
                year: yr,
                tok: tok.clone(),
                sym: sym.clone(),
                dep_pk,
                wd_pk,
                dep30,
                wd30,
                net30,
                held,
                distr,
                mom30: p0 / pm - 1.0,
                fwd7: p7 / p0 - 1.0,
                fwd30: p30 / p0 - 1.0,
                fwd90: p90.map(|x| x / p0 - 1.0).unwrap_or(f64::NAN),
                rel30: 0.0,
                rel90: 0.0,
            });
        }
    }
    eprintln!("[cexemis] спостережень: {}", obs.len());
    if obs.is_empty() {
        eprintln!("[cexemis] нема спостережень — далі рахувати нема чого");
        return;
    }

    // ── ринок дня: медіана по всіх токенах ───────────────────────────────────
    let mut byday: HashMap<i64, Vec<f64>> = HashMap::new();
    let mut byday90: HashMap<i64, Vec<f64>> = HashMap::new();
    for o in &obs {
        byday.entry(o.day).or_default().push(o.fwd30);
        if o.fwd90.is_finite() {
            byday90.entry(o.day).or_default().push(o.fwd90);
        }
    }
    let mkt: HashMap<i64, f64> = byday.into_iter().map(|(d, mut v)| (d, median(&mut v))).collect();
    let mkt90: HashMap<i64, f64> = byday90.into_iter().map(|(d, mut v)| (d, median(&mut v))).collect();
    for o in obs.iter_mut() {
        o.rel30 = o.fwd30 - mkt.get(&o.day).copied().unwrap_or(0.0);
        o.rel90 = o.fwd90 - mkt90.get(&o.day).copied().unwrap_or(0.0);
    }

    // ── 1. подієве дослідження: заведення емісії на біржу ────────────────────
    println!("\n=== 1. ЧИ РОСТЕ ЦІНА, КОЛИ ГРУПА ЗАВОДИТЬ ЗАПАС НА БІРЖУ ({y0}-{y1}) ===");
    println!("{:>22} {:>7} {:>6} {:>10} {:>10} {:>8} {:>9} {:>9}",
        "заведено за 30дн", "n", "активів", "медіана", "вінзор.сер", "частка+", "t(активи)", "було до");
    let mut rows_json: Vec<Value> = Vec::new();
    let cuts = [
        ("не заводять (=0)", -1.0, 0.0),
        ("0-1% запасу", 0.0, 1.0),
        ("1-5%", 1.0, 5.0),
        ("5-15%", 5.0, 15.0),
        ("15-40%", 15.0, 40.0),
        ("понад 40%", 40.0, f64::INFINITY),
    ];
    for (name, lo, hi) in cuts {
        let sel: Vec<&Obs> = obs
            .iter()
            .filter(|o| if lo < 0.0 { o.dep_pk == 0.0 } else { o.dep_pk > lo && o.dep_pk <= hi })
            .collect();
        if sel.len() < 20 {
            continue;
        }
        let r30: Vec<f64> = sel.iter().map(|o| o.rel30).collect();
        let before: Vec<f64> = sel.iter().map(|o| o.mom30).collect();
        let pos = r30.iter().filter(|x| **x > 0.0).count() as f64 / r30.len() as f64;
        // Перекриті 30-денні вікна одного активу — це не незалежні спостереження.
        // Чесна значущість рахується по СЕРЕДНІХ на актив: скільки активів, стільки чисел.
        let mut per: HashMap<&str, Vec<f64>> = HashMap::new();
        for o in &sel {
            per.entry(o.tok.as_str()).or_default().push(o.rel30);
        }
        let am: Vec<f64> = per.values().map(|v| med(v)).collect();
        println!("{:>22} {:>7} {:>6} {:>9.1}% {:>9.1}% {:>7.0}% {:>9.1} {:>8.1}%",
            name, sel.len(), am.len(), 100.0 * med(&r30), 100.0 * wmean(&r30),
            100.0 * pos, tstat(&am), 100.0 * med(&before));
        rows_json.push(json!({"bucket": name, "n": sel.len(), "assets": am.len(),
            "median_rel30": med(&r30), "wmean_rel30": wmean(&r30), "hit": pos,
            "t_assets": tstat(&am), "mom30_before_median": med(&before)}));
    }

    // ── 2. те саме по роках: чи це властивість сигналу, чи ринку ─────────────
    println!("\n=== 2. ПО РОКАХ (заведення понад 5% запасу проти нульового) ===");
    println!("{:>6} {:>8} {:>10} {:>8} {:>10} {:>9}", "рік", "n(зав.)", "vs ринку", "n(нуль)", "vs ринку", "різниця");
    for y in y0..=y1 {
        let hi: Vec<f64> = obs.iter().filter(|o| o.year == y && o.dep_pk > 5.0).map(|o| o.rel30).collect();
        let zero: Vec<f64> = obs.iter().filter(|o| o.year == y && o.dep_pk == 0.0).map(|o| o.rel30).collect();
        if hi.len() < 20 || zero.len() < 20 {
            continue;
        }
        println!("{:>6} {:>8} {:>9.1}% {:>8} {:>9.1}% {:>8.1}pp",
            y, hi.len(), 100.0 * med(&hi), zero.len(), 100.0 * med(&zero),
            100.0 * (med(&hi) - med(&zero)));
    }

    // ── 3. від чого залежить зростання: порівняння предикторів ───────────────
    println!("\n=== 3. ВІД ЧОГО ЗАЛЕЖИТЬ ЗРОСТАННЯ (AUC для «обжене ринок за 30дн») ===");
    println!("{:>34} {:>7} {:>9} {:>10} {:>10}", "чинник", "AUC", "n", "верх 20%", "низ 20%");
    let factors: Vec<(&str, Box<dyn Fn(&Obs) -> f64>)> = vec![
        ("заведення на біржу, % запасу", Box::new(|o: &Obs| o.dep_pk)),
        ("виведення з біржі, % запасу", Box::new(|o: &Obs| o.wd_pk)),
        ("чистий набір позиції, % запасу", Box::new(|o: &Obs| o.net30)),
        ("роздано від піку, %", Box::new(|o: &Obs| o.distr)),
        ("ціна за попередні 30дн", Box::new(|o: &Obs| o.mom30)),
    ];
    let mut fj: Vec<Value> = Vec::new();
    for (name, f) in &factors {
        let mut pairs: Vec<(f64, bool)> = obs.iter().map(|o| (f(o), o.rel30 > 0.0)).collect();
        let a = auc(&mut pairs);
        let mut vals: Vec<(f64, f64)> = obs.iter().map(|o| (f(o), o.rel30)).collect();
        vals.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let k = vals.len() / 5;
        let top: Vec<f64> = vals[vals.len() - k..].iter().map(|x| x.1).collect();
        let bot: Vec<f64> = vals[..k].iter().map(|x| x.1).collect();
        println!("{:>34} {:>7.3} {:>9} {:>9.1}% {:>9.1}%",
            name, a, pairs.len(), 100.0 * med(&top), 100.0 * med(&bot));
        fj.push(json!({"factor": name, "auc": a, "n": pairs.len(),
            "top20_median": med(&top), "bot20_median": med(&bot)}));
    }

    // ── 4. чи справджується «60% емісії в руках» ─────────────────────────────
    println!("\n=== 4. КОНЦЕНТРАЦІЯ: чи інакше поводяться активи, де група тримає багато ===");
    println!("{:>26} {:>8} {:>10} {:>12} {:>12}", "тримають емісії", "n", "vs ринку", "при заведенні", "при наборі");
    for (name, lo, hi) in [("до 1%", 0.0, 1.0), ("1-10%", 1.0, 10.0), ("10-40%", 10.0, 40.0), ("понад 40%", 40.0, 1e9)] {
        let sel: Vec<&Obs> = obs.iter().filter(|o| o.held >= 0.0 && o.held > lo && o.held <= hi).collect();
        if sel.len() < 20 {
            continue;
        }
        let all: Vec<f64> = sel.iter().map(|o| o.rel30).collect();
        let dep: Vec<f64> = sel.iter().filter(|o| o.dep_pk > 5.0).map(|o| o.rel30).collect();
        let acc: Vec<f64> = sel.iter().filter(|o| o.wd_pk > 5.0).map(|o| o.rel30).collect();
        println!("{:>26} {:>8} {:>9.1}% {:>11.1}% {:>11.1}%",
            name, sel.len(), 100.0 * med(&all),
            if dep.len() > 10 { 100.0 * med(&dep) } else { f64::NAN },
            if acc.len() > 10 { 100.0 * med(&acc) } else { f64::NAN });
    }

    // ── 5. лід-лаг: ціна до і після ──────────────────────────────────────────
    println!("\n=== 5. ЩО БУЛО ДО І ЩО ПІСЛЯ (великі заведення, понад 15% запасу) ===");
    let big: Vec<&Obs> = obs.iter().filter(|o| o.dep_pk > 15.0).collect();
    if big.len() > 20 {
        let b: Vec<f64> = big.iter().map(|o| o.mom30).collect();
        let f7: Vec<f64> = big.iter().map(|o| o.fwd7).collect();
        let f30: Vec<f64> = big.iter().map(|o| o.rel30).collect();
        let f90: Vec<f64> = big.iter().filter(|o| o.rel90.is_finite()).map(|o| o.rel90).collect();
        let assets: HashSet<&str> = big.iter().map(|o| o.tok.as_str()).collect();
        println!("n={} на {} активах (медіани) · за 30дн ДО: {:+.1}% · далі 7дн: {:+.1}% · 30дн vs ринку: {:+.1}% · 90дн vs ринку: {:+.1}%",
            big.len(), assets.len(), 100.0 * med(&b), 100.0 * med(&f7), 100.0 * med(&f30), 100.0 * med(&f90));
    }

    // ── охоплення ────────────────────────────────────────────────────────────
    let toks: HashSet<&str> = obs.iter().map(|o| o.tok.as_str()).collect();
    let mut syms: Vec<&str> = obs.iter().map(|o| o.sym.as_str()).collect();
    syms.sort();
    syms.dedup();
    println!("\nОХОПЛЕННЯ: {} спостережень, {} активів ({}), {}-{} роки",
        obs.len(), toks.len(), syms.join(","), y0, y1);

    std::fs::write(
        format!("{bf}/cexemis.json"),
        serde_json::to_string_pretty(&json!({
            "years": [y0, y1], "observations": obs.len(), "tokens": toks.len(),
            "buckets": rows_json, "factors": fj
        })).unwrap(),
    ).ok();
    eprintln!("[cexemis] збережено cexemis.json");
}
