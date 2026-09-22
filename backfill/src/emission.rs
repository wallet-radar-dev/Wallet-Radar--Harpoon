//! emission — які активи з топ-1000 за капіталізацією мають ВИКУПЛЕНУ емісію.
//!
//! Питання: скільки % емісії сидить у топ-N холдерів. «Викуплена» ≠ «лежить у топі»:
//! спалене з обігу вийшло, а контракти (стейкінг, вестинг, мости, пули) і біржові
//! гарячі гаманці — це НЕ куплені кимось позиції. Тому рахуємо три частки окремо:
//!   top_share  — уся емісія в топ-N (без спаленого), верхня межа концентрації
//!   cex_share  — з них на біржових адресах (кастодіальне, чуже)
//!   eoa_share  — на звичайних гаманцях = власне «викуплено» приватними власниками
//!
//! Знаменник — total_supply МІНУС спалене: концентрацію рахуємо від обігу, а не від
//! номінальної емісії, інакше токен із 50% у 0x…dead виглядає розподіленим.
//!
//! Джерела: CoinGecko (топ-1000 за mcap + ethereum-контракт + категорії) та Blockscout
//! eth (`/api/v2/tokens/{c}` + `/holders`, безключовий). Сектор тягнемо ЛИШЕ для тих,
//! хто пройшов поріг, — це економить ~900 запитів під ліміт 30/хв демо-ключа.
//!
//! Args: ROOT. Env: UNIVERSE(1000) TOPN(1000) MIN_SHARE(40) CONC(6)
//! Вихід: dataset/backfill/emission-buyback.json + emission-report.md

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

const BS: &str = "https://eth.blockscout.com";
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

/// Спалені адреси: з обігу вийшли, у знаменник не входять.
fn is_burn(a: &str) -> bool {
    // Перелік спалених адрес — у базі, тег `burn`. Їхній баланс вийшов з обігу
    // і в знаменник емісії не входить.
    wr_store::snapshot().wallet_has_tag("eth", a, "burn")
}

/// Кастодіальні/біржові холдери за тегами Blockscout. Токен на гарячому гаманці біржі
/// належить її клієнтам гуртом — це не «хтось викупив емісію».
const CEX_SLUGS: [&str; 24] = [
    "binance", "coinbase", "kraken", "okx", "bybit", "kucoin", "huobi", "htx", "bitfinex",
    "upbit", "bithumb", "mexc", "bitget", "gate", "crypto-com", "bitstamp", "gemini",
    "poloniex", "bingx", "paribu", "btcturk", "exchange", "hot-wallet", "cold-wallet",
];

fn holder_kind(is_contract: bool, name: &str, tags: &Value) -> &'static str {
    let mut hay = name.to_lowercase();
    if let Some(arr) = tags.as_array() {
        for t in arr {
            hay.push(' ');
            hay.push_str(&t["slug"].as_str().unwrap_or("").to_lowercase());
            hay.push(' ');
            hay.push_str(&t["name"].as_str().unwrap_or("").to_lowercase());
        }
    }
    if CEX_SLUGS.iter().any(|s| hay.contains(s)) {
        return "cex";
    }
    if is_contract {
        return "contract";
    }
    "eoa"
}

async fn get_json(c: &reqwest::Client, url: &str, key: &str) -> Option<Value> {
    for attempt in 0..5u64 {
        let mut rq = c.get(url).timeout(Duration::from_secs(30));
        if url.starts_with(CG) && !key.is_empty() {
            rq = rq.header("x-cg-demo-api-key", key);
        }
        match rq.send().await {
            Ok(r) => {
                let st = r.status().as_u16();
                if st == 429 || st >= 500 {
                    tokio::time::sleep(Duration::from_millis(1200 * (attempt + 1))).await;
                    continue;
                }
                if let Ok(v) = r.json::<Value>().await {
                    return Some(v);
                }
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
    }
    None
}

/// next_page_params → query-рядок. Blockscout міняє набір ключів між версіями,
/// тому перекладаємо що прийшло, а не фіксований список.
fn qs(next: &Value) -> String {
    let Some(o) = next.as_object() else { return String::new() };
    let parts: Vec<String> = o
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{k}={s}")
        })
        .collect();
    if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) }
}

