//! linkall — зв'язки між ВСІМА гаманцями з tokenpnl за критеріями grouppart.
//!
//! Правила взяті з їхнього розбиття дослівно, бо вони вистраждані на помилках:
//!   • стейбли й обгортки не є сигналом — спільний USDC не пов'язує нікого;
//!   • вага спільного токена = IDF: рідкісний важить більше за популярний, а спам-ейрдроп,
//!     розкиданий на тисячі адрес, знецінюється сам;
//!   • ОДНОГО спільного токена не досить — потрібно ≥MIN_LINKS незалежних (у них через
//!     єдине dust-ребро в NET затягнуло EOA з $1.97 млрд);
//!   • хаби геть: гаманець, що сидить у надто багатьох активах, склеює все підряд —
//!     це аналог їхнього HUB_DEG для портфельного графа;
//!   • казна емітента, біржі й отримувачі роздачі виключені ще на рівні tokenpnl.
//!
//! І головне, чого в їхньому біні немає, а в цій сесії вже двічі рятувало: НУЛЬ-ТЕСТ.
//! Тасуємо зв'язки гаманець↔актив зі збереженням обох степенів і ганяємо ту саму
//! кластеризацію. Якщо випадковість дає такі самі групи — знахідки немає, і про це
//! треба сказати, а не показувати таблицю.
//!
//! Args: ROOT. Env: MIN_LINKS(2) MIN_SCORE(8) HUB_TOK(40) MIN_GROUP(3) NULL_ITERS(10)
//! Вихід: dataset/backfill/linkall.json + linkall-report.md

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;

fn envn<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d)
}
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Правило is_stable із grouppart, слово в слово.
fn is_stable(sym: &str) -> bool {
    let s = sym.to_uppercase();
    s.contains("USD")
        || s.contains("EUR")
        || s.contains("GBP")
        || matches!(
            s.as_str(),
            "DAI" | "FRAX" | "LUSD" | "TUSD" | "USTB" | "AUSD" | "GHO" | "MIM" | "WBTC"
                | "CBBTC" | "TBTC" | "WETH" | "WBETH" | "CBETH" | "WSTETH" | "STETH" | "RETH"
                | "WEETH" | "EETH"
        )
        || s.ends_with("ON")
}

struct Uf(Vec<usize>);
impl Uf {
    fn new(n: usize) -> Self {
        Uf((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra] = rb;
        }
    }
}

