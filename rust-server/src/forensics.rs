//! Port of the pure core of server/forensics-worker.js — the continuous deep scan that
//! finds COORDINATED holder groups inside each whale asset, and the alpha feed that
//! falls out of it ("a whale just entered a token it never held").
//!
//! Two independent links merge holders into one group:
//!   (a) a shared value FUNDER — the same address funded both holders' gas/capital;
//!   (b) a MIRRORED portfolio — two holders carry ≥6 of the same niche tokens with
//!       ≥50% containment. Ubiquitous tokens (ETH/USDC/…) are excluded, or every
//!       wallet on earth would "mirror" every other.
//!
//! Both links feed one union-find, so a group can be held together by either kind of
//! evidence. Network I/O (holders, funders, portfolios) stays in main.rs.

use crate::jsmath::{js_num, js_round, round_fixed};
use indexmap::{IndexMap, IndexSet};
use serde_json::{json, Value};

/// forensics-worker.js:20 — depth: top-50 holders per asset.
pub const HOLDERS: usize = 50;
/// Chains whose native asset is ETH.
const ETH_NATIVE: [&str; 10] =
    ["eth", "base", "optimism", "arbitrum", "zksync", "scroll", "linea", "blast", "mode", "zora"];
/// forensics-worker.js:44 — tokens almost everyone holds never count as a mirror match.
const UBIQUITOUS: [&str; 10] = ["ETH", "WETH", "USDT", "USDC", "DAI", "USDS", "BUSD", "TUSD", "FRAX", "WBTC"];

pub fn is_ubiquitous(sym: &str) -> bool {
    UBIQUITOUS.contains(&sym)
}

/// forensics-worker.js:41 — the hard-coded exchange set (separate from cex.rs: this is
/// the worker's own short list, and staying byte-compatible with Node matters more than
/// deduplicating it).
/// Набір адрес із бази за тегом.
///
/// Шість модулів rust-server мали власну копію набору `KNOWN` з index.js, і копії
/// РОЗІЙШЛИСЬ: спільних 9 адрес, у трьох є ще FTX, у двох — ще одна біржа,
/// forensics не має жодної з двох. Тому в кожного свій тег: звести їх до одного
/// означало б мовчки розширити три модулі.
fn tag_has(tag: &str, addr: &str) -> bool {
    wr_store::snapshot().wallet_has_tag("eth", addr, tag)
}

pub fn is_known_cex(addr: &str) -> bool {
    tag_has("forensics:is_known_cex", addr)
}

/// Native-token prices, in the shape forensics-worker.js keeps them.
#[derive(Clone, Copy, Default)]
pub struct Px {
    pub eth: f64,
    pub matic: f64,
    pub xdai: f64,
    pub celo: f64,
}

/// forensics-worker.js:49 — a chain's native amount in USD.
pub fn native_usd(chain: &str, amt: f64, px: &Px) -> f64 {
    if ETH_NATIVE.contains(&chain) {
        amt * px.eth
    } else if chain == "polygon" {
        amt * px.matic
    } else if chain == "gnosis" {
        amt * px.xdai
    } else if chain == "celo" {
        amt * px.celo
    } else {
        amt * px.eth
    }
}

/// forensics-worker.js:72 `contain` — overlap of the SMALLER set into the larger, and
/// that overlap as a fraction of the smaller set.
fn contain(a: &IndexSet<String>, b: &IndexSet<String>) -> (usize, f64) {
    let (s, l) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let n = s.iter().filter(|x| l.contains(*x)).count();
    let denom = a.len().min(b.len()).max(1) as f64;
    (n, n as f64 / denom)
}

/// Union-find with path halving, exactly as the JS `find` does.
struct Uf(IndexMap<String, String>);
impl Uf {
    fn find(&mut self, x: &str) -> String {
        let mut x = x.to_string();
        while self.0.get(&x).map(String::as_str) != Some(x.as_str()) {
            let p = self.0[&x].clone();
            let gp = self.0[&p].clone();
            self.0.insert(x.clone(), gp);
            x = p;
        }
        x
    }
    fn union(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        self.0.insert(ra, rb);
    }
}

