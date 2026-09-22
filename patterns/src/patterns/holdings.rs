//! ПАТЕРН holdings: холдинги (native) + вивід на біржі по набору гаманців.
//! Відсів КОНТРАКТІВ за кодом (не за балансом). Вивід = вихідні native на не-члена,
//! не-контракт, високо-tx адресу (біржа). Member->member НЕ вивід.
//! Аргументи: members=<файл (0x.. [\t hop])>  maxhop=1  eth=3000  matic=0.55
use std::collections::HashSet;
use crate::core::ctx::Ctx;
use crate::core::fetch::Es;
use crate::core::{PResult, exchange_tx};
use super::{Pattern, num};

pub struct Holdings;

impl Pattern for Holdings {
    fn name(&self) -> &str { "holdings" }
    fn about(&self) -> &str { "холдинги native + вивід на біржі (відсів контрактів за кодом)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let path = args.get("members").ok_or("вкажи members=<файл>")?;
        let maxhop = num(args, "maxhop", 1.0) as u32;
        let peth = num(args, "eth", 3000.0);
        let pmatic = num(args, "matic", 0.55);
        let es = Es::new(&ctx.keys);

        // читаємо членів (0x.. [\t hop]), фільтр hop<=maxhop
        let mut members: Vec<String> = Vec::new();
        for line in std::fs::read_to_string(path).map_err(|e| e.to_string())?.lines() {
            let mut it = line.split_whitespace();
            let Some(a) = it.next() else { continue };
            if !a.starts_with("0x") { continue }
            let hop: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            if hop <= maxhop { members.push(a.to_lowercase()); }
        }
        let mset: HashSet<String> = members.iter().cloned().collect();
        eprintln!("[holdings] членів (hop<={maxhop}): {}", members.len());

        let (mut eth_t, mut matic_t, mut wd_t) = (0.0f64, 0.0f64, 0.0f64);
        let mut contracts = 0;
        let mut rows: Vec<(f64, f64, f64, f64, String)> = Vec::new(); // usd,eth,matic,wd,addr
        for (i, a) in members.iter().enumerate() {
            if es.is_contract(1, a) { contracts += 1; continue }  // не член — контракт/пул
            let eth = es.balance_eth(1, a) + es.balance_eth(42161, a);
            let matic = es.balance_eth(137, a);
            eth_t += eth; matic_t += matic;
            // вивід: вихідні native на не-члена, високо-tx (біржа)
            let mut wd = 0.0;
            for chain in [1u64, 42161, 137] {
                for (to, amt) in es.outbound_native(chain, a, 500) {
                    if mset.contains(&to) { continue }            // member->member не вивід
                    // справжній кешаут = EOA-біржа (високо-tx), НЕ DEX-роутер-контракт (то оборот)
                    if es.tx_count(chain, &to) as u64 > exchange_tx() && !es.is_contract(chain, &to) {
                        wd += amt;
                    }
                }
            }
            wd_t += wd;
            rows.push((eth * peth + matic * pmatic, eth, matic, wd, a.clone()));
            if (i + 1) % 25 == 0 { eprintln!("  {}/{}", i + 1, members.len()); }
        }
        rows.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
        let usd = eth_t * peth + matic_t * pmatic;
        println!("=== КОЛО ({} членів, {contracts} контрактів відсіяно):", members.len());
        println!("  ХОЛДИНГИ native: {:.1} ETH + {:.0} MATIC = ~${:.0}", eth_t, matic_t, usd);
        println!("  ВИВЕДЕНО на біржі (вибірка): ~{:.1} ETH", wd_t);
        println!("=== топ-10 за холдингами:");
        for (u, e, m, wd, a) in rows.iter().take(10) {
            println!("  {a}  {:.1} ETH + {:.0} MATIC = ~${:.0}  (вивід {:.1} ETH)", e, m, u, wd);
        }
        Ok(())
    }
}