/// Кластеризація за спільними рідкісними активами. Винесена окремо, щоб нуль-тест
/// проганяв РІВНО той самий код, а не його переказ своїми словами.
fn cluster(
    by_token: &[Vec<usize>],
    weight: &[f64],
    n_wallets: usize,
    min_links: usize,
    min_score: f64,
) -> Vec<Vec<usize>> {
    let mut pair: HashMap<(usize, usize), (f64, usize)> = HashMap::new();
    for (ti, holders) in by_token.iter().enumerate() {
        let w = weight[ti];
        if w <= 0.0 || holders.len() > 600 {
            continue;
        }
        for i in 0..holders.len() {
            for j in (i + 1)..holders.len() {
                let k = (holders[i].min(holders[j]), holders[i].max(holders[j]));
                let e = pair.entry(k).or_insert((0.0, 0));
                e.0 += w;
                e.1 += 1;
            }
        }
    }
    let mut uf = Uf::new(n_wallets);
    for ((a, b), (s, n)) in &pair {
        if *n >= min_links && *s >= min_score {
            uf.union(*a, *b);
        }
    }
    let mut comp: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n_wallets {
        let r = uf.find(i);
        comp.entry(r).or_default().push(i);
    }
    comp.into_values().collect()
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let dir = format!("{root}/dataset/backfill");
    let min_links: usize = envn("MIN_LINKS", 2);
    let min_score: f64 = envn("MIN_SCORE", 8.0);
    let hub_tok: usize = envn("HUB_TOK", 40);
    let min_group: usize = envn("MIN_GROUP", 3);
    let null_iters: usize = envn("NULL_ITERS", 10);

    // ── 1. Матриця гаманець × актив із tokenpnl ─────────────────────────────
    let mut wid: HashMap<String, usize> = HashMap::new();
    let mut wnames: Vec<String> = Vec::new();
    let mut tnames: Vec<String> = Vec::new();
    let mut by_token: Vec<Vec<usize>> = Vec::new();
    let mut skipped_stable = 0usize;

    for line in fs::read_to_string(format!("{dir}/tokenpnl.jsonl")).unwrap_or_default().lines() {
        let Ok(d) = serde_json::from_str::<Value>(line) else { continue };
        let Some(ws) = d["wallets"].as_array() else { continue };
        if ws.is_empty() || d["priceSuspect"] == json!(true) {
            continue;
        }
        let sym = d["symbol"].as_str().unwrap_or("");
        if is_stable(sym) {
            skipped_stable += 1;
            continue;
        }
        let mut holders = Vec::new();
        for w in ws {
            if w["treasurySuspect"] == json!(true)
                || w["cex"] == json!(true)
                || w["viaDistribution"] == json!(true)
            {
                continue;
            }
            let a = w["wallet"].as_str().unwrap_or("").to_string();
            if a.is_empty() {
                continue;
            }
            let id = *wid.entry(a.clone()).or_insert_with(|| {
                wnames.push(a.clone());
                wnames.len() - 1
            });
            holders.push(id);
        }
        holders.sort_unstable();
        holders.dedup();
        if holders.len() < 2 {
            continue;
        }
        tnames.push(sym.to_string());
        by_token.push(holders);
    }
    let n_all = wnames.len();
    println!(
        "[linkall] активів у графі: {} (стейблів/обгорток відкинуто: {skipped_stable}) · гаманців: {n_all}",
        by_token.len()
    );

    // ── 2. Хаби геть + лише мульти-активні ──────────────────────────────────
    let mut deg = vec![0usize; n_all];
    for h in &by_token {
        for &w in h {
            deg[w] += 1;
        }
    }
    let hubs = deg.iter().filter(|d| **d > hub_tok).count();
    let keep: Vec<bool> = deg.iter().map(|d| *d >= 2 && *d <= hub_tok).collect();
    let kept = keep.iter().filter(|x| **x).count();
    println!("[linkall] хабів (>{hub_tok} активів) виключено: {hubs} · у графі лишилось: {kept}");

    let by_token: Vec<Vec<usize>> = by_token
        .into_iter()
        .map(|h| h.into_iter().filter(|&w| keep[w]).collect())
        .collect();

    // IDF: рідкісний спільний актив важить більше за популярний
    let weight: Vec<f64> = by_token
        .iter()
        .map(|h| {
            if h.len() < 2 {
                0.0
            } else {
                (kept as f64 / h.len() as f64).ln().max(0.0)
            }
        })
        .collect();

    // ── 3. Кластеризація ────────────────────────────────────────────────────
    let mut groups: Vec<Vec<usize>> = cluster(&by_token, &weight, n_all, min_links, min_score)
        .into_iter()
        .filter(|g| g.len() >= min_group)
        .collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));
    println!("[linkall] груп (≥{min_group}): {} · найбільша: {}", groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0));

    // ── 4. Нуль-тест ────────────────────────────────────────────────────────
    let mut pairs: Vec<(usize, usize)> = Vec::new(); // (wallet, token)
    for (ti, h) in by_token.iter().enumerate() {
        if weight[ti] > 0.0 {
            for &w in h {
                pairs.push((w, ti));
            }
        }
    }
    let mut seed = 0xC0FFEE123456789u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut null_groups, mut null_largest) = (0.0f64, 0usize);
    for _ in 0..null_iters {
        let n = pairs.len();
        for _ in 0..(n * 6) {
            let (i, j) = ((rnd() as usize) % n, (rnd() as usize) % n);
            if pairs[i].1 == pairs[j].1 || pairs[i].0 == pairs[j].0 {
                continue;
            }
            let (a1, a2) = (pairs[i].1, pairs[j].1);
            pairs[i].1 = a2;
            pairs[j].1 = a1;
        }
        let mut bt: Vec<Vec<usize>> = vec![Vec::new(); by_token.len()];
        for (w, t) in &pairs {
            bt[*t].push(*w);
        }
        let g: Vec<Vec<usize>> = cluster(&bt, &weight, n_all, min_links, min_score)
            .into_iter()
            .filter(|g| g.len() >= min_group)
            .collect();
        null_groups += g.len() as f64;
        null_largest = null_largest.max(g.iter().map(|x| x.len()).max().unwrap_or(0));
    }
    null_groups /= null_iters.max(1) as f64;
    println!(
        "[linkall] НУЛЬ: {null_groups:.1} груп, найбільша до {null_largest} · спостережено {} груп, найбільша {}",
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    );

    // ── 5. Опис ─────────────────────────────────────────────────────────────
    let known: HashMap<String, String> = fs::read_to_string(format!("{dir}/group-membership.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["wallets"].as_object().cloned())
        .map(|o| {
            o.into_iter()
                .filter_map(|(k, v)| v["group"].as_str().map(|g| (k.to_lowercase(), g.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let mut wtok: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ti, h) in by_token.iter().enumerate() {
        for &w in h {
            wtok.entry(w).or_default().push(ti);
        }
    }

    let mut out = Vec::new();
    for (gi, g) in groups.iter().take(40).enumerate() {
        let mut basket: HashMap<usize, usize> = HashMap::new();
        let mut kg: HashMap<&str, usize> = HashMap::new();
        for &w in g {
            for t in wtok.get(&w).map(|v| v.as_slice()).unwrap_or(&[]) {
                *basket.entry(*t).or_insert(0) += 1;
            }
            *kg.entry(known.get(&wnames[w]).map(|s| s.as_str()).unwrap_or("—")).or_insert(0) += 1;
        }
        let mut bv: Vec<(usize, usize)> = basket.into_iter().filter(|x| x.1 >= 2).collect();
        bv.sort_by_key(|x| std::cmp::Reverse(x.1));
        let mut kv: Vec<(&str, usize)> = kg.into_iter().collect();
        kv.sort_by_key(|x| std::cmp::Reverse(x.1));
        out.push(json!({
            "id": format!("LG{}", gi + 1),
            "size": g.len(),
            "members": g.iter().take(30).map(|&w| wnames[w].as_str()).collect::<Vec<&str>>(),
            "basket": bv.iter().take(10).map(|(t,n)| json!({"symbol": tnames[*t], "members": n, "idf": weight[*t]})).collect::<Vec<Value>>(),
            "knownGroups": kv.iter().take(4).map(|(g,n)| json!({"group": g, "n": n})).collect::<Vec<Value>>(),
        }));
    }

    let res = json!({
        "params": {"minLinks": min_links, "minScore": min_score, "hubTok": hub_tok, "minGroup": min_group},
        "walletsTotal": n_all, "walletsInGraph": kept, "tokens": by_token.len(),
        "null": {"groupsMean": null_groups, "largestMax": null_largest, "iters": null_iters},
        "observed": {"groups": groups.len(), "largest": groups.first().map(|g| g.len()).unwrap_or(0)},
        "groups": out,
    });
    fs::write(format!("{dir}/linkall.json"), serde_json::to_string_pretty(&res).unwrap()).ok();

    let mut md = String::new();
    md.push_str("# Зв'язки між усіма гаманцями за критеріями grouppart\n\n");
    md.push_str(&format!(
        "Гаманців усього {n_all} · у графі {kept} (хаби понад {hub_tok} активів виключені) · активів {}\n\n",
        by_token.len()
    ));
    md.push_str(&format!(
        "**Нуль-тест** ({null_iters} перестановок зі збереженням степенів): випадковість дає \
         {null_groups:.1} груп, найбільша до {null_largest}. Спостережено {} груп, найбільша {}.\n\n",
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    ));
    for g in res["groups"].as_array().unwrap_or(&vec![]) {
        md.push_str(&format!("## {} — {} гаманців\n\n", g["id"].as_str().unwrap_or(""), g["size"]));
        md.push_str("| актив | членів тримає | IDF |\n|---|---:|---:|\n");
        for b in g["basket"].as_array().unwrap_or(&vec![]) {
            md.push_str(&format!(
                "| {} | {} | {:.1} |\n",
                b["symbol"].as_str().unwrap_or(""),
                b["members"],
                num(&b["idf"])
            ));
        }
        let kv: Vec<String> = g["knownGroups"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|x| format!("{}×{}", x["group"].as_str().unwrap_or(""), x["n"]))
            .collect();
        md.push_str(&format!("\nВідоме розбиття: {}\n\n", kv.join(", ")));
    }
    fs::write(format!("{dir}/linkall-report.md"), md).ok();
    println!("[linkall] готово → {dir}/linkall.json + linkall-report.md");
}