/// forensics-worker.js `analyzeAsset`, minus the fetches.
///
/// `holders`: `[{addr, share}]`, already filtered and capped at HOLDERS.
/// `hf`: addr → funder → `[{amount, chain}]` (that holder's incoming native funders).
/// `pf`: addr → `{tokens: [niche syms], totalUsd, top}` — the portfolio fingerprint.
pub fn analyze_groups(holders: &[Value], hf: &Value, pf: &Value, px: &Px) -> (Vec<Value>, IndexMap<String, IndexSet<String>>) {
    let addr_of = |h: &Value| h["addr"].as_str().unwrap_or("").to_string();

    // fh: funder → {holders, txs}, in first-seen order (JS Map).
    let mut fh: IndexMap<String, (IndexSet<String>, Vec<Value>)> = IndexMap::new();
    for h in holders {
        let a = addr_of(h);
        let Some(funders) = hf[&a].as_object() else { continue };
        for (f, txs) in funders {
            let e = fh.entry(f.clone()).or_insert_with(|| (IndexSet::new(), Vec::new()));
            e.0.insert(a.clone());
            if let Some(arr) = txs.as_array() {
                e.1.extend(arr.iter().cloned());
            }
        }
    }

    let mut uf = Uf(holders.iter().map(|h| (addr_of(h), addr_of(h))).collect());
    // Evidence is anchored to a MEMBER address, not to the root that existed when it was
    // found. The JS keys `shared[find(hs[0])]` immediately, so a later mirror union that
    // moves the root strands the entry: the group survives, but its funder evidence
    // silently disappears. That is not cosmetic — conductor-finder feeds on
    // `commonFunders`, so a stranded entry means a real conductor is never detected.
    // (Seen live on ALLO: same group, funders 5 in one run and 0 in the next.)
    // Anchoring to a member and resolving the root at the END keeps every piece of
    // evidence, whichever way the unions happen to fall.
    let mut shared_entries: Vec<(String, Value)> = Vec::new();
    // Cross-asset funder accumulation, returned for the caller's global map.
    let mut global_funders: IndexMap<String, IndexSet<String>> = IndexMap::new();

    // (a) link by a shared value funder.
    for (f, (hs, txs)) in &fh {
        if hs.len() < 2 {
            continue;
        }
        let sum: f64 = txs
            .iter()
            .map(|x| native_usd(x["chain"].as_str().unwrap_or(""), x["amount"].as_f64().unwrap_or(0.0), px))
            .sum();
        let usd = sum / if txs.is_empty() { 1.0 } else { txs.len() as f64 };
        if usd < 50.0 {
            continue;
        }
        let hs: Vec<&String> = hs.iter().collect();
        for i in 1..hs.len() {
            uf.union(hs[0], hs[i]);
        }
        shared_entries.push((
            hs[0].clone(),
            json!({
                "funder": f,
                "avgUsd": js_num(js_round(usd)),
                "holders": js_num(hs.len() as f64),
            }),
        ));
        let g = global_funders.entry(f.clone()).or_default();
        for x in &hs {
            g.insert((*x).clone());
        }
    }

    // (b) link by a mirrored portfolio.
    let sets: Vec<IndexSet<String>> = holders
        .iter()
        .map(|h| {
            pf[&addr_of(h)]["tokens"]
                .as_array()
                .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                .unwrap_or_default()
        })
        .collect();
    let mut mirror_entries: Vec<(String, Value)> = Vec::new();
    for i in 0..holders.len() {
        for j in (i + 1)..holders.len() {
            let (a, b) = (&sets[i], &sets[j]);
            if a.len() < 6 || b.len() < 6 {
                continue;
            }
            let (overlap, ratio) = contain(a, b);
            if overlap >= 6 && ratio >= 0.5 {
                let (ai, aj) = (addr_of(&holders[i]), addr_of(&holders[j]));
                uf.union(&ai, &aj);
                mirror_entries.push((ai.clone(), json!({"a": ai, "b": aj, "overlap": js_num(overlap as f64)})));
            }
        }
    }

    // Every union is done — NOW resolve each piece of evidence to its final root.
    let mut shared: IndexMap<String, Vec<Value>> = IndexMap::new();
    for (anchor, e) in shared_entries {
        let r = uf.find(&anchor);
        shared.entry(r).or_default().push(e);
    }
    let mut mirror: IndexMap<String, usize> = IndexMap::new();
    for (anchor, _) in mirror_entries {
        let r = uf.find(&anchor);
        *mirror.entry(r).or_insert(0) += 1;
    }

    // Components → groups of ≥2.
    let mut gmap: IndexMap<String, Vec<Value>> = IndexMap::new();
    for h in holders {
        let r = uf.find(&addr_of(h));
        gmap.entry(r).or_default().push(h.clone());
    }

    let mut groups: Vec<Value> = gmap
        .into_iter()
        .filter(|(_, g)| g.len() >= 2)
        .map(|(root, mut g)| {
            g.sort_by(|a, b| {
                let f = |v: &Value| v["share"].as_f64().unwrap_or(0.0);
                f(b).partial_cmp(&f(a)).unwrap_or(std::cmp::Ordering::Equal)
            });
            let combined: f64 = g.iter().map(|h| h["share"].as_f64().unwrap_or(0.0)).sum();
            let mut funders = shared.get(&root).cloned().unwrap_or_default();
            funders.sort_by(|a, b| {
                let f = |v: &Value| v["avgUsd"].as_f64().unwrap_or(0.0);
                f(b).partial_cmp(&f(a)).unwrap_or(std::cmp::Ordering::Equal)
            });
            funders.truncate(5);
            json!({
                "members": g,
                "combinedShare": js_num(round_fixed(combined, 2)),
                "commonFunders": funders,
                "mirror": js_num(mirror.get(&root).copied().unwrap_or(0) as f64),
            })
        })
        .collect();
    groups.sort_by(|a, b| {
        let f = |v: &Value| v["combinedShare"].as_f64().unwrap_or(0.0);
        f(b).partial_cmp(&f(a)).unwrap_or(std::cmp::Ordering::Equal)
    });

    (groups, global_funders)
}