struct Tok {
    rank: u64,
    id: String,
    sym: String,
    name: String,
    contract: String,
    mcap: f64,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let universe: usize = envn("UNIVERSE", 1000);
    let topn: usize = envn("TOPN", 1000);
    let min_share: f64 = envn("MIN_SHARE", 40.0);
    let conc: usize = envn("CONC", 6);
    let cgkey = envv(&root, "COINGECKO_API_KEY");

    let client = reqwest::Client::builder()
        .user_agent("wallet-radar-emission/1.0")
        .build()
        .expect("client");

    // ── 1. Всесвіт: топ-N за капіталізацією ─────────────────────────────────
    let mut markets: Vec<Value> = Vec::new();
    let pages = universe.div_ceil(250);
    for p in 1..=pages {
        let url = format!(
            "{CG}/coins/markets?vs_currency=usd&order=market_cap_desc&per_page=250&page={p}"
        );
        match get_json(&client, &url, &cgkey).await {
            Some(Value::Array(a)) => markets.extend(a),
            _ => eprintln!("[emission] сторінка markets {p} не відповіла"),
        }
        tokio::time::sleep(Duration::from_millis(2200)).await;
    }
    markets.truncate(universe);
    println!("[emission] всесвіт: {} активів за капіталізацією", markets.len());

    // ── 2. ethereum-контракти для них ───────────────────────────────────────
    let list = get_json(&client, &format!("{CG}/coins/list?include_platform=true"), &cgkey)
        .await
        .unwrap_or(Value::Null);
    let mut plat: HashMap<String, String> = HashMap::new();
    if let Some(a) = list.as_array() {
        for c in a {
            let (Some(id), Some(addr)) = (c["id"].as_str(), c["platforms"]["ethereum"].as_str())
            else {
                continue;
            };
            if addr.starts_with("0x") && addr.len() == 42 {
                plat.insert(id.to_string(), addr.to_lowercase());
            }
        }
    }

