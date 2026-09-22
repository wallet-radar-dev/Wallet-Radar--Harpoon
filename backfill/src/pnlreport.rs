//! pnlreport — зведення tokenpnl.jsonl по секторах: хто заробив, чи сидить досі,
//! за якою середньою ціною заходив і виходив.
//!
//! Окрім агрегації тут робиться те, чого не встиг зробити краул:
//!
//! СПІЛЬНА ПОДІЯ. Маркер роздачі в `tokenpnl` вимагає ≥DIST_MIN гаманців від одного
//! відправника за добу, і п'ятірку по RAIN він пропустив — а в неї ІДЕНТИЧНІ вхід
//! $0.00334 і вихід $0.00812 до копійки. П'ять однакових середніх — це одна подія на
//! п'ятьох, а не п'ять незалежних вдалих заходів. Тому пост-фактум склеюємо гаманці з
//! однаковою парою (вхід, вихід) у межах токена: від 3 штук — спільна подія, і в топ
//! «хто заробив» іде один згорнутий рядок, а не п'ять роздутих.
//!
//! ЩО ВИКЛЮЧЕНО з «чистих трейдерів»: казна емітента, біржі, отримувачі роздачі і всі
//! токени з `priceSuspect` (там «емісія × ціна» розходиться з капіталізацією понад
//! уп'ятеро — рішення за такими цифрами ухвалювати не можна).
//!
//! Args: ROOT. Env: MIN_PNL(1000) SHARED_MIN(3) TOPN(10)
//! Вихід: dataset/backfill/pnl-sectors.json + pnl-sectors-report.md

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::time::Duration;

const CG: &str = "https://api.coingecko.com/api/v3";

fn envv(root: &str, k: &str) -> String {
    let Ok(s) = fs::read_to_string(format!("{root}/.env")) else { return String::new() };
    for line in s.lines() {
        if let Some(v) = line.strip_prefix(&format!("{k}=")) {
            return v.trim().to_string();
        }
    }
    String::new()
}
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
fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

