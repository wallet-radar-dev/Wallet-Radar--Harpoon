//! Трейт патерну + РЕЄСТР. Додати патерн = mod + один рядок у registry().
use std::collections::BTreeMap;
use crate::core::ctx::Ctx;
use crate::core::PResult;

pub trait Pattern {
    /// коротка назва для CLI
    fn name(&self) -> &str;
    /// один рядок опису
    fn about(&self) -> &str;
    /// виконати; аргументи ключ=знач (напр. chains=1,42161  window_h=1  seeds=0x..,0x..)
    fn run(&self, ctx: &Ctx, args: &BTreeMap<String, String>) -> PResult;
}

mod synchro;
mod convergence;
mod funding_tree;
mod holdings;
mod pnl;
mod baltrack;
mod snapshot;
mod verify;
mod report;
mod config;
mod diff;
mod parity;
mod capexch;
mod scenario;
mod stress;
mod portfolio;
mod riskdecomp;
mod eventscan;
mod dossier;
mod journal;
mod mm;
mod cexflow;
mod mmsignal;
mod mmalert;
mod casctrack;
mod cascade;
mod cascalert;

/// ← ДОДАВАЙ НОВІ ПАТЕРНИ СЮДИ (і `mod <файл>;` вище).
pub fn registry() -> Vec<Box<dyn Pattern>> {
    vec![
        Box::new(synchro::Synchro),
        Box::new(convergence::Convergence),
        Box::new(funding_tree::FundingTree),
        Box::new(holdings::Holdings),
        Box::new(pnl::Pnl),
        Box::new(baltrack::BalTrack),
        Box::new(snapshot::Snapshot),
        Box::new(verify::Verify),
        Box::new(report::Report),
        Box::new(config::ShowConfig),
        Box::new(diff::Diff),
        Box::new(parity::Parity),
        Box::new(capexch::CapExch),
        Box::new(scenario::Scenario),
        Box::new(stress::Stress),
        Box::new(portfolio::Portfolio),
        Box::new(riskdecomp::RiskDecomp),
        Box::new(eventscan::EventScan),
        Box::new(dossier::Dossier),
        Box::new(journal::Journal),
        Box::new(mm::Mm),
        Box::new(cexflow::CexFlow),
        Box::new(mmsignal::MmSignal),
        Box::new(mmalert::MmAlert),
        Box::new(casctrack::CascTrack),
        Box::new(cascade::Cascade),
        Box::new(cascalert::CascAlert),
    ]
}

/// helper: розбір списку через кому у нижній регістр.
pub fn csv(args: &BTreeMap<String, String>, k: &str, default: &str) -> Vec<String> {
    args.get(k).map(|s| s.as_str()).unwrap_or(default)
        .split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
}
pub fn num(args: &BTreeMap<String, String>, k: &str, default: f64) -> f64 {
    args.get(k).and_then(|s| s.parse().ok()).unwrap_or(default)
}