    let toks: Vec<Tok> = markets
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let id = m["id"].as_str()?;
            let contract = plat.get(id)?.clone();
            Some(Tok {
                rank: (i + 1) as u64,
                id: id.to_string(),
                sym: m["symbol"].as_str().unwrap_or("").to_uppercase(),
                name: m["name"].as_str().unwrap_or("").to_string(),
                contract,
                mcap: num(&m["market_cap"]),
            })
        })
        .collect();
    println!(
        "[emission] з них ERC-20 на Ethereum: {} (решта — нативні монети інших мереж)",
        toks.len()
    );

    // ── 3. Холдери з Blockscout ─────────────────────────────────────────────
    let sem = Arc::new(Semaphore::new(conc));
    let done = Arc::new(AtomicUsize::new(0));
    let total = toks.len();
    let toks = Arc::new(toks);

    let mut futs = Vec::new();
    for idx in 0..total {
        let (sem, done, client, toks) = (sem.clone(), done.clone(), client.clone(), toks.clone());
        futs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.ok()?;
            let t = &toks[idx];
            let info = get_json(&client, &format!("{BS}/api/v2/tokens/{}", t.contract), "").await?;
            let supply_raw = num(&info["total_supply"]);
            let dec = num(&info["decimals"]).max(0.0) as i32;
            let holders_total = num(&info["holders_count"]);
            if supply_raw <= 0.0 {
                return None;
            }

            // топ-N холдерів, посторінково (Blockscout віддає по 50, вже відсортовані)
            let mut rows: Vec<(String, f64, &'static str)> = Vec::new();
            let mut burned = 0.0f64;
            let mut params = String::new();
            for _ in 0..(topn.div_ceil(50) + 1) {
                if rows.len() >= topn {
                    break;
                }
                let url = format!("{BS}/api/v2/tokens/{}/holders{params}", t.contract);
                let Some(d) = get_json(&client, &url, "").await else { break };
                let Some(items) = d["items"].as_array() else { break };
                if items.is_empty() {
                    break;
                }
                for it in items {
                    let a = it["address"]["hash"].as_str().unwrap_or("").to_lowercase();
                    let v = num(&it["value"]);
                    if is_burn(&a) || a == t.contract {
                        burned += v;
                        continue;
                    }
                    let kind = holder_kind(
                        it["address"]["is_contract"].as_bool().unwrap_or(false),
                        it["address"]["name"].as_str().unwrap_or(""),
                        &it["address"]["metadata"]["tags"],
                    );
                    rows.push((a, v, kind));
                    if rows.len() >= topn {
                        break;
                    }
                }
                let next = &d["next_page_params"];
                if next.is_null() {
                    break;
                }
                params = qs(next);
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
            if rows.is_empty() {
                return None;
            }

            // Обіг = емісія мінус спалене. Спалене могло бути й поза топ-N, але тоді
            // воно й у чисельник не потрапило — оцінка лишається консервативною.
            //
            // OKB зловив 665%: його «спалення» зменшило totalSupply(), не чіпаючи балансів,
            // тож сума холдерів БІЛЬША за емісію. Там, де контракт суперечить сам собі,
            // знаменником стає спостережена сума, а актив позначається suspect.
            let fetched: f64 = rows.iter().map(|r| r.1).sum();
            let nominal = (supply_raw - burned).max(1.0);
            let suspect = fetched > nominal * 1.02;
            let denom = if suspect { fetched } else { nominal };
            // Фаза 0.2: частка емісії — єдина формула зі СЛОВНИКА (wr_dict::emission_share).
            // Байт-паритет: для denom>0 emission_share(v,denom) ≡ v/denom (та сама IEEE-операція),
            // а denom тут завжди >0 (nominal=(supply-burned).max(1.0)≥1, fetched>0). Порядок
            // (…*100.0).max(0.0) збережено — результат бітово ідентичний старому `v/denom*100`.
            let pct = |v: f64| (wr_dict::emission_share(v, denom) * 100.0).max(0.0);

            let mut cum = 0.0;
            let mut to40 = 0usize;
            for (i, r) in rows.iter().enumerate() {
                cum += r.1;
                if to40 == 0 && pct(cum) >= 40.0 {
                    to40 = i + 1;
                }
            }
            let sum_kind = |k: &str| -> f64 { rows.iter().filter(|r| r.2 == k).map(|r| r.1).sum() };
            let n_kind = |k: &str| -> usize { rows.iter().filter(|r| r.2 == k).count() };

            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 25 == 0 {
                println!("[emission] опрацьовано {n}/{total}");
            }

            Some(json!({
                "rank": t.rank,
                "id": t.id,
                "symbol": t.sym,
                "name": t.name,
                "contract": t.contract,
                "marketCap": t.mcap,
                "decimals": dec,
                "holdersOnChain": holders_total,
                "holdersFetched": rows.len(),
                "supplySuspect": suspect,
                "fetchedVsSupplyPct": wr_dict::emission_share(fetched, nominal) * 100.0, // nominal≥1
                "burnedPctOfSupply": wr_dict::emission_share(burned, supply_raw) * 100.0, // supply_raw>0
                "topSharePct": pct(cum),
                "top10Pct": pct(rows.iter().take(10).map(|r| r.1).sum::<f64>()),
                "top50Pct": pct(rows.iter().take(50).map(|r| r.1).sum::<f64>()),
                "top100Pct": pct(rows.iter().take(100).map(|r| r.1).sum::<f64>()),
                "cexPct": pct(sum_kind("cex")),
                "contractPct": pct(sum_kind("contract")),
                "eoaPct": pct(sum_kind("eoa")),
                "cexHolders": n_kind("cex"),
                "contractHolders": n_kind("contract"),
                "eoaHolders": n_kind("eoa"),
                "holdersTo40": to40,
            }))
        }));
    }

    let mut out: Vec<Value> = Vec::new();
    for f in futs {
        if let Ok(Some(v)) = f.await {
            out.push(v);
        }
    }
    println!("[emission] з холдерами зібрано: {} активів", out.len());

    // ── 4. Сектор — лише для тих, хто пройшов поріг ─────────────────────────
    //
    // Поріг ставимо на EOA-частку, а не на всю верхівку. Причина суто емпірична: у топ-1000
    // холдерів >40% емісії тримає майже КОЖЕН ERC-20 — такий фільтр не фільтрує нічого.
    // «Викуплена емісія» — це те, що лежить на приватних гаманцях: не спалене, не в
    // кастодії біржі й не замкнене в контракті стейкінгу/вестингу/мосту.
    let mut hits: Vec<Value> = out
        .iter()
        .filter(|v| num(&v["eoaPct"]) > min_share && !v["supplySuspect"].as_bool().unwrap_or(false))
        .cloned()
        .collect();
    hits.sort_by(|a, b| num(&b["eoaPct"]).partial_cmp(&num(&a["eoaPct"])).unwrap());
    println!("[emission] поріг >{min_share}% пройшли: {} — тягну сектори", hits.len());

    for h in hits.iter_mut() {
        let id = h["id"].as_str().unwrap_or("").to_string();
        let url = format!(
            "{CG}/coins/{id}?localization=false&tickers=false&market_data=false\
             &community_data=false&developer_data=false&sparkline=false"
        );
        let cats: Vec<String> = get_json(&client, &url, &cgkey)
            .await
            .and_then(|v| v["categories"].as_array().cloned())
            .map(|a| a.iter().filter_map(|c| c.as_str().map(String::from)).collect())
            .unwrap_or_default();
        h["sector"] = json!(sector_of(&cats));
        h["categories"] = json!(cats);
        tokio::time::sleep(Duration::from_millis(2200)).await;
    }

    let result = json!({
        "generated": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "params": {"universe": universe, "topN": topn, "minSharePct": min_share},
        "scanned": out.len(),
        "hits": hits.len(),
        "assets": hits,
        "all": out,
    });
    let dir = format!("{root}/dataset/backfill");
    fs::write(
        format!("{dir}/emission-buyback.json"),
        serde_json::to_string_pretty(&result).unwrap_or_default(),
    )
    .ok();
    fs::write(format!("{dir}/emission-report.md"), report(&result)).ok();
    println!("[emission] готово → {dir}/emission-buyback.json + emission-report.md");
}

