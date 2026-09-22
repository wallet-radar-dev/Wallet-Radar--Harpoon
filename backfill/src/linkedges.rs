//! linkedges — друга половина критерію grouppart: зв'язки за РЕАЛЬНИМИ переказами.
//!
//! Портфельна схожість (linkall) фон не пробила. Але в їхньому розбитті портфель — це
//! фаза 2, а основа — граф ETH-переказів EOA↔EOA. Тут відтворені саме ці правила:
//!   • ребро = переказ між двома гаманцями популяції; сума < DUST_ETH не є зв'язком
//!     (1e-9 ETH — це адресне отруєння з адрес-двійників, а не стосунки);
//!   • хаби зі ступенем > HUB_DEG виключаються — вони склеюють усе в один клубок;
//!   • ≥MIN_LINKS незалежних зв'язків, щоб потрапити в групу: через ОДНЕ спам-ребро
//!     у їхній NET колись затягнуло EOA з 1.03М ETH.
//!
//! Додатково рахуємо СПІЛЬНОГО КОНТРАГЕНТА — двоє гаманців, що обидва мають справу з
//! однією рідкісною адресою. Рідкісність обов'язкова: спільна Binance не пов'язує нікого,
//! тому вага контрагента = IDF за його ступенем, а надто популярні відкидаються зовсім.
//!
//! Нуль-тест обов'язковий: тасуємо ребра зі збереженням степенів. Якщо випадковий граф
//! дає такі самі компоненти — структури немає.
//!
//! Args: ROOT EDGES_JSONL WALLETS_TXT. Env: DUST_ETH(0.001) HUB_DEG(300) MIN_LINKS(2)
//!       CP_MAX(50) MIN_GROUP(3) NULL_ITERS(10)
//! Вихід: dataset/backfill/linkedges.json + linkedges-report.md

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};

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

