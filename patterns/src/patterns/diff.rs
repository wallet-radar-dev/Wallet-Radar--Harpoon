//! ПАТЕРН diff (Фаза 1.3 плану — «динаміка груп найцінніше для продажу»):
//! порівнює два останні знімки з group-history.csv по кожній мережі й показує ДЕЛЬТИ
//! (кластери, координовані гам, викупи емісії). Що зросло/зникло між датами.
//! Аргументи: chain=<all|1|42161|137>   (за замовч. усі мережі у файлі)
use std::collections::BTreeMap as Map;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::Pattern;

pub struct Diff;

// один рядок історії: chain -> (ts, clusters, coord_wallets, emission_buyups, top_token)
#[derive(Clone, Default)]
struct Row { ts: u64, clusters: i64, coord: i64, buyups: i64, top: String }

impl Pattern for Diff {
    fn name(&self) -> &str { "diff" }
    fn about(&self) -> &str { "дельти між двома останніми знімками груп (динаміка у часі)" }

    fn run(&self, ctx: &Ctx, args: &Map<String, String>) -> PResult {
        let path = format!("{}/group-history.csv", ctx.dir);
        let txt = std::fs::read_to_string(&path)
            .map_err(|e| format!("нема group-history.csv (спершу snapshot): {e}"))?;
        let only = args.get("chain").map(|s| s.as_str()).filter(|s| *s != "all");

        // очікуваний заголовок snapshot.rs: ts,dict_ver,chain,clusters,coord_wallets,emission_buyups,top_token,...
        let mut hdr: Vec<&str> = vec![];
        let mut per_chain: Map<String, Vec<Row>> = Map::new();
        for (i, line) in txt.lines().enumerate() {
            let c: Vec<&str> = line.split(',').collect();
            if i == 0 { hdr = c; continue }
            if c.len() < 4 { continue }
            let get = |name: &str| -> Option<usize> { hdr.iter().position(|h| h.trim() == name) };
            let idx = |name: &str, dflt: usize| get(name).unwrap_or(dflt);
            let chain = c.get(idx("chain", 1)).copied().unwrap_or("?").trim().to_string();
            if let Some(f) = only { if chain != f { continue } }
            let numv = |name: &str, dflt: usize| c.get(idx(name, dflt)).and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
            // назви як у snapshot.rs: ts,chain,dict_ver,n_clusters,n_coord_wallets,n_emission_buyups,top_token,top_cluster
            let row = Row {
                ts: c.get(idx("ts", 0)).and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0),
                clusters: numv("n_clusters", 3),
                coord: numv("n_coord_wallets", 4),
                buyups: numv("n_emission_buyups", 5),
                top: c.get(idx("top_token", 6)).copied().unwrap_or("").trim().to_string(),
            };
            per_chain.entry(chain).or_default().push(row);
        }
        if per_chain.is_empty() { println!("[diff] у group-history.csv нема даних для порівняння"); return Ok(()) }

        let arrow = |d: i64| if d > 0 { format!("▲+{d}") } else if d < 0 { format!("▼{d}") } else { "=".into() };
        println!("── ДИНАМІКА ГРУП (дельти між останніми знімками) ──");
        for (chain, mut rows) in per_chain {
            rows.sort_by_key(|r| r.ts);
            if rows.len() < 2 {
                let r = rows.last().cloned().unwrap_or_default();
                println!("chain {chain}: лише 1 знімок (ts {}) — потрібен ще один для дельти. \
                          зараз: кластери {}, координ. {}, викупи {}, топ {}",
                    r.ts, r.clusters, r.coord, r.buyups, r.top);
                continue;
            }
            let a = &rows[rows.len() - 2];
            let b = &rows[rows.len() - 1];
            println!("chain {chain}  (ts {} → {}):", a.ts, b.ts);
            println!("   кластери:        {} → {}  {}", a.clusters, b.clusters, arrow(b.clusters - a.clusters));
            println!("   координ. гам:     {} → {}  {}", a.coord, b.coord, arrow(b.coord - a.coord));
            println!("   викупи емісії:    {} → {}  {}", a.buyups, b.buyups, arrow(b.buyups - a.buyups));
            if a.top != b.top { println!("   топ-токен ЗМІНИВСЯ: {} → {}", a.top, b.top); }
            else { println!("   топ-токен: {} (без змін)", b.top); }
        }
        Ok(())
    }
}