/// Категорії CoinGecko → один компактний сектор.
///
/// Звіряємо категорію ЦІЛКОМ, не підрядком: «stablecoin» як підрядок ловить
/// «Stablecoin Protocol» і робить AAVE стейблкоїном, а «exchange-based» ловить UNI.
/// Порядок = пріоритет: токен майже завжди в 5-10 категоріях одночасно, і перша
/// збіжність має бути найінформативнішою (LINK — оракул, не RWA).
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
        // DEX перед біржовим: CG вішає «Exchange-based Tokens» і на UNI теж, а Uniswap —
        // це DEX, не токен централізованої біржі.
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
    cats.first().cloned().unwrap_or_else(|| "інше".into())
}

fn report(r: &Value) -> String {
    let empty = vec![];
    let a = r["assets"].as_array().unwrap_or(&empty);
    let p = &r["params"];
    let mut s = String::new();
    s.push_str("# Активи з викупленою емісією\n\n");
    s.push_str(&format!(
        "Всесвіт: топ-{} за капіталізацією · глибина: топ-{} холдерів · поріг: **EOA-частка >{}%**\n\n",
        p["universe"], p["topN"], p["minSharePct"]
    ));
    s.push_str(
        "«Викуплена емісія» = частка обігу на приватних гаманцях у топі холдерів: без \
         спаленого (воно й з обігу, і зі знаменника), без кастодії бірж і без контрактів \
         (стейкінг, вестинг, мости, пули). Колонка «вся верхівка» — для контексту: там \
         сидить усе підряд, тому вона майже завжди велика.\n\n",
    );
    s.push_str(&format!(
        "Просканованo ERC-20 з холдерами: **{}** · пройшли поріг: **{}**\n\n",
        r["scanned"], r["hits"]
    ));
    s.push_str("| # | Актив | Сектор | % емісії викуплено (EOA) | вся верхівка | CEX | контракти | холдерів у топі | до 40% вистачає | усього холдерів |\n");
    s.push_str("|---:|---|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for (i, v) in a.iter().enumerate() {
        s.push_str(&format!(
            "| {} | {} ({}) | {} | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {} | {} | {} |\n",
            i + 1,
            v["symbol"].as_str().unwrap_or(""),
            v["name"].as_str().unwrap_or(""),
            v["sector"].as_str().unwrap_or("—"),
            num(&v["eoaPct"]),
            num(&v["topSharePct"]),
            num(&v["cexPct"]),
            num(&v["contractPct"]),
            v["holdersFetched"],
            v["holdersTo40"],
            num(&v["holdersOnChain"]) as u64,
        ));
    }

    let all = r["all"].as_array().unwrap_or(&empty);
    let susp: Vec<&Value> =
        all.iter().filter(|v| v["supplySuspect"].as_bool().unwrap_or(false)).collect();
    if !susp.is_empty() {
        s.push_str(
            "\n## Виключені: контракт суперечить сам собі\n\nСума балансів холдерів більша за \
             `totalSupply()` — зазвичай «спалення», що зменшило емісію, не чіпаючи балансів. \
             Частку від такої емісії рахувати не можна.\n\n",
        );
        for v in susp {
            s.push_str(&format!(
                "- **{}** ({}) — холдери тримають {:.0}% номінальної емісії\n",
                v["symbol"].as_str().unwrap_or(""),
                v["name"].as_str().unwrap_or(""),
                num(&v["fetchedVsSupplyPct"]),
            ));
        }
    }
    s
}

