//! Рушій патернів пошуку координованих груп капіталу.
//! ЯК ДОДАТИ НОВИЙ ПАТЕРН (без переписування ядра):
//!   1) створити src/patterns/<моя_назва>.rs зі структурою, що impl Pattern;
//!   2) `mod <моя_назва>;` у src/patterns/mod.rs;
//!   3) додати рядок у registry() в src/patterns/mod.rs.
//! Спільні примітиви (граф, BFS, контроль, класифікація бірж, дані) — у core/*, їх патерни
//! ПЕРЕВИКОРИСТОВУЮТЬ, а не копіюють.
//!
//! Запуск:  patterns list                      — перелік патернів
//!          patterns <назва> [ключ=знач ...]    — виконати патерн з аргументами
mod core;
mod patterns;

use std::collections::BTreeMap;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let reg = patterns::registry();
    if argv.is_empty() || argv[0] == "list" || argv[0] == "--help" {
        eprintln!("Рушій патернів. Патерни ({}):", reg.len());
        for p in &reg {
            eprintln!("  {:<16} — {}", p.name(), p.about());
        }
        eprintln!("\nЗапуск: patterns <назва> [ключ=знач ...]");
        return;
    }
    let name = &argv[0];
    // аргументи ключ=знач
    let mut args: BTreeMap<String, String> = BTreeMap::new();
    for a in &argv[1..] {
        if let Some((k, v)) = a.split_once('=') {
            args.insert(k.to_string(), v.to_string());
        }
    }
    let Some(p) = reg.iter().find(|p| p.name() == name) else {
        eprintln!("невідомий патерн '{name}'. `patterns list` — перелік.");
        std::process::exit(2);
    };
    let ctx = core::ctx::Ctx::new(&args);
    match p.run(&ctx, &args) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("[{name}] помилка: {e}");
            std::process::exit(1);
        }
    }
}
