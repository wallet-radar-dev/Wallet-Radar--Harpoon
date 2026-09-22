//! rolegroups — свіжий пошук груп капіталу за РОЛЬОВОЮ моделлю. Старі біни до уваги не беремо.
//!
//! СТАДІЯ A (цей файл): по КОЖНОМУ токену з ОБСЯГОВИМ пампом (усі чейни, тижнева роздільність)
//! визначає памп-тиждень P, розкладає (гаманець×токен) на ролі та знаходить диригента.
//!   • памп = тиждень, де inflow-обсяг ≥ VOL_MULT×медіана І к-сть inflow-подій ≥ CNT_MULT×медіана
//!     (проксі «сплеску нових покупців»); rise = пік/медіана ≥ MIN_RISE; ≥ MIN_WEEKS активних тижнів.
//!   • ролі per (w,tok):  mm (features: in_ratio∈[lo,hi] & txpd≥ & days≥, БУДЬ-ЯКА адреса);
//!     інакше зайшов ДО P → dumped(out_pump/in≥DUMP_FRAC)?prepump:holder ; інакше pump.
//!   • диригент токена = EOA-фандер (нативний ETH), що газом підняв ≥MIN_REUSE холдерів цього
//!     токена; крос-активний фан-аут >MAX_FANOUT токенів = роздавач (CEX/деплоєр) → відкинути.
//! Контракти НЕ фільтруємо як членів (Q3); диригент — лише EOA (kind≠contract/exchange/otc).
//! Виходи: dataset/backfill/rolegroups-pumps.json, dataset/backfill/rolegroups-conductors.json.
//! Межа групи (N-хопи 10–15 + дерево газу + union-find) — СТАДІЯ B, окремим проходом.
//!
//! Пам'ять-ощадно: стрімінг transfers.csv у 3 проходи; позиції лише для памп-токенів;
//! газові ребра — лише в холдерів памп-токенів. Env нижче; LIMIT=N — смоук-тест на N рядків.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

fn envf(k: &str, d: f64) -> f64 { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }
fn envi(k: &str, d: i64) -> i64 { std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d) }

// ---- дата → глобальний індекс тижня (без chrono), Howard Hinnant days_from_civil ----
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
fn week_of(ts: &str) -> Option<i32> {
    let y: i64 = ts.get(0..4)?.parse().ok()?;
    let m: i64 = ts.get(5..7)?.parse().ok()?;
    let d: i64 = ts.get(8..10)?.parse().ok()?;
    Some(days_from_civil(y, m, d).div_euclid(7) as i32)
}
fn day_of(ts: &str) -> Option<i32> {
    let y: i64 = ts.get(0..4)?.parse().ok()?;
    let m: i64 = ts.get(5..7)?.parse().ok()?;
    let d: i64 = ts.get(8..10)?.parse().ok()?;
    Some(days_from_civil(y, m, d) as i32)
}
fn day_to_date(day: i32) -> String { week_to_date_impl(day as i64) }
fn week_to_date(w: i32) -> String { week_to_date_impl(w as i64 * 7) }
fn week_to_date_impl(days: i64) -> String {
    // days від епохи → ISO-дата (civil_from_days)
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// ---- інтернер адрес/токенів → u32 (ощадно на пам'ять) ----
#[derive(Default)]
struct Interner { map: HashMap<String, u32>, vec: Vec<String> }
impl Interner {
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.map.get(s) { return id; }
        let id = self.vec.len() as u32;
        self.vec.push(s.to_string());
        self.map.insert(s.to_string(), id);
        id
    }
    fn get(&self, s: &str) -> Option<u32> { self.map.get(s).copied() }
    fn name(&self, id: u32) -> &str { &self.vec[id as usize] }
}

// розбір рядка transfers.csv → (wallet, chain, dir, ts, token, symbol, counterparty, amount)
// front-поля безпечні (без ком); counterparty/amount беремо з кінця (symbol теоретично міг би мати кому)
fn parse<'a>(line: &'a str) -> Option<(&'a str, &'a str, &'a str, &'a str, &'a str, &'a str, &'a str, f64)> {
    let mut it = line.splitn(6, ',');
    let wallet = it.next()?;
    let chain = it.next()?;
    let dir = it.next()?;
    let ts = it.next()?;
    let token = it.next()?;
    let rest = it.next()?;
    if dir != "i" && dir != "o" { return None; }
    let symbol = rest.split(',').next().unwrap_or("");
    let mut rit = line.rsplitn(4, ',');
    let _hash = rit.next()?;
    let amount_s = rit.next()?;
    let counterparty = rit.next()?;
    let amount: f64 = amount_s.parse().unwrap_or(0.0);
    Some((wallet, chain, dir, ts, token, symbol, counterparty, amount))
}

// junk-адреси: нульова/мінт, EVM-precompile (0x…0000..800x), спалені (dead) — не токени й не диригенти
fn is_junk_addr(a: &str) -> bool {
    a == wr_store::addrs::zero()
        || a.starts_with("0x000000000000000000000000000000000000")  // precompile/системні (…800a нативний ETH, …8001 тощо)
        || a.contains("dead")
}

fn uf_find(uf: &mut [usize], x: usize) -> usize {
    let mut r = x; while uf[r] != r { r = uf[r]; }
    let mut c = x; while uf[c] != c { let nx = uf[c]; uf[c] = r; c = nx; }
    r
}
fn uf_union(uf: &mut [usize], a: usize, b: usize) {
    let ra = uf_find(uf, a); let rb = uf_find(uf, b);
    if ra != rb { uf[ra] = rb; }
}

fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() { return 0.0; }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let vol_mult = envf("VOL_MULT", 8.0);
    let cnt_mult = envf("CNT_MULT", 3.0);
    let min_rise = envf("MIN_RISE", 4.0);
    let min_pump_events = envi("MIN_PUMP_EVENTS", 20) as u32; // абсолютний поріг: памп-тиждень має мати ≥ N inflow-подій
    let min_cores = envi("MIN_CORES", 3) as u32;              // цікавий памп = ≥ N ядер (holder+prepump) у бекфілі
    let min_weeks = envi("MIN_WEEKS", 4) as usize;
    let pump_win = envi("PUMP_WIN", 1) as i32;          // вікно до піку для «зливу в памп»
    let dump_win_after = envi("DUMP_WIN_AFTER", 4) as i32; // вікно ПІСЛЯ піку (дампи часто трейлять)
    let dump_frac = envf("DUMP_FRAC", 0.5);
    let dump_cap = envf("DUMP_CAP", 3.0); // злив не більше ніж DUMP_CAP×нет-позиції (інакше це не «злив накопиченого», а скидання отриманого в памп)
    let mm_lo = envf("MM_INLO", 0.4);
    let mm_hi = envf("MM_INHI", 0.6);
    let mm_txpd = envf("MM_TXPD", 5.0);
    let mm_days = envf("MM_DAYS", 30.0);
    let max_fanout = envi("MAX_FANOUT", 40) as usize;
    let min_reuse = envi("MIN_REUSE", 2) as usize;
    let limit = envi("LIMIT", 0) as u64; // 0 = усе
    let tpath = format!("{root}/dataset/backfill/transfers.csv");

    let open = || BufReader::with_capacity(1 << 20, File::open(&tpath).expect("transfers.csv"));
    let mut tok_i = Interner::default();

    // ---------- ПРОХІД 1: тижневий inflow-обсяг + к-сть подій per токен ----------
    // tok_id -> week -> (in_vol, in_cnt)
    let mut wk: HashMap<u32, HashMap<i32, (f64, u32)>> = HashMap::new();
    let mut rows: u64 = 0;
    for line in open().lines().map_while(Result::ok) {
        rows += 1;
        if limit > 0 && rows > limit { break; }
        let Some((w, _chain, dir, ts, token, symbol, _cp, amt)) = parse(&line) else { continue };
        if w == "wallet" { continue; }
        if dir != "i" { continue; }              // памп = inflow-сплеск (покупки)
        if symbol == "ETH" { continue; }          // нативний газ — не памп-ціль
        if amt <= 0.0 { continue; }
        if is_junk_addr(token) { continue; }      // мінт/precompile/спалене — не токен
        let Some(week) = week_of(ts) else { continue };
        let t = tok_i.intern(token);
        let e = wk.entry(t).or_default().entry(week).or_insert((0.0, 0));
        e.0 += amt; e.1 += 1;
    }
    eprintln!("[rolegroups] П1: рядків={rows} токенів-з-inflow={}", wk.len());

    // детект памп-тижня P per токен
    // tok_id -> (P, rise)
    let mut pumps: HashMap<u32, (i32, f64)> = HashMap::new();
    for (t, weeks) in &wk {
        if weeks.len() < min_weeks { continue; }
        let mut vols: Vec<f64> = weeks.values().map(|x| x.0).collect();
        let mut cnts: Vec<f64> = weeks.values().map(|x| x.1 as f64).collect();
        let mv = median(&mut vols);
        let mc = median(&mut cnts);
        if mv <= 0.0 { continue; }
        // памп-тижні
        let mut best: Option<(i32, f64)> = None;
        for (&week, &(v, c)) in weeks.iter() {
            // відносний сплеск обсягу + відносний сплеск подій + АБСОЛЮТНИЙ поріг подій (відсікає dust/one-shot)
            if v >= vol_mult * mv && (c as f64) >= cnt_mult * mc.max(1.0) && c >= min_pump_events {
                if best.map(|(_, bv)| v > bv).unwrap_or(true) { best = Some((week, v)); }
            }
        }
        if let Some((pw, pv)) = best {
            let rise = pv / mv;
            if rise >= min_rise { pumps.insert(*t, (pw, rise)); }
        }
    }
    drop(wk);
    eprintln!("[rolegroups] П1: памп-токенів (обсяговий сплеск)={}", pumps.len());

    // ---------- допоміжні файли: kind + MM-набір ----------
    let mut kind: HashMap<String, String> = HashMap::new();
    if let Ok(v) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(
        File::open(format!("{root}/dataset/backfill/wallet-kind.json")).expect("wallet-kind.json"))) {
        for (a, k) in v { kind.insert(a.to_lowercase(), k.as_str().unwrap_or("").to_string()); }
    }
    let is_eoa = |a: &str| match kind.get(a).map(|s| s.as_str()) {
        Some("contract") | Some("exchange") | Some("otc") => false, _ => true,
    };
    // MM-набір із features.csv (БУДЬ-ЯКА адреса, не лише whale)
    let mut mm: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{root}/dataset/backfill/features.csv")) {
        for (i, l) in BufReader::new(f).lines().map_while(Result::ok).enumerate() {
            if i == 0 { continue; }
            let c: Vec<&str> = l.split(',').collect();
            if c.len() < 16 { continue; }
            let in_ratio: f64 = c[4].parse().unwrap_or(0.0);
            let active_days: f64 = c[13].parse().unwrap_or(0.0);
            let txpd: f64 = c[15].parse().unwrap_or(0.0);
            if in_ratio >= mm_lo && in_ratio <= mm_hi && txpd >= mm_txpd && active_days >= mm_days {
                mm.insert(c[0].to_lowercase());
            }
        }
    }
    eprintln!("[rolegroups] kind={} mm-набір={}", kind.len(), mm.len());

    // ---------- ПРОХІД 2: позиції (w,tok) ЛИШЕ для памп-токенів ----------
    #[derive(Default, Clone)]
    struct Pos { first_in: i32, first_in_day: i32, sum_in: f64, sum_out: f64, out_pump: f64, pre_in: f64, pre_out: f64 }
    let mut addr_i = Interner::default();
    let mut pos: HashMap<(u32, u32), Pos> = HashMap::new();
    let mut rows2: u64 = 0;
    for line in open().lines().map_while(Result::ok) {
        rows2 += 1;
        if limit > 0 && rows2 > limit { break; }
        let Some((w, _chain, dir, ts, token, symbol, _cp, amt)) = parse(&line) else { continue };
        if w == "wallet" { continue; }
        if symbol == "ETH" { continue; }
        if amt <= 0.0 { continue; }
        let Some(t) = tok_i.get(token) else { continue };
        let Some(&(pw, _)) = pumps.get(&t) else { continue };
        let Some(week) = week_of(ts) else { continue };
        let day = day_of(ts).unwrap_or(i32::MAX);
        let wid = addr_i.intern(w);
        let p = pos.entry((wid, t)).or_insert(Pos { first_in: i32::MAX, first_in_day: i32::MAX, ..Default::default() });
        if dir == "i" {
            p.sum_in += amt;
            if week < pw { p.pre_in += amt; }               // накопичення ДО пампу
            if week < p.first_in { p.first_in = week; }
            if day < p.first_in_day { p.first_in_day = day; }
        } else {
            p.sum_out += amt;
            if week < pw { p.pre_out += amt; }
            if week >= pw - pump_win && week <= pw + dump_win_after { p.out_pump += amt; } // злив у/після пампу
        }
    }
    eprintln!("[rolegroups] П2: (w,tok) позицій у памп-токенах={} адрес={}", pos.len(), addr_i.vec.len());

    // ролі + холдер-множини per токен
    // tok_id -> Vec<(wid, role, share_in)>   (share_in — груба вага для вибору диригента)
    let mut tok_holders: HashMap<u32, Vec<(u32, u8)>> = HashMap::new(); // role: 0 holder 1 prepump 2 pump 3 mm
    let mut holders_set: HashSet<u32> = HashSet::new();
    let mut census = [0u64; 4];
    // кандидати на диригента per токен: prepump-дампери з піковим pre-pump накопиченням
    let mut prepump_cand: HashMap<u32, Vec<(u32, f64)>> = HashMap::new(); // tok -> (wid, net_prepump)
    for ((wid, t), p) in &pos {
        let (pw, _) = pumps[t];
        let name = addr_i.name(*wid).to_string();
        let net_pre = (p.pre_in - p.pre_out).max(0.0);
        // дампер = мав РЕАЛЬНУ нет-позицію до пампу і злив її (DUMP_FRAC..DUMP_CAP × net_pre).
        // верхня межа відсікає тих, хто злив НАБАГАТО більше, ніж тримав до пампу (= скидали отримане в памп, не маніпулятори)
        let dumped = net_pre > 0.0 && p.out_pump >= dump_frac * net_pre && p.out_pump <= dump_cap * net_pre;
        let role: u8 = if mm.contains(&name) {
            3
        } else if p.first_in != i32::MAX && p.first_in < pw {
            if dumped { 1 } else { 0 }
        } else { 2 };
        census[role as usize] += 1;
        tok_holders.entry(*t).or_default().push((*wid, role));
        if role == 0 || role == 1 { holders_set.insert(*wid); } // ядра = ті, хто заходив до пампу
        if role == 1 && net_pre > 0.0 { prepump_cand.entry(*t).or_default().push((*wid, net_pre)); }
    }
    eprintln!("[rolegroups] ролі: holder={} prepump={} pump={} mm={}", census[0], census[1], census[2], census[3]);

    // ---------- ПРОХІД 3: газові ребра (нативний ETH) у холдерів памп-токенів ----------
    // funder_id -> set(child_id) ; child лише якщо child ∈ holders_set (ощадно)
    let mut funder_children: HashMap<u32, HashSet<u32>> = HashMap::new();
    let mut rows3: u64 = 0;
    for line in open().lines().map_while(Result::ok) {
        rows3 += 1;
        if limit > 0 && rows3 > limit { break; }
        let Some((w, _chain, dir, _ts, _token, symbol, cp, amt)) = parse(&line) else { continue };
        if w == "wallet" { continue; }
        if symbol != "ETH" || dir != "i" || amt <= 0.0 { continue; } // отримання нативного ETH = газове фінансування
        let Some(child) = addr_i.get(w) else { continue };
        if !holders_set.contains(&child) { continue; }
        let funder = addr_i.intern(cp);
        funder_children.entry(funder).or_default().insert(child);
    }
    eprintln!("[rolegroups] П3: фандерів газу (у холдерів)={}", funder_children.len());

    // ---------- диригент per токен ----------
    // ГОЛОВНИЙ сигнал (док): prepump-дампер із НАЙБІЛЬШИМ піковим pre-pump накопиченням.
    // БОНУС: він ще й газовий фандер ≥MIN_REUSE ядер цього токена (funding-tree).
    let mut token_holderset: HashMap<u32, HashSet<u32>> = HashMap::new();
    for (t, hs) in &tok_holders {
        let s: HashSet<u32> = hs.iter().filter(|(_, r)| *r == 0 || *r == 1).map(|(w, _)| *w).collect();
        token_holderset.insert(*t, s);
    }
    // conductor_of: tok -> (wid, net_prepump, has_gas_tree)
    let mut conductor_of: HashMap<u32, (u32, f64, bool)> = HashMap::new();
    for (t, cands) in &prepump_cand {
        let hset = &token_holderset[t];
        // argmax піковим накопиченням; НЕ виключаємо контракти/біржі (можуть роздавати — kind показуємо), лише junk
        let best = cands.iter().filter(|(w, _)| !is_junk_addr(addr_i.name(*w)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        if let Some(&(w, np)) = best {
            let gas = funder_children.get(&w).map(|kids| kids.intersection(hset).count() >= min_reuse).unwrap_or(false);
            conductor_of.insert(*t, (w, np, gas));
        }
    }
    // фан-аут диригента: керує >max_fanout токенами = бот/MM/роздавач → drop
    let mut fanout: HashMap<u32, usize> = HashMap::new();
    for (_t, (w, _, _)) in &conductor_of { *fanout.entry(*w).or_insert(0) += 1; }
    let dispensers: HashSet<u32> = fanout.iter().filter(|(_, n)| **n > max_fanout).map(|(w, _)| *w).collect();
    conductor_of.retain(|_, (w, _, _)| !dispensers.contains(w));
    let with_gas = conductor_of.values().filter(|(_, _, g)| *g).count();
    eprintln!("[rolegroups] диригентів (токен→акумулятор-дампер)={} унікальних={} з_газовим_деревом={} роздавачів-відкинуто={}",
        conductor_of.len(),
        conductor_of.values().map(|(w, _, _)| *w).collect::<HashSet<_>>().len(),
        with_gas, dispensers.len());

    // ---------- виходи ----------
    // 1) rolegroups-pumps.json — per памп-токен: P, rise, ролі-цензус, топ-ядра
    let mut pump_rows: Vec<Value> = Vec::new();
    for (t, &(pw, rise)) in &pumps {
        let empty = Vec::new();
        let hs = tok_holders.get(t).unwrap_or(&empty);
        if hs.is_empty() { continue; }
        let mut rc = [0u32; 4];
        for (_, r) in hs { rc[*r as usize] += 1; }
        if rc[0] + rc[1] < min_cores { continue; }   // цікавий памп = ≥ MIN_CORES ядер у бекфілі
        let cond = conductor_of.get(t).map(|(w, np, g)| json!({
            "wallet": addr_i.name(*w), "net_prepump": (np * 100.0).round() / 100.0, "has_gas_tree": g}));
        pump_rows.push(json!({
            "token": tok_i.name(*t),
            "pump_week": week_to_date(pw),
            "rise": (rise * 10.0).round() / 10.0,
            "roles": {"holder": rc[0], "prepump": rc[1], "pump": rc[2], "mm": rc[3]},
            "n_cores": rc[0] + rc[1],
            "conductor": cond,
        }));
    }
    // ранжуємо за к-стю ядер (group-релевантність), далі за rise
    pump_rows.sort_by(|a, b| b["n_cores"].as_u64().unwrap().cmp(&a["n_cores"].as_u64().unwrap())
        .then(b["rise"].as_f64().unwrap().partial_cmp(&a["rise"].as_f64().unwrap()).unwrap()));
    fs::write(format!("{root}/dataset/backfill/rolegroups-pumps.json"),
        serde_json::to_string_pretty(&json!({
            "params": {"VOL_MULT": vol_mult, "CNT_MULT": cnt_mult, "MIN_RISE": min_rise,
                       "MIN_WEEKS": min_weeks, "PUMP_WIN": pump_win, "DUMP_FRAC": dump_frac},
            "role_census": {"holder": census[0], "prepump": census[1], "pump": census[2], "mm": census[3]},
            "n_pumps": pump_rows.len(),
            "pumps": pump_rows,
        })).unwrap()).ok();

    // 2) rolegroups-conductors.json — унікальні диригенти + їхні токени + к-сть профінансованих ядер
    let mut by_cond: HashMap<u32, Vec<(String, f64, bool)>> = HashMap::new();
    for (t, (w, np, g)) in &conductor_of {
        by_cond.entry(*w).or_default().push((tok_i.name(*t).to_string(), *np, *g));
    }
    let mut cond_rows: Vec<Value> = by_cond.iter().map(|(w, toks)| {
        let mut ts = toks.clone(); ts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let n_gas = ts.iter().filter(|(_, _, g)| *g).count();
        json!({
            "conductor": addr_i.name(*w),
            "kind": kind.get(addr_i.name(*w)).cloned().unwrap_or_else(|| "eoa?".into()),
            "n_assets": ts.len(),
            "n_assets_with_gas_tree": n_gas,
            "funded_children_total": funder_children.get(w).map(|s| s.len()).unwrap_or(0),
            "assets": ts.iter().map(|(s, np, g)| json!({
                "token": s, "net_prepump": (np * 100.0).round() / 100.0, "has_gas_tree": g})).collect::<Vec<_>>(),
        })
    }).collect();
    // ранжуємо: спершу ті, що мають газове дерево (вища впевненість), далі за к-стю активів
    cond_rows.sort_by(|a, b| b["n_assets_with_gas_tree"].as_u64().unwrap().cmp(&a["n_assets_with_gas_tree"].as_u64().unwrap())
        .then(b["n_assets"].as_u64().unwrap().cmp(&a["n_assets"].as_u64().unwrap())));
    fs::write(format!("{root}/dataset/backfill/rolegroups-conductors.json"),
        serde_json::to_string_pretty(&json!({"n_conductors": cond_rows.len(), "conductors": cond_rows})).unwrap()).ok();

    // ================= СТАДІЯ B: ГРУПИ КАПІТАЛУ (union-find) =================
    // Ядра = pre-pump учасники (holder+prepump). Лінк: (а) співволодіння РІДКІСНИМ токеном
    // (≤MAX_CORES_LINK ядер → координація, IDF-логіка), (б) газове ребро funder↔child.
    // Диригент групи = ядро з макс піковим pre-pump накопиченням. Члени = ядра + газові діти.
    // N-хопи 10–15 (транзакційний граф) — наступний інкремент. Частка емісії — де є supply.
    let do_groups = envi("GROUPS", 1) != 0;
    if do_groups {
        let max_cores_link = envi("MAX_CORES_LINK", 30) as usize;
        let min_shared = envi("MIN_SHARED", 2) as u16; // ≥N спільних рідкісних токенів = координація (проти гігант-компоненти)
        let min_group_cores = envi("MIN_GROUP_CORES", 3) as usize;

        // supply (mainnet) — для частки емісії (де є)
        let mut supply: HashMap<String, (f64, String)> = HashMap::new();
        if let Ok(sv) = serde_json::from_reader::<_, serde_json::Map<String, Value>>(BufReader::new(
            File::open(format!("{root}/dataset/token-supply.json")).unwrap())) {
            for (c, v) in &sv {
                let dec = v["decimals"].as_u64().unwrap_or(18) as i32;
                let raw: f64 = v["supply"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                if raw > 0.0 { supply.insert(c.to_lowercase(), (raw / 10f64.powi(dec), v["symbol"].as_str().unwrap_or("").to_string())); }
            }
        }
        // ціни + символи токенів (остання) : token(lower) -> (price, decimals) ; token -> symbol (покриває L2)
        let mut price: HashMap<String, (f64, i32)> = HashMap::new();
        let mut psym: HashMap<String, String> = HashMap::new();
        if let Ok(rd) = fs::read_dir(format!("{root}/dataset/nextaction/prices")) {
            for e in rd.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                let Some(stem) = fname.strip_suffix(".json") else { continue };
                let Some((_chain, tok)) = stem.split_once('_') else { continue };
                let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(match File::open(e.path()) { Ok(f) => f, Err(_) => continue })) else { continue };
                let dec = v["decimals"].as_i64().unwrap_or(18) as i32;
                if let Some(s) = v["symbol"].as_str() { psym.entry(tok.to_lowercase()).or_insert_with(|| s.to_string()); }
                if let Some(last) = v["prices"].as_array().and_then(|a| a.last()) {
                    let p = last["price"].as_f64().unwrap_or(0.0);
                    if p > 0.0 { price.entry(tok.to_lowercase()).or_insert((p, dec)); }
                }
            }
        }
        eprintln!("[rolegroups] цін токенів завантажено: {} (символів: {})", price.len(), psym.len());

        // компактний індекс ядер
        let cores: Vec<u32> = holders_set.iter().copied().collect();
        let mut idx: HashMap<u32, usize> = HashMap::new();
        for (i, &w) in cores.iter().enumerate() { idx.insert(w, i); }
        let n = cores.len();
        let _ = &idx; // idx лишаємо для довідки; union-find тепер ПО ДИРИГЕНТАХ (нижче, після графа)

        // ---- N-хоп транзакційний граф (усі адреси), hub-pruned — для розширення членства ----
        let hops = envi("HOPS", 12) as u32;
        let hub_deg = envi("HUB_DEG", 500) as u32;
        let max_group_members = envi("MAX_GROUP_MEMBERS", 200000) as usize;
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut deg: HashMap<u32, u32> = HashMap::new();
        if hops > 0 {
            // П4: ступені вузлів (інтернуємо ВСІ адреси)
            let mut r = 0u64;
            for line in open().lines().map_while(Result::ok) {
                r += 1; if limit > 0 && r > limit { break; }
                let Some((w, _c, _d, _ts, _tok, _sym, cp, amt)) = parse(&line) else { continue };
                if w == "wallet" || amt <= 0.0 || is_junk_addr(w) || is_junk_addr(cp) { continue; }
                let wi = addr_i.intern(w); let ci = addr_i.intern(cp);
                if wi == ci { continue; }
                *deg.entry(wi).or_insert(0) += 1; *deg.entry(ci).or_insert(0) += 1;
            }
            // П5: повна суміжність (хаби ВКЛЮЧАЄМО як вузли — біржа може бути джерелом; не протягуємо крізь них у BFS)
            let mut r2 = 0u64;
            for line in open().lines().map_while(Result::ok) {
                r2 += 1; if limit > 0 && r2 > limit { break; }
                let Some((w, _c, _d, _ts, _tok, _sym, cp, amt)) = parse(&line) else { continue };
                if w == "wallet" || amt <= 0.0 { continue; }
                let (Some(wi), Some(ci)) = (addr_i.get(w), addr_i.get(cp)) else { continue };
                if wi == ci { continue; }
                adj.entry(wi).or_default().push(ci);
                adj.entry(ci).or_default().push(wi);
            }
            for v in adj.values_mut() { v.sort_unstable(); v.dedup(); }
            let hubs = deg.values().filter(|d| **d > hub_deg).count();
            eprintln!("[rolegroups] граф: вузлів={} ребер-вузлів hub(>{}) = {} (не протягуємо крізь них), hops={}",
                adj.len(), hub_deg, hubs, hops);
        }

        // wid -> макс net_prepump (для вибору диригента групи)
        let mut wid_score: HashMap<u32, f64> = HashMap::new();
        for (_t, cands) in &prepump_cand {
            for (w, np) in cands { let e = wid_score.entry(*w).or_insert(0.0); if *np > *e { *e = *np; } }
        }
        // wid -> токени, де воно ядро
        let mut wid_tokens: HashMap<u32, HashSet<u32>> = HashMap::new();
        for (t, hs) in &tok_holders {
            for (w, r) in hs { if *r == 0 || *r == 1 { wid_tokens.entry(*w).or_default().insert(*t); } }
        }
        // wid, що були prepump-дамперами (для розкладу ролей групи)
        let prepump_wids: HashSet<u32> = prepump_cand.values().flat_map(|v| v.iter().map(|(w, _)| *w)).collect();

        // ---- ОПЕРАЦІЯ = ОДИН ТОКЕН: кожне ядро приписуємо до ПЕРВИННОГО токена (де найбільше накопичило ДО пампу) ----
        let min_members = envi("MIN_MEMBERS", 50) as usize;
        let mut wid_primary: HashMap<u32, (u32, f64)> = HashMap::new(); // wid -> (token, pre_in)
        for ((w, t), p) in &pos {
            if !holders_set.contains(w) { continue; }
            let e = wid_primary.entry(*w).or_insert((*t, -1.0));
            if p.pre_in > e.1 { *e = (*t, p.pre_in); }
        }
        eprintln!("[rolegroups] ядер із первинним токеном: {}", wid_primary.len());

        // ---- КОНКУРЕНТНИЙ multi-source BFS: сід = ядра, мітка = їхній первинний ТОКЕН; гаманець → найближче ядро ----
        let mut claim: HashMap<u32, u32> = HashMap::new(); // wallet -> token(op)
        {
            let mut frontier: Vec<u32> = Vec::new();
            for &w in &cores {
                if let Some(&(pt, _)) = wid_primary.get(&w) { claim.insert(w, pt); frontier.push(w); }
            }
            for _ in 0..hops {
                if frontier.is_empty() { break; }
                let mut next: Vec<u32> = Vec::new();
                for &u in &frontier {
                    let op = claim[&u];
                    if let Some(ns) = adj.get(&u) {
                        for &v in ns {
                            if !claim.contains_key(&v) {
                                claim.insert(v, op);
                                if deg.get(&v).copied().unwrap_or(0) <= hub_deg { next.push(v); } // не протягуємо крізь хаб
                            }
                        }
                    }
                }
                frontier = next;
            }
        }
        eprintln!("[rolegroups] конкурентний BFS (per-токен): охоплено гаманців={}", claim.len());

        // ---- члени per-токен (BFS) + ЗЛИТТЯ токенів за СПІЛЬНИМИ ЯДРАМИ (серійні оператори) ----
        let mut op_members: HashMap<u32, usize> = HashMap::new();
        for op in claim.values() { *op_members.entry(*op).or_insert(0) += 1; }
        let merge_shared = envi("MERGE_SHARED", 2) as u16;         // ≥N спільних специфічних ядер → одна операція
        let max_core_tokens = envi("MAX_CORE_TOKENS", 12) as usize; // ядро в >N памп-токенах = ринковий кит, не лінк (IDF)
        let min_op_cores = envi("MIN_OP_CORES", 3) as usize;
        // token -> усі ядра (holder+prepump); ядро може бути у кількох токенах
        let mut token_cores: HashMap<u32, Vec<u32>> = HashMap::new();
        for (t, hs) in &tok_holders {
            let cs: Vec<u32> = hs.iter().filter(|(_, r)| *r == 0 || *r == 1).map(|(w, _)| *w).collect();
            if !cs.is_empty() { token_cores.insert(*t, cs); }
        }
        // core -> токени (для IDF)
        let mut core_tokens: HashMap<u32, Vec<u32>> = HashMap::new();
        for (t, cs) in &token_cores { for w in cs { core_tokens.entry(*w).or_default().push(*t); } }
        let toks: Vec<u32> = token_cores.keys().copied().collect();
        let mut tidx: HashMap<u32, usize> = HashMap::new();
        for (i, &t) in toks.iter().enumerate() { tidx.insert(t, i); }
        let mut tuf: Vec<usize> = (0..toks.len()).collect();
        // пари токенів, що ділять СПЕЦИФІЧНЕ ядро (ядро у ≤max_core_tokens токенах)
        let mut tpair: HashMap<(u32, u32), u16> = HashMap::new();
        for (_w, ts) in &core_tokens {
            if ts.len() < 2 || ts.len() > max_core_tokens { continue; }
            for i in 0..ts.len() {
                for j in i + 1..ts.len() {
                    let a = tidx[&ts[i]].min(tidx[&ts[j]]) as u32;
                    let b = tidx[&ts[i]].max(tidx[&ts[j]]) as u32;
                    *tpair.entry((a, b)).or_insert(0) += 1;
                }
            }
        }
        let mut merges = 0u64;
        for ((a, b), c) in &tpair { if *c >= merge_shared { uf_union(&mut tuf, *a as usize, *b as usize); merges += 1; } }
        drop(tpair);
        let mut comp: HashMap<usize, Vec<u32>> = HashMap::new();
        for (i, &t) in toks.iter().enumerate() { comp.entry(uf_find(&mut tuf, i)).or_default().push(t); }
        eprintln!("[rolegroups] злиття: {} токенів → {} операцій (лінків токен-пар={})", toks.len(), comp.len(), merges);

        // ---- збірка ЗЛИТИХ груп (операція = кластер токенів + спільні оператори) ----
        let mut groups: Vec<Value> = Vec::new();
        for (_root, gtokens) in &comp {
            let mut gcores: HashSet<u32> = HashSet::new();
            for t in gtokens { if let Some(cs) = token_cores.get(t) { for w in cs { gcores.insert(*w); } } }
            if gcores.len() < min_op_cores { continue; }
            let nmem: usize = gtokens.iter().map(|t| op_members.get(t).copied().unwrap_or(0)).sum();
            let leader = gcores.iter().copied().max_by(|a, b| wid_score.get(a).copied().unwrap_or(0.0).partial_cmp(&wid_score.get(b).copied().unwrap_or(0.0)).unwrap());
            let (mut rp, mut rh, mut rm) = (0u32, 0u32, 0u32);
            for w in &gcores { let name = addr_i.name(*w); if mm.contains(name) { rm += 1; } else if prepump_wids.contains(w) { rp += 1; } else { rh += 1; } }
            let mut assets: Vec<Value> = gtokens.iter().map(|t| {
                let tname = tok_i.name(*t);
                let cs = token_cores.get(t).cloned().unwrap_or_default();
                let mut held = 0.0;
                for w in &cs { if let Some(p) = pos.get(&(*w, *t)) { held += (p.sum_in - p.sum_out).max(0.0); } }
                let (emis, sym) = if let Some((hs_sup, sym)) = supply.get(tname) { (((held / hs_sup).min(1.0) * 1e6).round() / 1e6, sym.clone()) } else { (-1.0, String::new()) };
                let (pw, rise) = pumps.get(t).copied().unwrap_or((0, 0.0));
                json!({"token": tname, "symbol": sym, "n_cores": cs.len(), "pump_week": week_to_date(pw), "rise": (rise * 10.0).round() / 10.0, "emission_share": emis})
            }).collect();
            assets.sort_by(|a, b| b["n_cores"].as_u64().unwrap().cmp(&a["n_cores"].as_u64().unwrap()));
            let max_emis = assets.iter().filter_map(|a| { let s = a["emission_share"].as_f64().unwrap(); if s >= 0.0 { Some(s) } else { None } }).fold(0.0f64, f64::max);
            let mut topc: Vec<&u32> = gcores.iter().collect();
            topc.sort_by(|a, b| wid_score.get(*b).copied().unwrap_or(0.0).partial_cmp(&wid_score.get(*a).copied().unwrap_or(0.0)).unwrap());
            groups.push(json!({
                "n_assets": gtokens.len(), "n_cores": gcores.len(), "n_members": nmem,
                "roles": {"prepump_dumpers": rp, "holders": rh, "mm": rm},
                "conductor": leader.map(|w| addr_i.name(w)),
                "conductor_kind": leader.map(|w| kind.get(addr_i.name(w)).cloned().unwrap_or_else(|| "eoa?".into())),
                "max_emission_share": (max_emis * 1e6).round() / 1e6,
                "assets": assets.into_iter().take(20).collect::<Vec<_>>(),
                "top_cores": topc.iter().take(15).map(|w| addr_i.name(**w)).collect::<Vec<_>>(),
            }));
        }
        // ранжуємо за к-стю активів (розмах операції), далі за ядрами
        groups.sort_by(|a, b| b["n_assets"].as_u64().unwrap().cmp(&a["n_assets"].as_u64().unwrap())
            .then(b["n_cores"].as_u64().unwrap().cmp(&a["n_cores"].as_u64().unwrap())));
        for (i, g) in groups.iter_mut().enumerate() { g["id"] = json!(i + 1); }
        fs::write(format!("{root}/dataset/backfill/rolegroups-groups.json"),
            serde_json::to_string_pretty(&json!({
                "params": {"grouping": "merged-by-shared-cores", "MERGE_SHARED": merge_shared, "MAX_CORE_TOKENS": max_core_tokens, "MIN_OP_CORES": min_op_cores, "HOPS": hops},
                "n_groups": groups.len(), "wallets_covered": claim.len(),
                "groups": groups,
            })).unwrap()).ok();
        eprintln!("[rolegroups] СТАДІЯ B (злиті): операцій(≥{min_op_cores} ядер)={}; охоплено гаманців={}", groups.len(), claim.len());

        eprintln!("\n=== ТОП-25 ОПЕРАЦІЙ (злиті за спільними ядрами; серійні оператори) ===");
        eprintln!("{:>4} {:>7} {:>6} {:>8} {:>11} {:>9}  топ-актив  leader (kind)", "id", "активів", "ядер", "членів", "pp/hold/mm", "maxEmis%");
        for g in groups.iter().take(25) {
            let ro = &g["roles"];
            let top_asset = g["assets"][0]["symbol"].as_str().unwrap_or("?");
            eprintln!("{:>4} {:>7} {:>6} {:>8} {:>3}/{:<3}/{:<3} {:>8.2}%  {:<9} {} ({})",
                g["id"].as_u64().unwrap(), g["n_assets"].as_u64().unwrap(), g["n_cores"].as_u64().unwrap(),
                g["n_members"].as_u64().unwrap(),
                ro["prepump_dumpers"].as_u64().unwrap(), ro["holders"].as_u64().unwrap(), ro["mm"].as_u64().unwrap(),
                g["max_emission_share"].as_f64().unwrap() * 100.0, top_asset,
                g["conductor"].as_str().unwrap_or("-"), g["conductor_kind"].as_str().unwrap_or("-"));
        }

        // ================= ПІДХІД #2: ТЕМПОРАЛЬНА СИНХРОННІСТЬ =================
        // Координація = гаманці, що РАЗ-У-РАЗ заходять в один памп-токен у той самий ТИЖДЕНЬ (до пампу).
        // Стійке до біржового шуму: біржі не синхронізують входи. Виключаємо exchange/otc + ubiquitous-інфру (IDF).
        let max_bucket = envi("MAX_BUCKET", 15) as usize;   // когорта >N входів у вікні = не координація (масовий вхід)
        let min_cosync = envi("MIN_COSYNC", 3) as u16;      // ≥N разів co-entry в те саме вікно → лінк
        let min_group = envi("MIN_GROUP", 3) as usize;
        let sync_days = envi("SYNC_DAYS", 1).max(1) as i32; // ширина вікна синхронності В ДНЯХ (1 = той самий день)
        let net_hops = envi("NET_HOPS", 4) as u32;          // розгортання координованого сіду транзакційною мережею
        // eligible: ядро у 2..MAX_CORE_TOKENS токенах (серійне, не одноразове й не ubiquitous), не біржа
        let eligible: HashSet<u32> = core_tokens.iter()
            .filter(|(w, ts)| {
                let n = ts.len();
                n >= 2 && n <= max_core_tokens
                    && !matches!(kind.get(addr_i.name(**w)).map(|s| s.as_str()), Some("exchange") | Some("otc"))
            })
            .map(|(w, _)| *w).collect();
        // стейбли/обгортки — co-entry в них НЕ координація (усі тримають USDT); виключаємо
        let is_stable = |sym: &str| {
            let s = sym.to_uppercase();
            s.contains("USD") || matches!(s.as_str(), "DAI" | "FRAX" | "MIM" | "EURC" | "EURS" | "GUSD" | "LUSD" | "USDE")
        };
        let stable_toks: HashSet<u32> = pos.keys().map(|(_, t)| *t).collect::<HashSet<_>>().into_iter()
            .filter(|t| {
                let name = tok_i.name(*t);
                supply.get(name).map(|(_, s)| is_stable(s)).unwrap_or(false)
                    || psym.get(name).map(|s| is_stable(s)).unwrap_or(false)
            }).collect();
        // bucket (token, вікно-ДНІВ першого входу) -> eligible-гаманці, ЛИШЕ pre-pump, БЕЗ стейблів
        let mut buckets: HashMap<(u32, i32), Vec<u32>> = HashMap::new();
        for ((w, t), p) in &pos {
            if !eligible.contains(w) || p.first_in == i32::MAX || p.first_in_day == i32::MAX || stable_toks.contains(t) { continue; }
            if let Some(&(pw, _)) = pumps.get(t) { if p.first_in < pw { buckets.entry((*t, p.first_in_day.div_euclid(sync_days))).or_default().push(*w); } }
        }
        eprintln!("[rolegroups] ТЕМПОРАЛЬНА: виключено стейбл-токенів={}", stable_toks.len());
        // попарна co-sync (лише когорти розміру 2..MAX_BUCKET — рідкісна синхронність)
        let mut cosync: HashMap<(u32, u32), u16> = HashMap::new();
        for ws in buckets.values() {
            if ws.len() < 2 || ws.len() > max_bucket { continue; }
            for i in 0..ws.len() {
                for j in i + 1..ws.len() {
                    let a = ws[i].min(ws[j]); let b = ws[i].max(ws[j]);
                    *cosync.entry((a, b)).or_insert(0) += 1;
                }
            }
        }
        // union-find по eligible-гаманцях
        let ev: Vec<u32> = eligible.iter().copied().collect();
        let mut eidx: HashMap<u32, usize> = HashMap::new();
        for (i, &w) in ev.iter().enumerate() { eidx.insert(w, i); }
        let mut euf: Vec<usize> = (0..ev.len()).collect();
        let mut colinks = 0u64;
        for ((a, b), c) in &cosync { if *c >= min_cosync { uf_union(&mut euf, eidx[a], eidx[b]); colinks += 1; } }
        let mut ccomp: HashMap<usize, Vec<u32>> = HashMap::new();
        for (i, &w) in ev.iter().enumerate() { ccomp.entry(uf_find(&mut euf, i)).or_default().push(w); }
        let mut coord: Vec<Vec<u32>> = ccomp.into_values().filter(|v| v.len() >= min_group).collect();
        coord.sort_by(|a, b| b.len().cmp(&a.len()));
        eprintln!("[rolegroups] ТЕМПОРАЛЬНА: eligible={} co-sync-лінків(≥{})={} координованих груп(≥{})={}",
            ev.len(), min_cosync, colinks, min_group, coord.len());

        // збірка координованих груп
        let mut cgroups: Vec<Value> = Vec::new();
        for members in &coord {
            let mset: HashSet<u32> = members.iter().copied().collect();
            // спільні токени, де ≥2 членів co-entered той самий тиждень
            let mut shared_tokens: HashMap<u32, u32> = HashMap::new();
            for ((t, _wk), ws) in &buckets {
                let c = ws.iter().filter(|w| mset.contains(w)).count();
                if c >= 2 { *shared_tokens.entry(*t).or_insert(0) = (*shared_tokens.get(t).unwrap_or(&0)).max(c as u32); }
            }
            let mut toks_v: Vec<(u32, u32)> = shared_tokens.into_iter().collect();
            toks_v.sort_by(|a, b| b.1.cmp(&a.1));
            let leader = members.iter().copied().max_by(|a, b| wid_score.get(a).copied().unwrap_or(0.0).partial_cmp(&wid_score.get(b).copied().unwrap_or(0.0)).unwrap());
            let (mut rp, mut rh) = (0u32, 0u32);
            for w in members { if prepump_wids.contains(w) { rp += 1; } else { rh += 1; } }
            // РОЗГОРТАННЯ ТРАНЗАКЦІЙНОЮ МЕРЕЖЕЮ: hub-pruned BFS від синхро-сіду
            let mut net: HashSet<u32> = members.iter().copied().collect();
            {
                let mut frontier: Vec<u32> = members.clone();
                for _ in 0..net_hops {
                    let mut nx: Vec<u32> = Vec::new();
                    for &u in &frontier {
                        if let Some(ns) = adj.get(&u) {
                            for &v in ns { if net.insert(v) && deg.get(&v).copied().unwrap_or(0) <= hub_deg { nx.push(v); } }
                        }
                    }
                    if nx.is_empty() { break; }
                    frontier = nx;
                }
            }
            let n_network = net.len() - members.len();
            cgroups.push(json!({
                "n_members": members.len(),
                "n_network": n_network,
                "n_shared_tokens": toks_v.len(),
                "roles": {"prepump_dumpers": rp, "holders_only": rh},
                "leader": leader.map(|w| addr_i.name(w)),
                "leader_kind": leader.map(|w| kind.get(addr_i.name(w)).cloned().unwrap_or_else(|| "eoa?".into())),
                "shared_tokens": toks_v.iter().take(15).map(|(t, c)| json!({"symbol": supply.get(tok_i.name(*t)).map(|(_, s)| s.clone()).unwrap_or_default(), "token": tok_i.name(*t), "cosync_wallets": c})).collect::<Vec<_>>(),
                "members": members.iter().take(25).map(|w| addr_i.name(*w)).collect::<Vec<_>>(),
            }));
        }
        for (i, g) in cgroups.iter_mut().enumerate() { g["id"] = json!(i + 1); }
        fs::write(format!("{root}/dataset/backfill/rolegroups-coordinated.json"),
            serde_json::to_string_pretty(&json!({
                "params": {"MAX_BUCKET": max_bucket, "MIN_COSYNC": min_cosync, "MIN_GROUP": min_group, "MAX_CORE_TOKENS": max_core_tokens, "SYNC_DAYS": sync_days, "NET_HOPS": net_hops},
                "n_groups": cgroups.len(), "eligible_wallets": ev.len(),
                "groups": cgroups,
            })).unwrap()).ok();
        eprintln!("\n=== ТОП-20 КООРДИНОВАНИХ ГРУП (синхронність {}-денна; +мережа) ===", sync_days);
        eprintln!("{:>4} {:>6} {:>8} {:>7}  спільні-токени               leader (kind)", "id", "сід", "мережа", "спіл.ток");
        for g in cgroups.iter().take(20) {
            let toks: Vec<String> = g["shared_tokens"].as_array().unwrap().iter().take(4)
                .map(|x| x["symbol"].as_str().filter(|s| !s.is_empty()).unwrap_or_else(|| &x["token"].as_str().unwrap()[..8]).to_string()).collect();
            eprintln!("{:>4} {:>6} {:>8} {:>7}  {:<28}  {} ({})",
                g["id"].as_u64().unwrap(), g["n_members"].as_u64().unwrap(), g["n_network"].as_u64().unwrap(),
                g["n_shared_tokens"].as_u64().unwrap(),
                toks.join(","), g["leader"].as_str().unwrap_or("-"), g["leader_kind"].as_str().unwrap_or("-"));
        }
    }

    // ---------- звіт ----------
    eprintln!("\n=== ТОП-25 ПАМП-ТОКЕНІВ за rise (обсяговий сплеск) ===");
    eprintln!("{:<44} {:>10} {:>6} {:>4}/{:<4}/{:<4}/{:<3} conductor", "token", "pump_wk", "rise", "H", "P", "pu", "mm");
    for r in pump_rows.iter().take(25) {
        let ro = &r["roles"];
        eprintln!("{:<44} {:>10} {:>5.1}x {:>4}/{:<4}/{:<4}/{:<3} {}",
            r["token"].as_str().unwrap(), r["pump_week"].as_str().unwrap(), r["rise"].as_f64().unwrap(),
            ro["holder"].as_u64().unwrap(), ro["prepump"].as_u64().unwrap(),
            ro["pump"].as_u64().unwrap(), ro["mm"].as_u64().unwrap(),
            r["conductor"].as_object().map(|c| c["wallet"].as_str().unwrap()).unwrap_or("-"));
    }
    eprintln!("\n=== ТОП-15 ДИРИГЕНТІВ (акумулятор-дампер; +газове дерево = вища впевненість) ===");
    for r in cond_rows.iter().take(15) {
        eprintln!("  {} assets={:>3} з_газом={:>2} funded_children={:>4} kind={}",
            r["conductor"].as_str().unwrap(), r["n_assets"].as_u64().unwrap(),
            r["n_assets_with_gas_tree"].as_u64().unwrap(),
            r["funded_children_total"].as_u64().unwrap(), r["kind"].as_str().unwrap());
    }
    eprintln!("\nвиходи: rolegroups-pumps.json ({} пампів) | rolegroups-conductors.json ({} диригентів)",
        pump_rows.len(), cond_rows.len());
}
