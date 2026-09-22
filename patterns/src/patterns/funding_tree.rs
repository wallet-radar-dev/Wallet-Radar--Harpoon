//! ПАТЕРН funding-tree: коло за фінансуванням від опорних гаманців (хопи вниз через Etherscan).
//! Від сідів BFS: кому вони слали native (>=2 перекази = засів), виключаючи біржі/контракти.
//! Рахує розподіл по хопах і макс. глибину. Аргументи: seeds=0x..,0x..  max_hop=8  cap=700
use std::collections::{HashMap, HashSet};
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::fetch::Es;
use crate::core::{PResult, exchange_tx};
use super::{Pattern, csv, num};

pub struct FundingTree;

impl Pattern for FundingTree {
    fn name(&self) -> &str { "funding-tree" }
    fn about(&self) -> &str { "коло за фінансуванням від опорних гаманців (хопи вниз, Etherscan)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let seeds = csv(args, "seeds", "");
        if seeds.is_empty() { return Err("вкажи seeds=0x..,0x..".into()) }
        let max_hop = num(args, "max_hop", 8.0) as u32;
        let cap = num(args, "cap", 700.0) as usize;
        let es = Es::new(&ctx.keys);

        // кому a слав native >=2 разів (eth+arb+poly)
        let children = |a: &str| -> Vec<String> {
            let mut cnt: HashMap<String, u32> = HashMap::new();
            for chain in [1u64, 42161, 137] {
                for act in ["txlist", "txlistinternal"] {
                    let r = es.call(chain, &format!("module=account&action={act}"),
                        &format!("&address={a}&startblock=0&endblock=99999999&page=1&offset=1000&sort=desc"));
                    if let Some(arr) = r.as_ref().and_then(Value::as_array) {
                        for t in arr {
                            let fr = t.get("from").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                            let to = t.get("to").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                            let v = t.get("value").and_then(|x| x.as_str()).unwrap_or("0");
                            if fr == a && !to.is_empty() && to != a && v != "0" && !v.is_empty() {
                                *cnt.entry(to).or_default() += 1;
                            }
                        }
                    }
                }
            }
            cnt.into_iter().filter(|(_, c)| *c >= 2).map(|(w, _)| w).collect()
        };
        let is_contract = |a: &str| -> bool {
            es.call(1, "module=proxy&action=eth_getCode", &format!("&address={a}&tag=latest"))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .map(|s| s != "0x" && s != "0x0" && !s.is_empty()).unwrap_or(false)
        };

        let mut member_hop: HashMap<String, u32> = seeds.iter().map(|s| (s.clone(), 0)).collect();
        let mut frontier: Vec<String> = seeds.clone();
        for hop in 1..=max_hop {
            let mut next = Vec::new();
            'outer: for w in frontier.iter().take(60) {
                for c in children(w) {
                    if member_hop.contains_key(&c) { continue }
                    if is_contract(&c) { continue }
                    if es.tx_count(1, &c) as u64 > exchange_tx() { continue }
                    member_hop.insert(c.clone(), hop);
                    next.push(c);
                    if member_hop.len() >= cap { break 'outer }
                }
            }
            eprintln!("[funding-tree] hop {hop}: +{} (усього {})", next.len(), member_hop.len());
            if next.is_empty() || member_hop.len() >= cap { break }
            frontier = next;
        }
        // розподіл
        let mut byhop: HashMap<u32, usize> = HashMap::new();
        for h in member_hop.values() { *byhop.entry(*h).or_default() += 1 }
        let maxh = member_hop.values().copied().max().unwrap_or(0);
        println!("=== КОЛО: {} гаманців, макс хоп {maxh}", member_hop.len());
        let mut hops: Vec<_> = byhop.into_iter().collect(); hops.sort();
        for (h, n) in hops { println!("  hop {h}: {n} гаманців"); }
        // збереження
        let out = format!("{}/circle-{}.txt", ctx.dir, &seeds[0][..10.min(seeds[0].len())]);
        let body: String = { let mut v: Vec<_> = member_hop.iter().collect(); v.sort_by_key(|x| *x.1);
            v.iter().map(|(w, h)| format!("{w}\t{h}")).collect::<Vec<_>>().join("\n") };
        let _ = std::fs::write(&out, body);
        println!("=> {out}");
        let _: HashSet<()> = HashSet::new();
        Ok(())
    }
}