/// Компоненти за списком ребер із вимогою ≥min_links незалежних зв'язків.
/// Винесено окремо, щоб нуль-тест ганяв РІВНО той самий код.
fn components(pairs: &[(usize, usize)], n: usize, min_links: usize, min_group: usize) -> Vec<Vec<usize>> {
    // «Незалежність» тут = різні контрагенти: рахуємо, скільки РІЗНИХ сусідів має вузол
    // усередині графа. Вузол з єдиним зв'язком до групи не приєднується.
    let mut nb: Vec<HashSet<usize>> = vec![HashSet::new(); n];
    for (a, b) in pairs {
        nb[*a].insert(*b);
        nb[*b].insert(*a);
    }
    let mut uf = Uf::new(n);
    for (a, b) in pairs {
        if nb[*a].len() >= min_links && nb[*b].len() >= min_links {
            uf.union(*a, *b);
        }
    }
    let mut comp: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        if nb[i].len() >= min_links {
            let r = uf.find(i);
            comp.entry(r).or_default().push(i);
        }
    }
    comp.into_values().filter(|c| c.len() >= min_group).collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| "/opt/wallet-tracker".into());
    let edges_path = args.next().expect("вкажи шлях до eth-edges2.jsonl");
    let wallets_path = args.next().expect("вкажи шлях до списку гаманців");
    let dir = format!("{root}/dataset/backfill");
    let dust: f64 = envn("DUST_ETH", 0.001);
    let hub_deg: usize = envn("HUB_DEG", 300);
    let min_links: usize = envn("MIN_LINKS", 2);
    let cp_max: usize = envn("CP_MAX", 50);
    let min_group: usize = envn("MIN_GROUP", 3);
    let null_iters: usize = envn("NULL_ITERS", 10);

    let pop: Vec<String> = fs::read_to_string(&wallets_path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| l.starts_with("0x"))
        .collect();
    let id: HashMap<&str, usize> = pop.iter().enumerate().map(|(i, w)| (w.as_str(), i)).collect();
    let n = pop.len();
    println!("[linkedges] популяція: {n}");

    // ── читаємо ребра ───────────────────────────────────────────────────────
    let f = fs::File::open(&edges_path).expect("edges jsonl");
    let mut direct: HashSet<(usize, usize)> = HashSet::new();
    // контрагент поза популяцією → хто з популяції з ним мав справу
    let mut shared: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut lines = 0usize;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(w) = v["w"].as_str() else { continue };
        let w = w.to_lowercase();
        let Some(&wi) = id.get(w.as_str()) else { continue };
        lines += 1;
        for cp in v["cps"].as_array().unwrap_or(&vec![]) {
            let Some(a) = cp["a"].as_str() else { continue };
            let a = a.to_lowercase();
            // ДРІБ'ЯЗОК НЕ Є ЗВ'ЯЗКОМ
            if num(&cp["i"]) + num(&cp["o"]) < dust {
                continue;
            }
            match id.get(a.as_str()) {
                Some(&ai) if ai != wi => {
                    direct.insert((wi.min(ai), wi.max(ai)));
                }
                None => {
                    shared.entry(a).or_default().insert(wi);
                }
                _ => {}
            }
        }
    }
    println!(
        "[linkedges] записів у краулі: {lines} · прямих ребер: {} · зовнішніх контрагентів: {}",
        direct.len(),
        shared.len()
    );

    // ── спільні контрагенти, лише рідкісні ──────────────────────────────────
    let mut via: HashMap<(usize, usize), usize> = HashMap::new();
    let mut rare_cps = 0usize;
    for (_, set) in shared.iter() {
        // Спільна Binance не пов'язує нікого: контрагент, з яким має справу пів
        // популяції, — це інфраструктура, а не зв'язок.
        if set.len() < 2 || set.len() > cp_max {
            continue;
        }
        rare_cps += 1;
        let v: Vec<usize> = set.iter().copied().collect();
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                *via.entry((v[i].min(v[j]), v[i].max(v[j]))).or_insert(0) += 1;
            }
        }
    }
    println!("[linkedges] рідкісних спільних контрагентів (2..{cp_max}): {rare_cps}");

    // Пара стає ребром: прямий переказ АБО ≥2 спільні рідкісні контрагенти.
    let mut pairs: Vec<(usize, usize)> = direct.iter().copied().collect();
    let mut via_edges = 0usize;
    for (k, c) in &via {
        if *c >= min_links && !direct.contains(k) {
            pairs.push(*k);
            via_edges += 1;
        }
    }
    println!("[linkedges] ребер за спільними контрагентами: {via_edges} · разом: {}", pairs.len());

    // ── хаби геть ───────────────────────────────────────────────────────────
    let mut deg = vec![0usize; n];
    for (a, b) in &pairs {
        deg[*a] += 1;
        deg[*b] += 1;
    }
    let hubs: Vec<bool> = deg.iter().map(|d| *d > hub_deg).collect();
    let nh = hubs.iter().filter(|x| **x).count();
    pairs.retain(|(a, b)| !hubs[*a] && !hubs[*b]);
    println!("[linkedges] хабів (ступінь >{hub_deg}) виключено: {nh} · ребер лишилось: {}", pairs.len());

    // ── компоненти ──────────────────────────────────────────────────────────
    let mut groups = components(&pairs, n, min_links, min_group);
    groups.sort_by_key(|g| std::cmp::Reverse(g.len()));
    println!(
        "[linkedges] груп (≥{min_group}): {} · найбільша: {}",
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    );

    // ── нуль-тест: тасування ребер зі збереженням степенів ──────────────────
    let mut seed = 0x5DEECE66Du64 | 1;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let (mut ng, mut nl) = (0.0f64, 0usize);
    let mut shuf = pairs.clone();
    for _ in 0..null_iters {
        let m = shuf.len();
        if m > 1 {
            for _ in 0..(m * 8) {
                let (i, j) = ((rnd() as usize) % m, (rnd() as usize) % m);
                let (a1, b1) = shuf[i];
                let (a2, b2) = shuf[j];
                if a1 == b2 || a2 == b1 {
                    continue;
                }
                shuf[i] = (a1.min(b2), a1.max(b2));
                shuf[j] = (a2.min(b1), a2.max(b1));
            }
        }
        let g = components(&shuf, n, min_links, min_group);
        ng += g.len() as f64;
        nl = nl.max(g.iter().map(|x| x.len()).max().unwrap_or(0));
    }
    ng /= null_iters.max(1) as f64;
    println!(
        "[linkedges] НУЛЬ: {ng:.1} груп, найбільша до {nl} · спостережено {} груп, найбільша {}",
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    );

    // ── опис ────────────────────────────────────────────────────────────────
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

    let out: Vec<Value> = groups
        .iter()
        .take(30)
        .enumerate()
        .map(|(i, g)| {
            let mut kg: HashMap<&str, usize> = HashMap::new();
            for &w in g {
                *kg.entry(known.get(&pop[w]).map(|s| s.as_str()).unwrap_or("—")).or_insert(0) += 1;
            }
            let mut kv: Vec<(&str, usize)> = kg.into_iter().collect();
            kv.sort_by_key(|x| std::cmp::Reverse(x.1));
            json!({
                "id": format!("EG{}", i + 1),
                "size": g.len(),
                "members": g.iter().take(25).map(|&w| pop[w].as_str()).collect::<Vec<&str>>(),
                "knownGroups": kv.iter().take(4).map(|(g,n)| json!({"group": g, "n": n})).collect::<Vec<Value>>(),
            })
        })
        .collect();

    let res = json!({
        "params": {"dustEth": dust, "hubDeg": hub_deg, "minLinks": min_links, "cpMax": cp_max},
        "population": n, "directEdges": direct.len(), "viaEdges": via_edges,
        "null": {"groupsMean": ng, "largestMax": nl, "iters": null_iters},
        "observed": {"groups": groups.len(), "largest": groups.first().map(|g| g.len()).unwrap_or(0)},
        "groups": out,
    });
    fs::write(format!("{dir}/linkedges.json"), serde_json::to_string_pretty(&res).unwrap()).ok();

    let mut md = String::new();
    md.push_str("# Зв'язки за реальними переказами (критерій grouppart)\n\n");
    md.push_str(&format!(
        "Популяція {n} · прямих ребер {} · ребер за спільними рідкісними контрагентами {via_edges}\n\n\
         Правила: дріб'язок <{dust} ETH не є зв'язком · хаби зі ступенем >{hub_deg} виключені · \
         потрібно ≥{min_links} незалежні зв'язки · спільний контрагент зараховується лише якщо \
         з ним має справу не більше {cp_max} гаманців популяції.\n\n",
        direct.len()
    ));
    md.push_str(&format!(
        "**Нуль-тест** ({null_iters} тасувань ребер зі збереженням степенів): випадковість дає \
         {ng:.1} груп, найбільша до {nl}. Спостережено {} груп, найбільша {}.\n\n",
        groups.len(),
        groups.first().map(|g| g.len()).unwrap_or(0)
    ));
    for g in res["groups"].as_array().unwrap_or(&vec![]) {
        let kv: Vec<String> = g["knownGroups"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|x| format!("{}×{}", x["group"].as_str().unwrap_or(""), x["n"]))
            .collect();
        md.push_str(&format!(
            "## {} — {} гаманців · відоме розбиття: {}\n\n",
            g["id"].as_str().unwrap_or(""),
            g["size"],
            kv.join(", ")
        ));
        for m in g["members"].as_array().unwrap_or(&vec![]).iter().take(10) {
            md.push_str(&format!("- `{}`\n", m.as_str().unwrap_or("")));
        }
        md.push('\n');
    }
    fs::write(format!("{dir}/linkedges-report.md"), md).ok();
    println!("[linkedges] готово → {dir}/linkedges.json + linkedges-report.md");
}
