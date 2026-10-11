//! memberprofile — дати ПОВЕДІНКОВІ ознаки решті складу груп.
//!
//! Навіщо. Класифікатор `patdb` присвоює призначення гаманцю, але його всесвіт —
//! база патернів, а в ній 27.5 тис. адрес. У складах груп їх 127.5 тис. Тобто
//! три чверті адрес не мають підпису не тому, що вони незрозумілі, а тому, що
//! про них ніхто не рахував ознак. Цей бін рахує — і кладе в ту саму базу, щоб
//! `patdb` далі працював без змін і тими самими порогами.
//!
//! Свідомо БЕЗ цін. Ціни є для 281 токена з 306 тисяч, тож будь-яке правило на
//! доларах покриє одиниці. Натомість структура руху відома для кожного, хто
//! взагалі має перекази: скільки контрагентів, у який бік, як давно, чи є серед
//! них біржі. Цього досить для чесного підпису.
//!
//! Що кладемо в базу (усе — з прямого підрахунку, не з припущень):
//!   DORMANT       остання дія давніша за STALE днів
//!   DISTRIBUTOR   роздає багатьом: одержувачів ≥ FANOUT і їх утричі більше за джерела
//!   FUNNEL        збирає з багатьох: джерел ≥ FANIN і їх утричі більше за одержувачів
//!   MAKER         багато операцій і потік двобічний
//!   CEX-DIRECT    серед контрагентів є відома біржова адреса
//!   TRANSIT       кількість входів і виходів майже рівна при помітному обігу
//! Плюс докази: к-сть переказів, перший і останній час, контрагенти, токени.
//!
//! Args: ROOT. Env: STALE(180) FANOUT(12) FANIN(12) MIN_TX(4) DRY(1 — не писати базу)
use backfill::known::pat;
use backfill::patterns::{Finding, PatternDb};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}

#[derive(Default)]
struct Agg {
    n_in: u32,
    n_out: u32,
    first: i64,
    last: i64,
    cps_in: HashSet<u64>,
    cps_out: HashSet<u64>,
    toks: HashSet<u64>,
    cex_hits: u32,
}

/// Контрагентів і токени тримаємо як хеші, а не рядки: інакше 127 тисяч адрес
/// із сотнями контрагентів кожна не влізуть у пам'ять.
fn h64(s: &str) -> u64 {
    let mut x: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        x ^= *b as u64;
        x = x.wrapping_mul(0x100000001b3);
    }
    x
}

