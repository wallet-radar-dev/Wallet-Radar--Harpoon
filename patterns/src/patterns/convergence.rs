//! ПАТЕРН convergence: приховані спільні вузли рою (збірки/координатори) з КОНТРОЛЬ-тестом.
//! Будує funding-граф із txfull, від сідів шукає не-хаб вузли, куди сходяться >=min сідів,
//! і те саме від випадкових сідів (контроль). Реальний сигнал = рій сходиться, контроль ні.
//! Аргументи: chains=42161,137  seeds=<файл>  min=4  hub_pct=0.98
use std::collections::HashSet;
use crate::core::ctx::Ctx;
use crate::core::graph::FundingGraph;
use crate::core::PResult;
use super::{Pattern, csv, num};

pub struct Convergence;

impl Pattern for Convergence {
    fn name(&self) -> &str { "convergence" }
    fn about(&self) -> &str { "приховані спільні вузли рою (збірки) з контроль-тестом" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let chains: Vec<String> = csv(args, "chains", "42161,137");
        let chains_ref: Vec<&str> = chains.iter().map(|s| s.as_str()).collect();
        let min = num(args, "min", 4.0) as usize;
        let hub_pct = num(args, "hub_pct", 0.98);
        let seeds_path = args.get("seeds").ok_or("вкажи seeds=<файл з адресами>")?;

        let seeds: HashSet<String> = std::fs::read_to_string(seeds_path)
            .map_err(|e| format!("seeds: {e}"))?
            .lines().map(|l| l.split_whitespace().next().unwrap_or("").to_lowercase())
            .filter(|s| s.starts_with("0x")).collect();
        eprintln!("[convergence] сідів {}, будую граф по {:?}...", seeds.len(), chains);

        let g = FundingGraph::build(ctx, &chains_ref);
        let hubs = g.hubs(hub_pct);
        eprintln!("[convergence] вузлів {}, хабів-виключено {}", g.adj.len(), hubs.len());

        let conv = g.convergence(&seeds, &hubs, min);
        let ctrl_seeds = g.control_seeds(&seeds, seeds.len());
        let ctrl = g.convergence(&ctrl_seeds, &hubs, min);

        println!("=== РІЙ: вузлів-сходжень (>= {min} сідів, не-хаб): {}", conv.len());
        println!("=== КОНТРОЛЬ (стільки ж випадкових): {}", ctrl.len());
        if ctrl.len() * 3 < conv.len().max(1) {
            println!(">>> СИГНАЛ РЕАЛЬНИЙ (рій сходиться, контроль майже ні)");
        } else {
            println!(">>> УВАГА: контроль зіставний — можлива графова широта, не координація");
        }
        println!("=== топ прихованих спільних вузлів:");
        for (a, n) in conv.iter().take(20) {
            let g_of = ctx.group_of(a).unwrap_or_else(|| "поза-графом".into());
            println!("  {a}  <- {n} сідів  [{g_of}]");
        }
        Ok(())
    }
}
