//! Граф фінансування з txfull + примітиви: сусідство, виключення хабів, конвергенція, контроль.
use std::collections::{HashMap, HashSet};
use crate::core::ctx::Ctx;
use crate::core::data::stream_txfull;

/// Ненапрямлений граф native-переказів (хто з ким рухав ETH) по заданих мережах.
pub struct FundingGraph {
    pub adj: HashMap<String, HashSet<String>>,
    pub deg: HashMap<String, u32>,
}

impl FundingGraph {
    /// Будує з txfull по мережах (k=ext, value>0).
    pub fn build(ctx: &Ctx, chains: &[&str]) -> FundingGraph {
        let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
        let mut deg: HashMap<String, u32> = HashMap::new();
        for ch in chains {
            stream_txfull(&ctx.txfull(ch), |tx| {
                if tx.k != "ext" || tx.v == "0" || tx.v.is_empty() { return }
                let w = tx.w;
                let cp = if tx.f == w { tx.t } else if tx.t == w { tx.f } else { return };
                if cp.is_empty() || cp == w { return }
                adj.entry(w.clone()).or_default().insert(cp.clone());
                adj.entry(cp.clone()).or_default().insert(w.clone());
                *deg.entry(cp).or_default() += 1;
            });
        }
        FundingGraph { adj, deg }
    }

    /// Хаби = top-degree (біржі/роутери) за перцентилем.
    pub fn hubs(&self, pct: f64) -> HashSet<String> {
        let mut degs: Vec<u32> = self.deg.values().copied().collect();
        degs.sort_unstable();
        if degs.is_empty() { return HashSet::new() }
        let cut = degs[((degs.len() as f64) * pct) as usize];
        self.deg.iter().filter(|(_, &d)| d >= cut).map(|(a, _)| a.clone()).collect()
    }

    /// Конвергенція: не-хаб вузли, на які виходять >=min_seeds сідів (1 хоп, не самі сіди).
    /// Повертає (вузол, к-сть сідів), відсортовано за спаданням.
    pub fn convergence(&self, seeds: &HashSet<String>, hubs: &HashSet<String>, min_seeds: usize)
        -> Vec<(String, usize)>
    {
        let mut hit: HashMap<String, HashSet<String>> = HashMap::new();
        for s in seeds {
            if let Some(nb) = self.adj.get(s) {
                for cp in nb {
                    if hubs.contains(cp) || seeds.contains(cp) { continue }
                    hit.entry(cp.clone()).or_default().insert(s.clone());
                }
            }
        }
        let mut out: Vec<(String, usize)> = hit.into_iter()
            .filter(|(_, v)| v.len() >= min_seeds)
            .map(|(a, v)| (a, v.len())).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    /// Детермінований набір N "випадкових" не-хаб вузлів (для контроль-тесту).
    pub fn control_seeds(&self, exclude: &HashSet<String>, n: usize) -> HashSet<String> {
        let mut all: Vec<&String> = self.adj.keys().filter(|a| !exclude.contains(*a)).collect();
        all.sort();
        if all.is_empty() || n == 0 { return HashSet::new() }
        let step = (all.len() / n).max(1);
        all.iter().step_by(step).take(n).map(|s| s.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edge(g: &mut FundingGraph, a: &str, b: &str) {
        g.adj.entry(a.into()).or_default().insert(b.into());
        g.adj.entry(b.into()).or_default().insert(a.into());
        *g.deg.entry(b.into()).or_default() += 1;
    }
    #[test]
    fn convergence_finds_shared_hub() {
        // s1,s2,s3 усі шлють у hub H; s1->x (не спільне)
        let mut g = FundingGraph { adj: HashMap::new(), deg: HashMap::new() };
        edge(&mut g, "s1", "h"); edge(&mut g, "s2", "h"); edge(&mut g, "s3", "h");
        edge(&mut g, "s1", "x");
        let seeds: HashSet<String> = ["s1","s2","s3"].iter().map(|s| s.to_string()).collect();
        let hubs = HashSet::new();
        let conv = g.convergence(&seeds, &hubs, 2);
        // h має зійтись від 3 сідів; x — лише від 1 (нижче порогу)
        assert_eq!(conv.len(), 1);
        assert_eq!(conv[0].0, "h");
        assert_eq!(conv[0].1, 3);
    }
    #[test]
    fn hubs_excluded_from_convergence() {
        let mut g = FundingGraph { adj: HashMap::new(), deg: HashMap::new() };
        edge(&mut g, "s1", "h"); edge(&mut g, "s2", "h");
        let seeds: HashSet<String> = ["s1","s2"].iter().map(|s| s.to_string()).collect();
        let hubs: HashSet<String> = ["h"].iter().map(|s| s.to_string()).collect();
        assert_eq!(g.convergence(&seeds, &hubs, 2).len(), 0); // h виключено як хаб
    }
}