async fn get_json(c: &reqwest::Client, url: &str, key: &str) -> Option<Value> {
    for attempt in 0..5u64 {
        let mut rq = c.get(url).timeout(Duration::from_secs(30));
        if !key.is_empty() {
            rq = rq.header("x-cg-demo-api-key", key);
        }
        if let Ok(r) = rq.send().await {
            let st = r.status().as_u16();
            if st != 429 && st < 500 {
                if let Ok(v) = r.json::<Value>().await {
                    return Some(v);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(1500 * (attempt + 1))).await;
    }
    None
}

/// Ті самі правила, що й у `emission`: категорія звіряється ЦІЛКОМ, не підрядком
/// («stablecoin» як підрядок ловить «Stablecoin Protocol» і робить AAVE стейблкоїном).
fn sector_of(cats: &[String]) -> String {
    let low: Vec<String> = cats.iter().map(|c| c.to_lowercase()).collect();
    const MAP: [(&[&str], &str); 15] = [
        (&["meme"], "мемкоїн"),
        (&["stablecoins"], "стейблкоїн"),
        (&["wrapped-tokens", "bridged-tokens"], "wrapped/мостовий"),
        (&["oracle"], "оракул"),
        (
            &["real world assets (rwa)", "tokenized gold", "tokenized treasury bonds (t-bonds)",
              "tokenized stock", "tokenized commodities"],
            "RWA",
        ),
        (
            &["liquid staking tokens", "liquid restaking tokens",
              "liquid staking governance tokens", "restaking"],
            "LST/рестейкінг",
        ),
        (&["depin"], "DePIN"),
        (&["artificial intelligence (ai)", "ai agents"], "AI"),
        (&["gaming (gamefi)", "metaverse"], "GameFi"),
        (&["privacy coins"], "приватність"),
        (&["decentralized exchange (dex)"], "DEX"),
        (&["exchange-based tokens", "centralized exchange (cex)"], "біржовий токен"),
        (&["layer 2 (l2)"], "L2"),
        (&["layer 1 (l1)", "smart contract platform"], "L1"),
        (&["decentralized finance (defi)"], "DeFi"),
    ];
    for (needles, label) in MAP {
        if low.iter().any(|c| needles.contains(&c.as_str())) {
            return label.to_string();
        }
    }
    "інше".into()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let dir = format!("{root}/dataset/backfill");
    let min_pnl: f64 = envn("MIN_PNL", 1000.0);
    let shared_min: usize = envn("SHARED_MIN", 3);
    let topn: usize = envn("TOPN", 10);
    let cgkey = envv(&root, "COINGECKO_API_KEY");
    let client = reqwest::Client::builder().user_agent("wallet-radar-pnlreport/1.0").build().unwrap();

    // contract → coingecko id (за адресою, незалежно від платформи — адреси унікальні)
    let list = get_json(&client, &format!("{CG}/coins/list?include_platform=true"), &cgkey)
        .await
        .unwrap_or(Value::Null);
    let mut by_addr: HashMap<String, String> = HashMap::new();
    if let Some(a) = list.as_array() {
        for c in a {
            let Some(id) = c["id"].as_str() else { continue };
            if let Some(o) = c["platforms"].as_object() {
                for (_, addr) in o {
                    if let Some(s) = addr.as_str() {
                        if s.starts_with("0x") {
                            by_addr.entry(s.to_lowercase()).or_insert_with(|| id.to_string());
                        }
                    }
                }
            }
        }
    }

    struct Row {
        sym: String,
        wallet: String,
        pnl: f64,
        entry: f64,
        exit: Option<f64>,
        still: bool,
        from_hist: bool,
        group: String,
        shared: usize,
    }
    struct TokAgg {
        sym: String,
        contract: String,
        sector: String,
        rows: Vec<Row>,
    }

    let mut toks: Vec<TokAgg> = Vec::new();
    let mut skipped_price = 0usize;
    for line in fs::read_to_string(format!("{dir}/tokenpnl.jsonl")).unwrap_or_default().lines() {
        let Ok(d) = serde_json::from_str::<Value>(line) else { continue };
        let Some(ws) = d["wallets"].as_array() else { continue };
        if ws.is_empty() {
            continue;
        }
        if d["priceSuspect"] == json!(true) {
            skipped_price += 1;
            continue;
        }
        // склейка однакових пар (вхід, вихід) — спільна подія на кількох гаманцях
        let keyof = |w: &Value| -> String {
            format!(
                "{:.6e}|{}",
                num(&w["avgEntryUsd"]),
                w["avgExitUsd"].as_f64().map(|x| format!("{x:.6e}")).unwrap_or_default()
            )
        };
        let mut cnt: HashMap<String, usize> = HashMap::new();
        for w in ws {
            *cnt.entry(keyof(w)).or_insert(0) += 1;
        }
        let mut rows = Vec::new();
        for w in ws {
            if w["treasurySuspect"] == json!(true)
                || w["cex"] == json!(true)
                || w["viaDistribution"] == json!(true)
            {
                continue;
            }
            let pnl = num(&w["totalPnlUsd"]);
            if pnl.abs() < min_pnl {
                continue;
            }
            rows.push(Row {
                sym: d["symbol"].as_str().unwrap_or("").to_string(),
                wallet: w["wallet"].as_str().unwrap_or("").to_string(),
                pnl,
                entry: num(&w["avgEntryUsd"]),
                exit: w["avgExitUsd"].as_f64(),
                still: w["stillIn"] == json!(true),
                from_hist: w["fromCurrentTop"] != json!(true),
                group: w["group"].as_str().unwrap_or("").to_string(),
                shared: *cnt.get(&keyof(w)).unwrap_or(&1),
            });
        }
        if rows.is_empty() {
            continue;
        }
        toks.push(TokAgg {
            sym: d["symbol"].as_str().unwrap_or("").to_string(),
            contract: d["contract"].as_str().unwrap_or("").to_string(),
            sector: String::new(),
            rows,
        });
    }
    println!("[pnlreport] токенів у зведенні: {} (пропущено як priceSuspect: {skipped_price})", toks.len());

    // Сектори тягнуться по одному запиту на токен під ліміт демо-ключа — це чверть
    // години на кожен перезапуск звіту. Категорія токена не змінюється щодня, тож
    // кешуємо мапу контракт→сектор і повторні прогони стають миттєвими.
    let cache_path = format!("{dir}/pnl-token-sectors.json");
    let mut cache: HashMap<String, String> = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    println!("[pnlreport] секторів у кеші: {}", cache.len());

    let n_toks = toks.len();
    for (i, t) in toks.iter_mut().enumerate() {
        if let Some(s) = cache.get(&t.contract) {
            t.sector = s.clone();
            continue;
        }
        let sec = match by_addr.get(&t.contract) {
            Some(id) => {
                let url = format!(
                    "{CG}/coins/{id}?localization=false&tickers=false&market_data=false\
                     &community_data=false&developer_data=false&sparkline=false"
                );
                let cats: Vec<String> = get_json(&client, &url, &cgkey)
                    .await
                    .and_then(|v| v["categories"].as_array().cloned())
                    .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                tokio::time::sleep(Duration::from_millis(2300)).await;
                sector_of(&cats)
            }
            None => "інше".into(),
        };
        cache.insert(t.contract.clone(), sec.clone());
        t.sector = sec;
        if (i + 1) % 50 == 0 {
            println!("[pnlreport] сектори {}/{}", i + 1, n_toks);
            fs::write(&cache_path, serde_json::to_string(&cache).unwrap()).ok();
        }
    }
    fs::write(&cache_path, serde_json::to_string(&cache).unwrap()).ok();

    // ── ПОАСЕТНИЙ ЗВІТ: три відповіді на кожен актив ────────────────────────
    {
        let mut by_asset: Vec<(&TokAgg, f64)> = toks
            .iter()
            .map(|t| {
                let best = t.rows.iter().map(|r| r.pnl).fold(f64::MIN, f64::max);
                (t, best)
            })
            .collect();
        by_asset.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let mut md = String::new();
        md.push_str("# Хто заробив на кожному активі, чи сидить досі, за якою ціною\n\n");
        md.push_str(
            "На кожен актив — три найкращі гаманці. Виключені казна емітента, біржі та \
             отримувачі роздачі; токени з розходженням «емісія × ціна» проти капіталізації \
             понад уп'ятеро не рахувалися взагалі. «Спільна подія» означає, що кілька \
             гаманців мають ІДЕНТИЧНУ пару вхід/вихід — це один захід на всіх.\n\n",
        );
        md.push_str("| Актив | Сектор | Гаманець | P&L | сер. вхід | сер. вихід | сидить | трейдерів | вийшли |\n");
        md.push_str("|---|---|---|---:|---:|---:|---|---:|---:|\n");
        for (t, _) in &by_asset {
            let mut rs: Vec<&Row> = t.rows.iter().collect();
            rs.sort_by(|a, b| b.pnl.partial_cmp(&a.pnl).unwrap());
            let n = t.rows.len();
            let out = t.rows.iter().filter(|r| !r.still).count();
            for (k, r) in rs.iter().take(3).enumerate() {
                md.push_str(&format!(
                    "| {} | {} | `{}` | ${} | {:.6} | {} | {} | {} | {} |\n",
                    if k == 0 { t.sym.clone() } else { String::new() },
                    if k == 0 { t.sector.clone() } else { String::new() },
                    &r.wallet[..12.min(r.wallet.len())],
                    thousands(r.pnl),
                    r.entry,
                    r.exit.map(|x| format!("{x:.6}")).unwrap_or_else(|| "—".into()),
                    if r.still { "так" } else { "**НІ**" },
                    if k == 0 { n.to_string() } else { String::new() },
                    if k == 0 { format!("{:.0}%", out as f64 / n.max(1) as f64 * 100.0) } else { String::new() },
                ));
            }
        }
        fs::write(format!("{dir}/pnl-by-asset-report.md"), &md).ok();
        println!("[pnlreport] поасетний звіт → {dir}/pnl-by-asset-report.md ({} активів)", by_asset.len());
    }

    // ── агрегація по секторах ───────────────────────────────────────────────
    struct Sec {
        tokens: usize,
        traders: usize,
        pnl_pos: f64,
        pnl_neg: f64,
        still: usize,
        hist: usize,
        mult: Vec<f64>,
        top: Vec<(String, String, f64, f64, Option<f64>, bool, usize)>,
    }
    let mut secs: HashMap<String, Sec> = HashMap::new();
    for t in &toks {
        let e = secs.entry(t.sector.clone()).or_insert(Sec {
            tokens: 0,
            traders: 0,
            pnl_pos: 0.0,
            pnl_neg: 0.0,
            still: 0,
            hist: 0,
            mult: Vec::new(),
            top: Vec::new(),
        });
        e.tokens += 1;
        for r in &t.rows {
            e.traders += 1;
            if r.pnl >= 0.0 { e.pnl_pos += r.pnl } else { e.pnl_neg += r.pnl }
            if r.still {
                e.still += 1;
            }
            if r.from_hist {
                e.hist += 1;
            }
            if let Some(x) = r.exit {
                if r.entry > 0.0 && x > 0.0 {
                    e.mult.push(x / r.entry);
                }
            }
            e.top.push((r.sym.clone(), r.wallet.clone(), r.pnl, r.entry, r.exit, r.still, r.shared));
        }
    }
    for s in secs.values_mut() {
        s.top.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        s.top.truncate(topn);
    }

    let mut order: Vec<(&String, &Sec)> = secs.iter().collect();
    order.sort_by(|a, b| b.1.pnl_pos.partial_cmp(&a.1.pnl_pos).unwrap());

    let mut md = String::new();
    md.push_str("# P&L по секторах: хто заробив, чи сидить досі, за якою ціною\n\n");
    md.push_str(
        "Виключені з «чистих трейдерів»: казна емітента, біржові адреси, отримувачі роздачі \
         та всі токени, де «емісія × ціна» розходиться з капіталізацією понад уп'ятеро. \
         Колонка «спільна подія» показує, скільки гаманців мають ІДЕНТИЧНУ пару вхід/вихід — \
         від 3 це одна подія на всіх, а не кілька незалежних вдалих заходів.\n\n",
    );
    md.push_str(&format!(
        "Токенів у зведенні: **{}** · пропущено через підозрілу ціну: **{}**\n\n",
        toks.len(),
        skipped_price
    ));
    md.push_str("| Сектор | токенів | трейдерів | сумарний + | сумарний − | сидять досі | знайдені з історії | медіана вихід/вхід |\n");
    md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for (name, s) in &order {
        let mut m = s.mult.clone();
        md.push_str(&format!(
            "| {} | {} | {} | ${:.1}М | ${:.1}М | {:.0}% | {:.0}% | ×{:.2} |\n",
            name,
            s.tokens,
            s.traders,
            s.pnl_pos / 1e6,
            s.pnl_neg / 1e6,
            s.still as f64 / s.traders.max(1) as f64 * 100.0,
            s.hist as f64 / s.traders.max(1) as f64 * 100.0,
            median(&mut m)
        ));
    }
    for (name, s) in &order {
        md.push_str(&format!("\n## {name}\n\n"));
        md.push_str("| актив | гаманець | P&L | сер. вхід | сер. вихід | сидить | спільна подія |\n");
        md.push_str("|---|---|---:|---:|---:|---|---:|\n");
        for (sym, w, pnl, entry, exit, still, shared) in &s.top {
            md.push_str(&format!(
                "| {} | `{}` | ${} | {:.6} | {} | {} | {} |\n",
                sym,
                &w[..12.min(w.len())],
                thousands(*pnl),
                entry,
                exit.map(|x| format!("{x:.6}")).unwrap_or_else(|| "—".into()),
                if *still { "так" } else { "**НІ**" },
                if *shared >= shared_min { format!("⚠️ {shared} гам.") } else { "—".into() }
            ));
        }
    }
    fs::write(format!("{dir}/pnl-sectors-report.md"), &md).ok();

    let js: Vec<Value> = order
        .iter()
        .map(|(n, s)| {
            let mut m = s.mult.clone();
            json!({"sector": n, "tokens": s.tokens, "traders": s.traders,
                   "pnlPositive": s.pnl_pos, "pnlNegative": s.pnl_neg,
                   "stillInPct": s.still as f64 / s.traders.max(1) as f64 * 100.0,
                   "fromHistoryPct": s.hist as f64 / s.traders.max(1) as f64 * 100.0,
                   "medianExitOverEntry": median(&mut m)})
        })
        .collect();
    fs::write(
        format!("{dir}/pnl-sectors.json"),
        serde_json::to_string_pretty(&json!({"sectors": js, "tokens": toks.len()})).unwrap(),
    )
    .ok();
    println!("[pnlreport] готово → {dir}/pnl-sectors-report.md");
}

fn thousands(x: f64) -> String {
    let n = x.round() as i64;
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}