/// ISO-8601 у секунди. Формат у корпусі фіксований: 2026-08-14T13:41:35.000000Z
fn ts(s: &str) -> i64 {
    if s.len() < 19 {
        return 0;
    }
    let p = |a: usize, b: usize| s[a..b].parse::<i64>().unwrap_or(0);
    let (y, m, d) = (p(0, 4), p(5, 7), p(8, 10));
    let (hh, mm, ss) = (p(11, 13), p(14, 16), p(17, 19));
    if y < 2000 {
        return 0;
    }
    let mut days: i64 = 0;
    for yy in 1970..y {
        days += if (yy % 4 == 0 && yy % 100 != 0) || yy % 400 == 0 { 366 } else { 365 };
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let ml = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for i in 0..(m - 1).max(0) as usize {
        days += ml[i];
    }
    days += d - 1;
    days * 86400 + hh * 3600 + mm * 60 + ss
}

fn scan(path: &str, mem: &HashSet<String>, cex: &HashSet<u64>, agg: &mut HashMap<String, Agg>) -> u64 {
    // Живий файл ПЛЮС його .zst-архіви: після прокрутки старші рядки лежать поруч
    // стисненими, і читати лише живий файл означало б аналізувати обрізок історії.
    let parts = backfill::csvin::readers(path);
    if parts.is_empty() {
        eprintln!("[memberprofile] немає {path} — пропускаю (це НЕ означає відсутності активності)");
        return 0;
    }
    let mut rows: u64 = 0;
    for part in parts {
    for line in part.lines().map_while(Result::ok) {
        rows += 1;
        if rows % 25_000_000 == 0 {
            eprintln!("   …{} млн рядків", rows / 1_000_000);
        }
        let mut it = line.split(',');
        let (Some(w), Some(_chain), Some(dir), Some(t), Some(tok), Some(_sym), Some(cp)) =
            (it.next(), it.next(), it.next(), it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        if !mem.contains(w) {
            continue;
        }
        let e = agg.entry(w.to_string()).or_default();
        let tsec = ts(t);
        if tsec > 0 {
            if e.first == 0 || tsec < e.first {
                e.first = tsec;
            }
            e.last = e.last.max(tsec);
        }
        let ch = h64(cp);
        if dir == "i" {
            e.n_in += 1;
            e.cps_in.insert(ch);
        } else {
            e.n_out += 1;
            e.cps_out.insert(ch);
        }
        if cex.contains(&ch) {
            e.cex_hits += 1;
        }
        e.toks.insert(h64(tok));
    }
    }
    rows
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let bf = format!("{root}/dataset/backfill");
    let stale_days: i64 = envn("STALE", 180);
    let fanout: usize = envn("FANOUT", 12);
    let fanin: usize = envn("FANIN", 12);
    let min_tx: u32 = envn("MIN_TX", 4);
    let dry: u32 = envn("DRY", 0);

    // склад груп
    let mut mem: HashSet<String> = HashSet::new();
    if let Ok(f) = File::open(format!("{bf}/group-membership.json")) {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::with_capacity(1 << 20, f)) {
            if let Some(o) = v["wallets"].as_object() {
                for k in o.keys() {
                    mem.insert(k.to_lowercase());
                }
            }
        }
    }
    if mem.is_empty() {
        eprintln!("[memberprofile] склад груп порожній — нема кого профілювати");
        std::process::exit(2);
    }

    let mut db = PatternDb::load(&bf);

    // відомі біржові адреси: з бази патернів і з власної класифікації типу
    let mut cex: HashSet<u64> = HashSet::new();
    for a in mem.iter() {
        if db.has(a, pat::CEX_LABELED) || db.has(a, "CEX-DEPOSIT-ADDR") {
            cex.insert(h64(a));
        }
    }
    if let Ok(f) = File::open(format!("{bf}/wallet-kind.json")) {
        if let Ok(v) = serde_json::from_reader::<_, Value>(BufReader::new(f)) {
            if let Some(o) = v.as_object() {
                for (k, val) in o {
                    if val.as_str() == Some("exchange") {
                        cex.insert(h64(&k.to_lowercase()));
                    }
                }
            }
        }
    }
    eprintln!("[memberprofile] склад {} адрес · відомих біржових {} · у базі патернів уже {}",
        mem.len(), cex.len(), mem.iter().filter(|a| !db.pats(a).is_empty()).count());

    let mut agg: HashMap<String, Agg> = HashMap::new();
    let mut rows = 0u64;
    for f in ["transfers-grp.csv", "transfers-native.csv", "fwdindex-transfers.csv"] {
        eprintln!("[memberprofile] читаю {f}");
        rows += scan(&format!("{bf}/{f}"), &mem, &cex, &mut agg);
    }
    eprintln!("[memberprofile] рядків прочитано {rows} · адрес з активністю {}", agg.len());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut added: HashMap<&str, usize> = HashMap::new();
    for (a, g) in &agg {
        let tx = g.n_in + g.n_out;
        if tx < min_tx {
            continue;
        }
        let out_cps = g.cps_out.len();
        let in_cps = g.cps_in.len();
        let src = "memberprofile";

        // Докази кладемо завжди: навіть якщо жодне правило не спрацює, наступний
        // бін не рахуватиме те саме вдруге.
        let base = Finding::new("ACTIVITY", src)
            .ev("tx", tx as f64)
            .ev("in", g.n_in as f64)
            .ev("out", g.n_out as f64)
            .ev("cpsIn", in_cps as f64)
            .ev("cpsOut", out_cps as f64)
            .ev("tokens", g.toks.len() as f64)
            .ev("first", g.first as f64)
            .ev("last", g.last as f64);
        if dry == 0 {
            db.add(a, base);
        }
        *added.entry("ACTIVITY").or_default() += 1;

        let days_idle = if g.last > 0 { (now - g.last) / 86400 } else { 9999 };
        if days_idle > stale_days {
            if dry == 0 {
                db.add(a, Finding::new(pat::DORMANT, src).ev("idleDays", days_idle as f64));
            }
            *added.entry(pat::DORMANT).or_default() += 1;
        }
        if out_cps >= fanout && out_cps > in_cps * 3 {
            if dry == 0 {
                db.add(a, Finding::new(pat::DISTRIBUTOR, src).ev("recipients", out_cps as f64));
            }
            *added.entry(pat::DISTRIBUTOR).or_default() += 1;
        }
        if in_cps >= fanin && in_cps > out_cps * 3 {
            if dry == 0 {
                db.add(a, Finding::new(pat::FUNNEL, src).ev("sources", in_cps as f64));
            }
            *added.entry(pat::FUNNEL).or_default() += 1;
        }
        if g.cex_hits > 0 {
            if dry == 0 {
                db.add(a, Finding::new(pat::CEX_DIRECT, src).ev("cexTx", g.cex_hits as f64));
            }
            *added.entry(pat::CEX_DIRECT).or_default() += 1;
        }
        // двобічний потік при помітній активності: ознака мейкера
        let bal = if tx > 0 { (g.n_in.min(g.n_out) as f64) / (g.n_in.max(g.n_out).max(1) as f64) } else { 0.0 };
        if tx >= 40 && bal >= 0.6 {
            if dry == 0 {
                db.add(a, Finding::new(pat::MAKER, src).ev("tx", tx as f64).ev("balance", bal));
            }
            *added.entry(pat::MAKER).or_default() += 1;
        }
        // прийняв і віддав приблизно стільки ж разів — гроші проходять наскрізь
        if tx >= 10 && bal >= 0.8 && out_cps <= 3 && in_cps <= 3 {
            if dry == 0 {
                db.add(a, Finding::new(pat::TRANSIT, src).ev("tx", tx as f64));
            }
            *added.entry(pat::TRANSIT).or_default() += 1;
        }
    }

    println!("\n=== ДОДАНО ОЗНАК ===");
    let mut v: Vec<_> = added.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (p, n) in v {
        println!("  {p:<14} {n:>7}");
    }
    let no_activity = mem.len() - agg.len();
    println!("\n  адрес складу без ЖОДНОГО переказу: {no_activity} ({:.1}%)",
        100.0 * no_activity as f64 / mem.len() as f64);
    println!("  ← для них поведінкової ознаки не існує, і вигадувати її не можна");

    if dry == 0 {
        db.save(&bf);
        eprintln!("[memberprofile] базу патернів збережено");
    } else {
        eprintln!("[memberprofile] DRY=1 — базу не чіпав");
    }
}
