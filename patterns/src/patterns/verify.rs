//! ПАТЕРН verify (Частина X.2 плану): перевірки ПОВНОТИ даних.
//! Ловить обірваний краул і «порожні відповіді, що виглядають як успішні»:
//!   - txfull є, але рядків підозріло мало / нуль;
//!   - done-файл є, а txfull порожній (NOTOK-пастка: гаманець позначено done з 0 рядків);
//!   - покриття done vs очікуваного універсу.
//! Data-only, швидко, cron-ready. Аргументи: chains=1,42161,137,100,59144  universe=39169
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::fs::File;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, csv, num};

fn count_lines(path: &str) -> u64 {
    let Ok(f) = File::open(path) else { return u64::MAX }; // MAX = файлу нема
    BufReader::with_capacity(1 << 20, f).lines().count() as u64
}
fn unique_addrs(path: &str) -> u64 {
    let Ok(f) = File::open(path) else { return 0 };
    let mut s: HashSet<String> = HashSet::new();
    for l in BufReader::new(f).lines().map_while(Result::ok) {
        if let Some(a) = l.split_whitespace().next() { if a.starts_with("0x") { s.insert(a.to_lowercase()); } }
    }
    s.len() as u64
}

pub struct Verify;

impl Pattern for Verify {
    fn name(&self) -> &str { "verify" }
    fn about(&self) -> &str { "перевірки повноти даних (обірваний краул / порожні-успішні)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let chains = csv(args, "chains", "1,42161,137,100,59144");
        let universe = num(args, "universe", 39169.0) as u64;
        println!("{:<8}{:>14}{:>12}{:>10}  статус", "мережа", "рядків", "done(унік)", "покриття");
        let mut problems = 0;
        for ch in &chains {
            let rows = count_lines(&ctx.txfull(ch));
            let done = unique_addrs(&format!("{}/done-{}.txt", ctx.dir, ch));
            if rows == u64::MAX {
                println!("{ch:<8}{:>14}{done:>12}{:>10}  ФАЙЛУ НЕМА", "-", "-");
                problems += 1; continue;
            }
            let cov = if universe > 0 { done as f64 / universe as f64 * 100.0 } else { 0.0 };
            // евристики підозри
            let mut flag = "ok";
            if done > 0 && rows == 0 { flag = "!!! done є, рядків 0 (NOTOK-пастка)"; problems += 1; }
            else if done > 0 && rows < done { flag = "! рядків < done (мало даних)"; problems += 1; }
            else if cov < 50.0 { flag = "! покриття <50% (недокрауль)"; }
            println!("{ch:<8}{rows:>14}{done:>12}{cov:>9.0}%  {flag}");
        }
        // done-файли з дублями (косметика, але сигнал резюм-петель)
        println!("\n=== перевірка дублів у done-файлах:");
        for ch in &chains {
            let p = format!("{}/done-{}.txt", ctx.dir, ch);
            let total = count_lines(&p);
            if total == u64::MAX { continue }
            let uniq = unique_addrs(&p);
            if total > uniq + uniq / 10 {
                println!("  done-{ch}: {total} рядків / {uniq} унік — багато дублів (резюм-петлі)");
            }
        }
        if problems == 0 { println!("\n✅ повнота ОК: критичних проблем не знайдено"); }
        else { println!("\n⚠️  знайдено {problems} проблем повноти — див. вище"); }
        Ok(())
    }
}
