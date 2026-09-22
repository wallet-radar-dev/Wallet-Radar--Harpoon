//! ПАТЕРН pnl: реалізований P&L = НЕТТО реальних грошей (native + справжні стейбли/WETH ЗА КОНТРАКТОМ).
//! Нетто (in-out) скасовує валові churn-ноги, лишаючи реалізований прибуток у real-money.
//! Спам-токени, що прикидаються USDC/WETH, відсіяні (фільтр за адресою контракту, не символом).
//! Аргументи: chains=1,42161,137  members=<файл|опц.>  eth=3000  matic=0.55
use std::collections::HashMap;
use crate::core::ctx::Ctx;
use crate::core::data::stream_txfull;
use crate::core::dict::{self, Money};
use crate::core::PResult;
use super::{Pattern, csv, num};

pub struct Pnl;

impl Pattern for Pnl {
    fn name(&self) -> &str { "pnl" }
    fn about(&self) -> &str { "реалізований P&L: нетто реальних грошей (native+стейбли/WETH за контрактом)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let chains = csv(args, "chains", "1,42161,137");
        let _peth = num(args, "eth", dict::price_eth_usd());

        // опційний фільтр членів
        let members: Option<std::collections::HashSet<String>> = args.get("members").map(|p| {
            std::fs::read_to_string(p).unwrap_or_default().lines()
                .filter_map(|l| l.split_whitespace().next().map(|s| s.to_lowercase()))
                .filter(|s| s.starts_with("0x")).collect()
        });

        let mut net_in: HashMap<String, f64> = HashMap::new();  // real-money USD received
        let mut net_out: HashMap<String, f64> = HashMap::new();
        for ch in &chains {
            stream_txfull(&ctx.txfull(ch), |tx| {
                if let Some(m) = &members { if !m.contains(&tx.w) { return } }
                // сума в USD-екв.
                let usd = match tx.k.as_str() {
                    "ext" | "int" => dict::to_usd(Money::Eth, dict::scale(&tx.v, 18)), // native ETH
                    "token" => match dict::real_money(&tx.tok) {
                        Some((kind, dec)) => dict::to_usd(kind, dict::scale(&tx.v, dec)),
                        None => return, // не реальні гроші (спам-токен відсіяно у словнику)
                    },
                    _ => return,
                };
                if usd <= 0.0 { return }
                if tx.t == tx.w { *net_in.entry(tx.w.clone()).or_default() += usd; }
                else if tx.f == tx.w { *net_out.entry(tx.w.clone()).or_default() += usd; }
            });
        }
        // агрегат
        let mut wallets: std::collections::HashSet<&String> = net_in.keys().collect();
        wallets.extend(net_out.keys());
        let mut rows: Vec<(f64, f64, f64, String)> = wallets.iter().map(|w| {
            let i = *net_in.get(*w).unwrap_or(&0.0);
            let o = *net_out.get(*w).unwrap_or(&0.0);
            (i - o, i, o, (*w).clone())
        }).collect();
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let tot: f64 = rows.iter().map(|r| r.0).sum();
        let ni: f64 = rows.iter().map(|r| r.1).sum();
        let no: f64 = rows.iter().map(|r| r.2).sum();
        println!("=== P&L (real-money net, {} гаманців):", rows.len());
        println!("  отримано ${:.0} - відправлено ${:.0} = НЕТТО ${:.0}", ni, no, tot);
        println!("=== топ-10 за нетто-прибутком:");
        for (net, i, o, a) in rows.iter().take(10) {
            println!("  {a}  нетто ${:.0} (in ${:.0} / out ${:.0})", net, i, o);
        }
        println!("=== найбільші МІНУСИ:");
        for (net, i, o, a) in rows.iter().rev().take(5) {
            println!("  {a}  нетто ${:.0} (in ${:.0} / out ${:.0})", net, i, o);
        }
        Ok(())
    }
}