/// forensics-worker.js `portfolioOf` reduction, minus the fetch. `per` is one
/// `tokenBalances` list per chain (`{symbol, valueUsd}`).
///
/// The `< $1e9` guard is not paranoia: a single position priced above a billion dollars
/// is essentially always a spam token with a fake exchange rate.
pub fn portfolio_of(per: &[Value]) -> Value {
    let mut agg: IndexMap<String, f64> = IndexMap::new();
    for list in per {
        let Some(arr) = list.as_array() else { continue };
        for tk in arr {
            let usd = tk["valueUsd"].as_f64().unwrap_or(0.0);
            if usd > 200.0 && usd < 1e9 {
                let sym = tk["symbol"].as_str().unwrap_or("").to_string();
                *agg.entry(sym).or_insert(0.0) += usd;
            }
        }
    }
    let mut toks: Vec<(String, f64)> = agg.into_iter().collect();
    toks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let usd: f64 = toks.iter().map(|t| t.1).sum();
    let niche: Vec<Value> = toks.iter().filter(|t| !is_ubiquitous(&t.0)).map(|t| Value::String(t.0.clone())).collect();
    let top: Vec<Value> = toks.iter().take(12).map(|t| Value::String(t.0.clone())).collect();
    json!({"tokens": niche, "totalUsd": js_num(js_round(usd)), "top": top})
}

/// forensics-worker.js `emitAlpha` — the whale-entered-a-new-token event, or None if the
/// same (address, tokens) pair already fired within 24h.
pub fn alpha_event(feed: &[Value], address: &str, tokens: &[String], via_asset: &str, share: f64, now: i64) -> Option<Value> {
    let key = format!("{address}|{}", tokens.join(","));
    if feed.iter().any(|a| a["key"].as_str() == Some(key.as_str()) && now - a["ts"].as_i64().unwrap_or(0) < 86_400_000) {
        return None;
    }
    Some(json!({
        "key": key,
        "address": address,
        "tokens": tokens,
        "viaAsset": via_asset,
        "share": js_num(round_fixed(share, 2)),
        "ts": js_num(now as f64),
    }))
}

