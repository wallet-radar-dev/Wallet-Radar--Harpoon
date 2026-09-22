//! netassets — викуплена емісія З ІНШОГО БОКУ: не «шукаємо групу серед топ-активів», а
//! «беремо доведену мережу й дивимось, ЩО вона скупила».
//!
//! Підстава для розвороту: скан топ-1000 дав єдиний сигнал, що пережив перевірку — вибірка
//! виявилась збагачена адресами групи HUB у ~100 разів. Отже інформативна не вибірка активів,
//! а сама мережа. Тут ми йдемо від неї: балансі всіх членів HUB/NET → скільки емісії кожного
//! токена лежить у мережі. Це знімає стелю «топ-1000»: якщо мережа тримає 40% дрібного токена,
//! ми його побачимо, а сканер за капіталізацією — ні.
//!
//! Пастки, які тут враховано:
//!   • спам-ейрдропи роздаються тисячам гаманців і дають фальшиву «мережеву концентрацію» —
//!     тому токен рахується лише з ненульовою ринковою ціною (`exchange_rate`);
//!   • токен, у якого власників менше за MIN_CHAIN_HOLDERS, — це не «скуплена емісія»,
//!     а нерозподілений інструмент; такі позначаються окремо, а не змішуються з рештою;
//!   • ончейн-ціна бреше (COMP ×2350 у минулих прогонах), тому USD тут — оцінка порядку,
//!     а рішення ухвалюються за ЧАСТКОЮ ЕМІСІЇ, яка від ціни не залежить.
//!
//! Args: ROOT. Env: GROUPS(HUB,NET) CONC(8) MIN_MEMBERS(2) MIN_CHAIN_HOLDERS(200) MAX_WALLETS(0=всі)
//! Вихід: dataset/backfill/netassets.json + netassets-report.md

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Мережа задається через CHAIN: без цього «де оперує мережа» лишається без відповіді,
/// бо торгова частина операції цілком може жити на L2 — саме там її шукає паралельна сесія.
fn host(chain: &str) -> Option<&'static str> {
    Some(match chain {
        "eth" => "https://eth.blockscout.com",
        "base" => "https://base.blockscout.com",
        "arbitrum" => "https://arbitrum.blockscout.com",
        "optimism" => "https://optimism.blockscout.com",
        "polygon" => "https://polygon.blockscout.com",
        "zksync" => "https://zksync.blockscout.com",
        "linea" => "https://explorer.linea.build",
        "blast" => "https://blast.blockscout.com",
        "scroll" => "https://scroll.blockscout.com",
        "mantle" => "https://mantle.blockscout.com",
        "gnosis" => "https://gnosis.blockscout.com",
        "celo" => "https://celo.blockscout.com",
        _ => return None,
    })
}

/// Etherscan chainid для перевірки коду: контракт на Ethereum і контракт на Arbitrum —
/// різні речі, тож і кеш ознаки контракту має бути окремий на кожну мережу.
fn chain_id(chain: &str) -> u64 {
    match chain {
        "base" => 8453,
        "arbitrum" => 42161,
        "optimism" => 10,
        "polygon" => 137,
        "zksync" => 324,
        "linea" => 59144,
        "blast" => 81457,
        "scroll" => 534352,
        "mantle" => 5000,
        "gnosis" => 100,
        "celo" => 42220,
        _ => 1,
    }
}

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