#[cfg(test)]
mod parity_tests {
    /// БАЙТ-ПАРИТЕТ міграції на словник (Фаза 0.2): старий вираз `v/denom*100`
    /// і новий `wr_dict::emission_share(v,denom)*100` дають БІТОВО той самий f64
    /// для всіх denom>0 (а denom тут завжди >0). Перевіряємо to_bits() рівність.
    #[test]
    fn emission_share_bit_identical_to_old_expr() {
        let denoms: [f64; 6] = [1.0, 1.5, 1e6, 528_786_477.77, 1e18, 183_987_824_878.0];
        let vals: [f64; 6] = [0.0, 1.0, 250_000.0, 1e9, 9.9e17, 1.23456789e12];
        for &denom in &denoms {
            for &v in &vals {
                let old = (v / denom * 100.0).max(0.0);
                let new = (wr_dict::emission_share(v, denom) * 100.0).max(0.0);
                assert_eq!(old.to_bits(), new.to_bits(),
                    "розбіжність при v={v}, denom={denom}: old={old} new={new}");
            }
        }
    }
    /// Стандалон-поля теж: fetched/nominal*100 і burned/supply*100.
    #[test]
    fn standalone_fields_bit_identical() {
        for &(a, b) in &[(300.0f64, 1000.0f64), (0.0, 1e18), (1e9, 528_786_477.77)] {
            assert_eq!((a / b * 100.0).to_bits(), (wr_dict::emission_share(a, b) * 100.0).to_bits());
        }
    }
}