/// The tokens a whale now holds in its TOP portfolio that it did not hold before.
/// Only fires for a holder with a meaningful share (≥0.3%) — a dust holder entering a
/// token is not a signal.
pub fn fresh_tokens(old_tokens: &[Value], new_top: &[Value], share: f64) -> Vec<String> {
    if old_tokens.is_empty() || new_top.is_empty() || share < 0.3 {
        return vec![];
    }
    let old: IndexSet<&str> = old_tokens.iter().filter_map(|t| t.as_str()).collect();
    new_top
        .iter()
        .filter_map(|t| t.as_str())
        .filter(|s| !old.contains(s) && !is_ubiquitous(s))
        .take(4)
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0xaaa";
    const B: &str = "0xbbb";
    const C: &str = "0xccc";
    const F: &str = "0xf1";

    fn px() -> Px {
        Px { eth: 3000.0, matic: 1.0, xdai: 1.0, celo: 1.0 }
    }
    fn holders(hs: &[(&str, f64)]) -> Value {
        json!(hs.iter().map(|(a, s)| json!({"addr": a, "share": s})).collect::<Vec<_>>())
    }

    #[test]
    fn a_shared_funder_merges_holders_into_one_group() {
        let hs = holders(&[(A, 3.0), (B, 2.0), (C, 1.0)]);
        // A and B funded by the same address; C funded by nobody.
        let hf = json!({
            A: {F: [{"amount": 1.0, "chain": "eth"}]},
            B: {F: [{"amount": 2.0, "chain": "eth"}]},
            C: {},
        });
        let pf = json!({A: {"tokens": []}, B: {"tokens": []}, C: {"tokens": []}});
        let (groups, gf) = analyze_groups(hs.as_array().unwrap(), &hf, &pf, &px());

        assert_eq!(groups.len(), 1, "C is alone → not a group");
        assert_eq!(groups[0]["combinedShare"], json!(5));
        assert_eq!(groups[0]["members"].as_array().unwrap().len(), 2);
        assert_eq!(groups[0]["commonFunders"][0]["funder"], json!(F));
        // avg of ($3000, $6000) = $4500
        assert_eq!(groups[0]["commonFunders"][0]["avgUsd"], json!(4500));
        assert_eq!(gf[F].len(), 2, "the funder is remembered for the cross-asset pass");
    }

    /// A funder who dribbles dust is not evidence of coordination.
    #[test]
    fn a_sub_50_dollar_funder_links_nothing() {
        let hs = holders(&[(A, 1.0), (B, 1.0)]);
        let hf = json!({
            A: {F: [{"amount": 0.001, "chain": "eth"}]}, // $3
            B: {F: [{"amount": 0.001, "chain": "eth"}]},
        });
        let pf = json!({A: {"tokens": []}, B: {"tokens": []}});
        let (groups, _) = analyze_groups(hs.as_array().unwrap(), &hf, &pf, &px());
        assert!(groups.is_empty());
    }

    #[test]
    fn mirrored_portfolios_merge_holders_even_without_a_common_funder() {
        let hs = holders(&[(A, 1.0), (B, 1.0)]);
        let same = ["T1", "T2", "T3", "T4", "T5", "T6"];
        let hf = json!({A: {}, B: {}});
        let pf = json!({A: {"tokens": same}, B: {"tokens": same}});
        let (groups, _) = analyze_groups(hs.as_array().unwrap(), &hf, &pf, &px());

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["mirror"], json!(1));
        assert!(groups[0]["commonFunders"].as_array().unwrap().is_empty());
    }

    /// The guard that stops "everyone holds USDC" from making everyone a cartel: five
    /// shared niche tokens is below the bar, and ubiquitous tokens never count at all.
    #[test]
    fn five_shared_tokens_is_not_a_mirror() {
        let hs = holders(&[(A, 1.0), (B, 1.0)]);
        let hf = json!({A: {}, B: {}});
        let pf = json!({
            A: {"tokens": ["T1", "T2", "T3", "T4", "T5", "X1"]},
            B: {"tokens": ["T1", "T2", "T3", "T4", "T5", "X2"]},
        });
        let (groups, _) = analyze_groups(hs.as_array().unwrap(), &hf, &pf, &px());
        assert!(groups.is_empty(), "overlap 5 < 6 → not a mirror");
    }

    /// The regression this module was fixed for. A funder links A+B; a mirrored portfolio
    /// then links B+C, which moves the union-find root. The JS keys its funder evidence by
    /// the root as it stood at (a)-time, so after the move the evidence is stranded and the
    /// group reports `commonFunders: []` — even though a shared funder is precisely what
    /// holds it together. conductor-finder reads exactly that field, so the conductor
    /// silently disappears. Evidence must survive a later merge.
    #[test]
    fn funder_evidence_survives_a_later_mirror_merge() {
        let hs = holders(&[(A, 3.0), (B, 2.0), (C, 1.0)]);
        let hf = json!({
            A: {F: [{"amount": 1.0, "chain": "eth"}]},
            B: {F: [{"amount": 1.0, "chain": "eth"}]},
            C: {},
        });
        // B and C mirror each other, merging C into the A+B component afterwards.
        let same = ["T1", "T2", "T3", "T4", "T5", "T6"];
        let pf = json!({A: {"tokens": []}, B: {"tokens": same}, C: {"tokens": same}});

        let (groups, _) = analyze_groups(hs.as_array().unwrap(), &hf, &pf, &px());

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["members"].as_array().unwrap().len(), 3, "A, B and C are one group");
        assert_eq!(groups[0]["mirror"], json!(1));
        assert_eq!(
            groups[0]["commonFunders"][0]["funder"],
            json!(F),
            "the funder that built the group must still be reported as its evidence"
        );
    }

    #[test]
    fn portfolio_drops_dust_and_billion_dollar_fakes_and_strips_ubiquitous() {
        let per = json!([
            [{"symbol": "PEPE", "valueUsd": 5000.0}, {"symbol": "USDC", "valueUsd": 9000.0}],
            [{"symbol": "PEPE", "valueUsd": 1000.0}, {"symbol": "DUST", "valueUsd": 50.0},
             {"symbol": "FAKE", "valueUsd": 2e9}],
        ]);
        let p = portfolio_of(per.as_array().unwrap());

        // PEPE summed across chains; DUST (<$200) and FAKE (>$1e9) dropped.
        assert_eq!(p["totalUsd"], json!(15000));
        // niche = everything except the ubiquitous USDC, ordered by USD desc.
        assert_eq!(p["tokens"], json!(["PEPE"]));
        assert_eq!(p["top"], json!(["USDC", "PEPE"]));
    }

    #[test]
    fn alpha_fires_once_per_day_per_address_and_token_set() {
        let toks = vec!["PEPE".to_string(), "WIF".to_string()];
        let ev = alpha_event(&[], A, &toks, "LINK", 1.234, 1_000_000).unwrap();
        assert_eq!(ev["share"], json!(1.23));
        assert_eq!(ev["key"], json!("0xaaa|PEPE,WIF"));

        let feed = vec![ev.clone()];
        assert!(alpha_event(&feed, A, &toks, "LINK", 1.0, 1_000_001).is_none(), "same day → deduped");
        // 24h + 1ms later it may fire again.
        assert!(alpha_event(&feed, A, &toks, "LINK", 1.0, 1_000_000 + 86_400_001).is_some());
    }

    #[test]
    fn fresh_tokens_ignores_dust_holders_and_ubiquitous_tokens() {
        let old = json!(["PEPE"]);
        let top = json!(["PEPE", "WIF", "USDC"]);
        let old = old.as_array().unwrap();
        let top = top.as_array().unwrap();

        assert_eq!(fresh_tokens(old, top, 1.0), vec!["WIF".to_string()], "USDC is ubiquitous, PEPE is not new");
        assert!(fresh_tokens(old, top, 0.29).is_empty(), "a 0.29% holder is not a whale signal");
        assert!(fresh_tokens(&[], top, 5.0).is_empty(), "no prior fingerprint → nothing is 'new'");
    }
}