async fn get_json(c: &reqwest::Client, url: &str) -> Option<Value> {
    // Перший прогін загубив 12 888 гаманців із 59 138 (22%) на 429: чотирьох спроб із
    // лінійною паузою замало для багатогодинного навантаження. Експоненційний відкат.
    // 429 і 5xx — ще не весь ліміт: Etherscan під навантаженням відповідає HTTP **200** з тілом
    // {"status":"0","message":"NOTOK","result":"Max rate limit reached"}. Без перевірки ТІЛА
    // такий відказ повертався як валідний Some(v), і викликач читав його як «активів нема».
    for attempt in 0..7u64 {
        if let Ok(r) = c.get(url).timeout(Duration::from_secs(30)).send().await {
            let st = r.status().as_u16();
            if st != 429 && st < 500 {
                if let Ok(v) = r.json::<Value>().await {
                    let msg = v["message"].as_str().unwrap_or("");
                    let res = v["result"].as_str().unwrap_or("");
                    let limited = msg == "NOTOK"
                        || res.contains("rate limit")
                        || res.contains("Max calls")
                        || res.contains("Too many");
                    if !limited { return Some(v); }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(600 * (1u64 << attempt.min(5)))).await;
    }
    None
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
    let dir = format!("{root}/dataset/backfill");
    let groups: Vec<String> = std::env::var("GROUPS")
        .unwrap_or_else(|_| "HUB,NET".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let conc: usize = envn("CONC", 8);
    let min_members: usize = envn("MIN_MEMBERS", 2);
    let min_chain_holders: f64 = envn("MIN_CHAIN_HOLDERS", 200.0);
    let max_wallets: usize = envn("MAX_WALLETS", 0);
    let chain = std::env::var("CHAIN").unwrap_or_else(|_| "eth".into());
    let bs = host(&chain).expect("невідома мережа в CHAIN");
    let tag = if chain == "eth" { String::new() } else { format!("-{chain}") };
    println!("[netassets] мережа: {chain} ({bs})");

    let gm: Value =
        serde_json::from_str(&fs::read_to_string(format!("{dir}/group-membership.json")).unwrap())
            .unwrap();
    let mut members: Vec<(String, String)> = Vec::new(); // (wallet, group)
    if let Some(o) = gm["wallets"].as_object() {
        for (w, v) in o {
            let g = v["group"].as_str().unwrap_or("");
            if groups.iter().any(|x| x == g) {
                members.push((w.to_lowercase(), g.to_string()));
            }
        }
    }
    members.sort();
    if max_wallets > 0 && members.len() > max_wallets {
        members.truncate(max_wallets);
    }
    let mut per_group: HashMap<&str, usize> = HashMap::new();
    for (_, g) in &members {
        *per_group.entry(g.as_str()).or_insert(0) += 1;
    }
    println!("[netassets] членів мережі: {} {:?}", members.len(), per_group);

    // ── 0. ВІДСІВ КОНТРАКТІВ ────────────────────────────────────────────────
    //
    // Без цього кроку весь підрахунок бреше: veCRV (0x5f3b5dfeb7b2) з 850.8 млн CRV
    // значиться в розбитті як NET-WEAK, і саме він дав «мережа тримає 40% емісії CRV».
    // Правильна частка приватних гаманців — 4.4%. Так само POL, PENDLE, FRAX: там у
    // верхівці 62-98% емісії лежить у стейкінгових контрактах.
    // `wallet-kind.json` тут не рятує — по цій популяції в ньому НУЛЬ позначок contract.
    //
    // Пастка Etherscan, на якій уже горіла паралельна сесія: під лімітом він повертає
    // ТЕКСТ помилки в полі result, і перевірка «непорожній рядок = контракт» позначила
    // 653 EOA контрактами. Валідна відповідь eth_getCode ЗАВЖДИ починається з "0x".
    let code_path = format!("{dir}/wallet-iscontract{tag}.json");
    let mut iscontract: HashMap<String, bool> = fs::read_to_string(&code_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let todo: Vec<String> = members
        .iter()
        .map(|(w, _)| w.clone())
        .filter(|w| !iscontract.contains_key(w))
        .collect();
    println!("[netassets] перевірка коду: у кеші {} · до перевірки {}", iscontract.len(), todo.len());
    if !todo.is_empty() {
        let keys: Vec<String> = envv(&root, "BACKFILL_ETHERSCAN_KEYS")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        assert!(!keys.is_empty(), "BACKFILL_ETHERSCAN_KEYS порожній");
        let kc = Arc::new(AtomicUsize::new(0));
        let sem = Arc::new(Semaphore::new(conc));
        let done = Arc::new(AtomicUsize::new(0));
        let total = todo.len();
        let cid = chain_id(&chain);
        let mut futs = Vec::new();
        for w in todo {
            let (sem, kc, keys, done) = (sem.clone(), kc.clone(), keys.clone(), done.clone());
            let client = reqwest::Client::builder()
                .user_agent("wallet-radar-netassets/1.0")
                .build()
                .unwrap();
            futs.push(tokio::spawn(async move {
                let _p = sem.acquire().await.ok()?;
                for _ in 0..5 {
                    let key = &keys[kc.fetch_add(1, Ordering::Relaxed) % keys.len()];
                    let url = format!(
                        "https://api.etherscan.io/v2/api?chainid={cid}&module=proxy&action=eth_getCode\
                         &address={w}&tag=latest&apikey={key}"
                    );
                    if let Some(d) = get_json(&client, &url).await {
                        if let Some(r) = d["result"].as_str() {
                            if r.starts_with("0x") {
                                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                                if n % 2000 == 0 {
                                    println!("[netassets] код {n}/{total}");
                                }
                                return Some((w, r.len() > 2));
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                None
            }));
        }
        for f in futs {
            if let Ok(Some((w, c))) = f.await {
                iscontract.insert(w, c);
            }
        }
        fs::write(&code_path, serde_json::to_string(&iscontract).unwrap()).ok();
    }
    let n_contract = members.iter().filter(|(w, _)| *iscontract.get(w).unwrap_or(&false)).count();
    println!("[netassets] контрактів серед членів мережі: {n_contract} з {}", members.len());
    let members: Vec<(String, String)> = members
        .into_iter()
        .filter(|(w, _)| !*iscontract.get(w).unwrap_or(&false))
        .collect();
    println!("[netassets] лишилось приватних гаманців: {}", members.len());

    // ── 1. Балансі кожного члена ────────────────────────────────────────────
    let sem = Arc::new(Semaphore::new(conc));
    let done = Arc::new(AtomicUsize::new(0));
    let total = members.len();
    let members = Arc::new(members);
    let mut futs = Vec::new();
    for mi in 0..total {
        let (sem, done, client, members) = (
            sem.clone(),
            done.clone(),
            reqwest::Client::builder()
                .user_agent("wallet-radar-netassets/1.0")
                .build()
                .unwrap(),
            members.clone(),
        );
        futs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.ok()?;
            let (w, g) = &members[mi];
            let d = get_json(&client, &format!("{bs}/api/v2/addresses/{w}/token-balances")).await?;
            let mut rows: Vec<(String, String, f64, f64, f64, String)> = Vec::new();
            for it in d.as_array().unwrap_or(&vec![]) {
                let t = &it["token"];
                if t["type"].as_str().unwrap_or("") != "ERC-20" {
                    continue;
                }
                // Ціна — не для оцінки, а як ФІЛЬТР: у спам-ейрдропа її не буває.
                let rate = num(&t["exchange_rate"]);
                if rate <= 0.0 {
                    continue;
                }
                let dec = num(&t["decimals"]).clamp(0.0, 36.0);
                let amt = num(&it["value"]) / 10f64.powf(dec);
                if !amt.is_finite() || amt <= 0.0 {
                    continue;
                }
                // У цьому ендпоінті контракт лежить у `address_hash`, а не в `address`
                // (у /holders — навпаки, `address.hash`). Через це весь перший прогін
                // згорнувся в один порожній ключ: 59 138 гаманців дали «1 токен».
                let contract = match t["address_hash"].as_str().or_else(|| t["address"].as_str()) {
                    Some(a) if a.starts_with("0x") => a.to_lowercase(),
                    _ => continue,
                };
                rows.push((
                    contract,
                    t["symbol"].as_str().unwrap_or("").to_string(),
                    amt,
                    amt * rate,
                    num(&t["holders"]).max(num(&t["holders_count"])),
                    g.clone(),
                ));
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 250 == 0 {
                println!("[netassets] баланси {n}/{total}");
            }
            Some(rows)
        }));
    }

    struct Agg {
        symbol: String,
        amount: f64,
        usd: f64,
        holders_net: usize,
        hub: usize,
        chain_holders: f64,
    }
    let mut agg: HashMap<String, Agg> = HashMap::new();
    for f in futs {
        if let Ok(Some(rows)) = f.await {
            for (c, sym, amt, usd, ch, g) in rows {
                let e = agg.entry(c).or_insert(Agg {
                    symbol: sym,
                    amount: 0.0,
                    usd: 0.0,
                    holders_net: 0,
                    hub: 0,
                    chain_holders: 0.0,
                });
                e.amount += amt;
                e.usd += usd;
                e.holders_net += 1;
                if g == "HUB" {
                    e.hub += 1;
                }
                e.chain_holders = e.chain_holders.max(ch);
            }
        }
    }
    println!("[netassets] різних цінованих токенів у мережі: {}", agg.len());

    // ── 2. Емісія тих токенів, які мережа реально тримає ────────────────────
    let cands: Vec<String> = agg
        .iter()
        .filter(|(_, a)| a.holders_net >= min_members)
        .map(|(c, _)| c.clone())
        .collect();
    println!("[netassets] токенів із ≥{min_members} власниками з мережі: {}", cands.len());

    let sem = Arc::new(Semaphore::new(conc));
    let mut futs = Vec::new();
    for c in cands.clone() {
        let (sem, client) = (
            sem.clone(),
            reqwest::Client::builder()
                .user_agent("wallet-radar-netassets/1.0")
                .build()
                .unwrap(),
        );
        futs.push(tokio::spawn(async move {
            let _p = sem.acquire().await.ok()?;
            let d = get_json(&client, &format!("{bs}/api/v2/tokens/{c}")).await?;
            let dec = num(&d["decimals"]).clamp(0.0, 36.0);
            let supply = num(&d["total_supply"]) / 10f64.powf(dec);
            Some((
                c,
                supply,
                num(&d["holders_count"]),
                d["name"].as_str().unwrap_or("").to_string(),
            ))
        }));
    }

    let mut out: Vec<Value> = Vec::new();
    for f in futs {
        let Ok(Some((c, supply, chain_holders, name))) = f.await else { continue };
        let Some(a) = agg.get(&c) else { continue };
        if supply <= 0.0 {
            continue;
        }
        // Фаза 0.2: частка емісії — зі СЛОВНИКА. Порядок `(amount/supply)*100` тут той самий,
        // що старий `amount/supply*100` → байт-паритет (supply>0 гарантовано guard-ом вище).
        let share = wr_dict::emission_share(a.amount, supply) * 100.0;
        if !share.is_finite() || share <= 0.0 {
            continue;
        }
        out.push(json!({
            "contract": c,
            "symbol": a.symbol,
            "name": name,
            "emissionPctHeldByNetwork": share.min(100.0),
            "rawSharePct": share,
            "networkHolders": a.holders_net,
            "hubHolders": a.hub,
            "chainHolders": chain_holders,
            "usdEstimate": a.usd,
            "thin": chain_holders < min_chain_holders,
        }));
    }
    out.sort_by(|x, y| {
        num(&y["emissionPctHeldByNetwork"])
            .partial_cmp(&num(&x["emissionPctHeldByNetwork"]))
            .unwrap()
    });

    let res = json!({
        "groups": groups,
        "networkWallets": total,
        "tokensSeen": agg.len(),
        "assets": out,
    });
    fs::write(format!("{dir}/netassets{tag}.json"), serde_json::to_string_pretty(&res).unwrap()).ok();
    fs::write(format!("{dir}/netassets{tag}-report.md"), report(&res)).ok();
    println!("[netassets] готово → {dir}/netassets.json + netassets-report.md");
}

fn report(r: &Value) -> String {
    let empty = vec![];
    let a = r["assets"].as_array().unwrap_or(&empty);
    let mut s = String::new();
    s.push_str("# Що скупила мережа: викуплена емісія з боку китів\n\n");
    s.push_str(&format!(
        "Мережа: {:?} · гаманців {} · різних цінованих токенів {}\n\n",
        r["groups"], r["networkWallets"], r["tokensSeen"]
    ));
    s.push_str(
        "Частка емісії рахується від `totalSupply()` і від ціни не залежить — саме тому вона \
         тут головна, а USD лише орієнтир (ончейн-ціни вже ловили на похибці ×2350). Токени з \
         малою базою власників помічені як «тонкі»: там висока частка означає нерозподіленість, \
         а не скуповування.\n\n",
    );
    let solid: Vec<&Value> = a.iter().filter(|x| x["thin"] != json!(true)).collect();
    s.push_str(&format!(
        "| # | Актив | % емісії в мережі | власників з мережі | з них HUB | усього холдерів | ~USD |\n\
         |---:|---|---:|---:|---:|---:|---:|\n"
    ));
    for (i, v) in solid.iter().take(40).enumerate() {
        s.push_str(&format!(
            "| {} | {} ({}) | {:.2}% | {} | {} | {} | ${:.1}М |\n",
            i + 1,
            v["symbol"].as_str().unwrap_or(""),
            v["name"].as_str().unwrap_or(""),
            num(&v["emissionPctHeldByNetwork"]),
            v["networkHolders"],
            v["hubHolders"],
            num(&v["chainHolders"]) as u64,
            num(&v["usdEstimate"]) / 1e6,
        ));
    }
    let thin: Vec<&Value> = a.iter().filter(|x| x["thin"] == json!(true)).collect();
    if !thin.is_empty() {
        s.push_str(&format!(
            "\n## Тонкі токени ({}) — висока частка через відсутність розподілу\n\n",
            thin.len()
        ));
        for v in thin.iter().take(15) {
            s.push_str(&format!(
                "- {} — {:.1}% емісії, усього холдерів {}\n",
                v["symbol"].as_str().unwrap_or(""),
                num(&v["emissionPctHeldByNetwork"]),
                num(&v["chainHolders"]) as u64
            ));
        }
    }
    s
}

#[cfg(test)]
mod parity_tests {
    /// БАЙТ-ПАРИТЕТ (Фаза 0.2): старий `amount/supply*100` і новий
    /// `wr_dict::emission_share(amount,supply)*100` — бітово той самий f64 для supply>0
    /// (той самий порядок (a/s)*100). supply>0 гарантовано guard-ом у main.
    #[test]
    fn emission_share_pct_bit_identical_to_old_expr() {
        let sups: [f64; 6] = [1.0, 1.5, 1e6, 528_786_477.77, 1e18, 183_987_824_878.0];
        let amts: [f64; 6] = [0.0, 1.0, 250_000.0, 1e9, 9.9e17, 1.23456789e12];
        for &s in &sups {
            for &a in &amts {
                let old = a / s * 100.0;
                let new = wr_dict::emission_share(a, s) * 100.0;
                assert_eq!(old.to_bits(), new.to_bits(), "розбіжність a={a}, s={s}");
            }
        }
    }
}
