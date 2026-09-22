#![allow(dead_code, unused_imports)]
use crate::routes::*;
use crate::*;
use crate::state::*;
use crate::{dataset::Dataset, store::Store, labels::Labels, jsmath::{js_num, round_fixed}};
use axum::{body::Bytes, extract::{ConnectInfo, Path, Query, Request, State}, extract::ws::{Message, WebSocket, WebSocketUpgrade}, http::{HeaderMap, StatusCode}, middleware::Next, response::{IntoResponse, Response}, Json};
use futures::future::join_all;
use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// strategy-phases.js `run` — recompute each satellite candidate's DCA phase, one
/// at a time (each is an expensive asset-flow fan-out), persisting as it goes so a
/// crash mid-scan keeps what it already learned. Alerts only fire on a CHANGE, so
/// the first scan is silent.
pub async fn phases_tick(app: &Arc<App>) {
    let assets = phases::scan_assets(&universe::active_universe());
    for a in &assets {
        let (Some(sym), Some(contract)) = (a["sym"].as_str(), a["c"].as_str()) else { continue };
        let f = compute_asset_flow_value(contract).await;
        // strategy.rs cold-fallback, same as /api/strategy.
        let rising = f["distributed"].as_f64().unwrap_or(0.0) > 0.0;
        let s = strategy::asset_strategy(&f, rising);
        let (phase, e) = (s["phase"].clone(), s["exitRatio"].clone());
        let now = now_ms();
        if let Some(prev) = phases::record(sym, &phase, &e, now) {
            if phase.as_str() != Some(prev.as_str()) {
                if let Some(alert) = phases::notify_phase(sym, phase.as_str().unwrap_or(""), &e, now) {
                    let tg = phases::phase_telegram(&alert);
                    group_alert(app, &alert, &tg);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(6000)).await;
    }
    tracing::info!("[phases] DCA phases refreshed ({} assets)", phases::len());
}


/// ml-smart-money.js `features(addr)` — the feature vector, post-fetch.
pub async fn ml_features(addr: &str) -> Vec<f64> {
    let (flow, eth_b, base_b, arb_b) = tokio::join!(
        flow_of(addr),
        bs::token_balances("eth", addr),
        bs::token_balances("base", addr),
        bs::token_balances("arbitrum", addr),
    );
    let mut tokens = eth_b;
    tokens.extend(base_b);
    tokens.extend(arb_b);
    smart_money::sm_features(&flow, &tokens)
}


/// ml-smart-money.js `startMlWorker` tick — sample ONE anchor address per pass: extract
/// its features, label it by realized PnL, and retrain every 10 accepted samples.
pub async fn ml_tick(app: &Arc<App>) {
    ml_worker::refill_queue(ml_worker::seeds(
        &app.dataset.read_or("whale-clusters.json", json!({})),
        &app.dataset.read_or("cross-asset.json", json!({})),
        &app.dataset.read_or("alpha-feed.json", json!([])),
        &app.dataset.read_or("graph-db.json", json!({})),
    ));
    let Some(addr) = ml_worker::next_addr() else { return };

    let x = ml_features(&addr).await;
    let score = compute_wallet_score(&addr).await;
    let Some((y, realized)) = ml_worker::label_of(&score) else { return };
    if !x.iter().all(|v| v.is_finite()) {
        return;
    }

    let now = now_ms();
    let should_train = ml_worker::push_sample(&addr, &x, y, now);
    // The forward track: freeze this snapshot, and in 14 days ask whether the PnL GREW.
    ml_worker::snapshot_forward(&addr, &x, realized, now);
    if should_train {
        if let Some(line) = ml_worker::retrain(now) {
            tracing::info!("[ml] {line}");
        }
    }
}


/// ml-smart-money.js `matureForward` — re-score the snapshots whose horizon has closed.
pub async fn ml_mature_tick() {
    let due = ml_worker::due_forward(now_ms());
    // НЕ виходимо рано на порожньому due: recompute_forward_stats() наприкінці мусить крутитись
    // щотіку, інакше після рестарту fwd_stats лишається None і /api/ml-forward віддає null-fallback,
    // доки не визріє новий семпл (баг: AUC не публікувався навіть при 524 визрілих).
    // Дедуп фетчу: та сама адреса може бути due на кількох горизонтах (1/7/14/30/365) —
    // realizedUsd «зараз» однаковий для всіх, тож рахуємо score раз на адресу.
    let mut score_cache: std::collections::HashMap<String, Option<f64>> = std::collections::HashMap::new();
    for (addr, idx, horizon) in due {
        let pnl = match score_cache.get(&addr) {
            Some(v) => *v,
            None => {
                let s = compute_wallet_score(&addr).await;
                let v = s["realizedUsd"].as_f64();
                score_cache.insert(addr.clone(), v);
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                v
            }
        };
        if let Some(pnl) = pnl {
            ml_worker::mature_one(idx, horizon, pnl, now_ms());
        }
    }
    if let Some(st) = ml_worker::recompute_forward_stats() {
        tracing::info!("[ml] форвардний трек (мульти-горизонт 1/7/14/30/365д): {}", st["horizons"]);
    }
}


/// forensics-worker.js `fundersOf` — who funded this holder, across every chain we
/// index. Exchanges are excluded: a Binance hot wallet "funds" half of Ethereum.
pub async fn funders_of(addr: &str) -> Value {
    let per = join_all(bs::bs_chains().iter().map(|c| async move { (*c, bs::native_transfers(c, addr, 3).await) })).await;
    let mut by_f: serde_json::Map<String, Value> = serde_json::Map::new();
    for (chain, trs) in per {
        for t in &trs {
            let (from, to) = (t["from"].as_str().unwrap_or(""), t["to"].as_str().unwrap_or(""));
            if to != addr || from.is_empty() || forensics::is_known_cex(from) || from == addr {
                continue;
            }
            by_f.entry(from.to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("just inserted an array")
                .push(json!({"amount": t["amount"].clone(), "chain": chain}));
        }
    }
    Value::Object(by_f)
}


/// forensics-worker.js `portfolioOf` — the holder's niche token basket, across the six
/// chains that carry real capital.
pub async fn portfolio_of(addr: &str) -> Value {
    const PF_CHAINS: [&str; 6] = ["eth", "base", "arbitrum", "optimism", "polygon", "gnosis"];
    let per = join_all(PF_CHAINS.iter().map(|c| async move { Value::Array(bs::token_balances(c, addr).await) })).await;
    forensics::portfolio_of(&per)
}


/// forensics-worker.js `run()` — one full cycle over the whale assets: for each, pull the
/// top-50 holders, their funders and their portfolios, and cluster them. Persists after
/// every asset, so a crash keeps everything it already learned.
pub async fn forensics_tick(app: &Arc<App>) {
    let f = app.forensics.clone();
    if f.lock().unwrap().running {
        return;
    }
    let px = {
        let p = prices::native_prices_coord().await;
        forensics::Px {
            eth: p["eth"].as_f64().unwrap_or(0.0),
            matic: p["matic"].as_f64().unwrap_or(0.0),
            xdai: p["xdai"].as_f64().unwrap_or(1.0),
            celo: p["celo"].as_f64().unwrap_or(0.0),
        }
    };
    let whales: Vec<Value> = app
        .dataset
        .read("cmc-whale-assets.json")
        .and_then(|w| w["assets"].as_array().cloned())
        .map(|a| a.into_iter().filter(|x| (15.0..=90.0).contains(&x["top10pct"].as_f64().unwrap_or(0.0))).collect())
        .unwrap_or_default();

    // Порожній вхід — це збій джерела, а не «китів немає». Далі йти НЕ можна: цикл нікого не
    // обійде, дійде до фіналу з порожнім результатом і перезапише whale-clusters.json нулями —
    // а на ньому тримається active-groups. Саме так добрі кластери й зникали, тихо, без жодної
    // помилки в логах: файл на місці, просто порожній.
    if whales.is_empty() {
        tracing::error!(
            "[forensics] cmc-whale-assets.json не дав жодного активу (top10pct у 15–90%) — \
             цикл пропущено, whale-clusters.json НЕ чіпаємо"
        );
        return;
    }
    tracing::info!("[forensics] цикл стартує: {} активів на обхід", whales.len());

    {
        let mut st = f.lock().unwrap();
        st.running = true;
        st.cycle += 1;
        st.total = whales.len();
        st.processed = 0;
    }

    for asset in &whales {
        let (Some(symbol), Some(contract)) = (asset["symbol"].as_str(), asset["contract"].as_str()) else { continue };
        let top = bs::token_holders(contract, forensics::HOLDERS).await;

        // Concentration trend: the real top-10 share at this moment.
        let empty = vec![];
        let hs = top["holders"].as_array().unwrap_or(&empty);
        let top10: f64 = hs.iter().take(10).map(|h| h["share"].as_f64().unwrap_or(0.0)).sum();
        let top10 = jsmath::round_fixed(top10, 2);
        if top10 > 0.0 {
            history::record(&format!("conc:{symbol}"), &json!({"top10": jsmath::js_num(top10), "rank": asset["rank"].clone()}), now_ms());
        }

        let holders: Vec<Value> = hs
            .iter()
            .filter_map(|h| {
                let a = h["address"].as_str()?.to_lowercase();
                let ok = a.len() == 42
                    && a.starts_with("0x")
                    && a[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
                (ok && !forensics::is_known_cex(&a) && a != contract)
                    .then(|| json!({"addr": a, "share": jsmath::js_num(jsmath::round_fixed(h["share"].as_f64().unwrap_or(0.0), 2))}))
            })
            .take(forensics::HOLDERS)
            .collect();

        let mut hf = serde_json::Map::new();
        let mut pf = serde_json::Map::new();
        for h in &holders {
            let addr = h["addr"].as_str().unwrap_or("").to_string();
            hf.insert(addr.clone(), funders_of(&addr).await);
            let p = portfolio_of(&addr).await;

            // alpha: a whale (share ≥ 0.3%) added a token it never held before.
            let share = h["share"].as_f64().unwrap_or(0.0);
            let old = f.lock().unwrap().fingerprints.get(&addr).cloned().unwrap_or(Value::Null);
            let fresh = forensics::fresh_tokens(
                old["tokens"].as_array().unwrap_or(&empty),
                p["top"].as_array().unwrap_or(&empty),
                share,
            );
            if !fresh.is_empty() {
                let now = now_ms();
                let ev = {
                    let st = f.lock().unwrap();
                    forensics::alpha_event(&st.alpha, &addr, &fresh, symbol, share, now)
                };
                if let Some(ev) = ev {
                    let mut st = f.lock().unwrap();
                    st.alpha.insert(0, ev.clone());
                    st.alpha.truncate(100);
                    let alpha = Value::Array(st.alpha.clone());
                    drop(st);
                    app.dataset.write("alpha-feed.json", &alpha);
                    let n = history::latest("alpha")["count"].as_f64().unwrap_or(0.0) + 1.0;
                    history::record("alpha", &json!({"count": jsmath::js_num(n)}), now);
                    broadcast_msg(
                        app,
                        &json!({"type": "alpha", "address": addr, "tokens": fresh, "viaAsset": symbol, "share": ev["share"].clone(), "ts": jsmath::js_num(now as f64)}),
                    );
                    tracing::info!("[alpha] {}… → {} (via {symbol})", &addr[..10.min(addr.len())], fresh.join(","));
                }
            }
            f.lock().unwrap().fingerprints.insert(addr.clone(), p.clone());
            pf.insert(addr, p);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }

        let (groups, gf) = forensics::analyze_groups(&holders, &Value::Object(hf), &Value::Object(pf), &px);
        {
            let mut st = f.lock().unwrap();
            for (funder, addrs) in gf {
                let e = st.global_funders.entry(funder).or_default();
                e.extend(addrs);
            }
            if groups.is_empty() {
                st.results.shift_remove(symbol);
            } else {
                st.results.insert(
                    symbol.to_string(),
                    json!({
                        "symbol": symbol, "contract": contract, "rank": asset["rank"].clone(),
                        "top10pct": asset["top10pct"].clone(), "groups": groups.clone(), "ts": jsmath::js_num(now_ms() as f64),
                    }),
                );
            }
            st.processed += 1;
            st.updated_at = now_ms();
        }
        tracing::info!("[forensics] цикл {}: {symbol} → {} груп", f.lock().unwrap().cycle, groups.len());
        forensics_persist(app);
        // Кластери — після КОЖНОГО активу, а не раз на цикл. Один актив коштує ~23 хв (50 холдерів
        // × два мережевих виклики), тож повний обхід 520 активів — це 8+ діб, і кінця циклу на
        // практиці не існує: рестарт настає раніше. Поки clusters будувалися лише у фіналі,
        // whale-clusters.json не оновлювався НІКОЛИ — а з нього живе active-groups, і саме тому він
        // стояв 44 години. Перебудова чиста й дешева (сума в пам'яті), тож ціна — ніщо проти
        // «даних не буде взагалі».
        forensics_rebuild_clusters(app, &format!("після {symbol}"));
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
    }

    let count = forensics_rebuild_clusters(app, "кінець циклу").unwrap_or(0.0);
    f.lock().unwrap().running = false;
    history::record("clusters", &json!({"count": jsmath::js_num(count), "assetsWithGroups": jsmath::js_num(f.lock().unwrap().results.len() as f64)}), now_ms());
    history::persist();
    tracing::info!("[forensics] крос-активних китових кластерів: {count}");
    forensics_persist(app);
}


/// Крос-активні китові кластери з НАКОПИЧЕНИХ результатів (усе, що forensics знає на диску й у
/// пам'яті), а не лише з поточного циклу. `None` — накопичити ще нема чого; тоді файл не чіпаємо:
/// перезаписати наявні кластери порожнечею гірше, ніж не писати нічого.
pub fn forensics_rebuild_clusters(app: &Arc<App>, from: &str) -> Option<f64> {
    let f = app.forensics.clone();
    let payload = {
        let st = f.lock().unwrap();
        if st.results.is_empty() {
            tracing::error!("[forensics] перебудова ({from}): у пам'яті НУЛЬ активів — нічого будувати, файл не чіпаю");
            return None;
        }
        json!({
            "assets": st.results.values().cloned().collect::<Vec<Value>>(),
            "fingerprints": Value::Object(st.fingerprints.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            "funders": forensics_funders(&st),
        })
    };
    let mut out = whale_clusters::build_whale_clusters(&payload, &app.labels);
    let count = out["count"].as_f64().unwrap_or(0.0);
    let nodes = out["diagnostics"]["nodes"].as_i64().unwrap_or(0);

    // Нуль кластерів — це або правда, або збій. Різниця видна лише поруч із попереднім станом, тож
    // перезаписати непорожній файл порожнім ми НЕ маємо права: саме так уже зникали добрі кластери,
    // тихо, без жодної помилки в логах. Краще лишити вчорашні дані й кричати, ніж показати нуль.
    let n_assets = payload["assets"].as_array().map_or(0, Vec::len);
    if count == 0.0 {
        let prev = app.dataset.read("whale-clusters.json").and_then(|v| v["count"].as_f64()).unwrap_or(0.0);
        if prev > 0.0 {
            tracing::error!(
                "[forensics] перебудова ({from}) дала 0 кластерів (активів {n_assets}, вузлів {nodes}), а на диску лежить {prev} — файл НЕ чіпаю"
            );
            return None;
        }
    }
    tracing::info!("[forensics] кластери ({from}): активів {n_assets}, вузлів {nodes} → {count} кластерів");
    // `generated` дописуємо ТУТ, а не в build_whale_clusters: та функція чиста й звіряється з
    // Node golden-фікстурами, і годинник усередині неї зробив би тести недетермінованими. Але сам
    // штамп потрібен — whale-clusters.js його пише (whale-clusters.js:183), а порт загубив, і файл
    // лишався без жодної позначки, коли ж його востаннє будували.
    if let Some(o) = out.as_object_mut() {
        o.insert("generated".into(), json!(jsmath::js_num(now_ms() as f64)));
    }
    f.lock().unwrap().clusters = count as usize;
    app.dataset.write("whale-clusters.json", &out);
    Some(count)
}


/// Only funders that back ≥2 holders are worth persisting — one is a coincidence.
pub fn forensics_funders(st: &ForensicsState) -> Value {
    Value::Object(
        st.global_funders
            .iter()
            .filter(|(_, s)| s.len() >= 2)
            .map(|(f, s)| (f.clone(), json!(s.iter().cloned().collect::<Vec<String>>())))
            .collect(),
    )
}


pub fn forensics_persist(app: &Arc<App>) {
    let st = app.forensics.lock().unwrap();
    let mut assets: Vec<Value> = st.results.values().cloned().collect();
    assets.sort_by(|a, b| {
        let f = |v: &Value| v["groups"][0]["combinedShare"].as_f64().unwrap_or(0.0);
        f(b).partial_cmp(&f(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let out = json!({
        "generated": jsmath::js_num(st.results.len() as f64),
        "status": {
            "running": st.running, "cycle": jsmath::js_num(st.cycle as f64), "lastAsset": st.last_asset.clone(),
            "processed": jsmath::js_num(st.processed as f64), "total": jsmath::js_num(st.total as f64),
            "holders": jsmath::js_num(forensics::HOLDERS as f64), "clusters": jsmath::js_num(st.clusters as f64),
            "updatedAt": jsmath::js_num(st.updated_at as f64),
        },
        "fingerprints": Value::Object(st.fingerprints.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        "funders": forensics_funders(&st),
        "assets": assets,
    });
    drop(st);
    app.dataset.write("asset-forensics.json", &out);
    history::persist();
}


/// expand-worker.js `tick` — crawl exactly ONE node, then sleep. The gentleness is the
/// design: the free explorer limits are a budget shared with every other worker.
pub async fn expand_tick(app: &Arc<App>, since_persist: &mut u32, since_cluster: &mut u32, last_cluster: &mut i64) {
    let file = app.dataset.root().join("graph-db.json");
    let addr = {
        let mut g = app.graph.lock().unwrap();
        if g.frontier.is_empty() {
            let clusters = app.dataset.read_or("whale-clusters.json", json!({}));
            let forensics = app.dataset.read_or("asset-forensics.json", json!({}));
            g.seed(&clusters, &forensics, now_ms());
        }
        g.next_node()
    };
    let Some(addr) = addr else { return };

    let per = join_all(expand::CHAINS.iter().map(|c| async { Value::Array(bs::transfers_rich(c, &addr, 1).await) })).await;

    let mut g = app.graph.lock().unwrap();
    g.expand_from(&addr, &per, now_ms());
    g.expanded += 1;

    *since_persist += 1;
    if *since_persist >= expand::PERSIST_EVERY {
        g.persist(&file, now_ms());
        tracing::debug!("[expand] {}", g.status());
        *since_persist = 0;
    }
    *since_cluster += 1;
    // Cluster on a counter OR a timer — it costs no API calls, so it can be frequent.
    if *since_cluster >= expand::CLUSTER_EVERY || (now_ms() - *last_cluster > 300_000 && g.nodes.len() > 200) {
        let groups = g.cluster(now_ms());
        let n = groups["count"].clone();
        g.persist(&file, now_ms());
        drop(g);
        app.dataset.write("discovered-groups.json", &groups);
        tracing::info!("[expand] відкритих груп: {n}");
        *since_cluster = 0;
        *last_cluster = now_ms();
    }
}


/// Кеш балансів: повільний обхід УСІХ відстежуваних гаманців → dataset/wallet-balances.json.
///
/// Фронт не може порахувати «Сумарну вартість» сам: один баланс — це обхід 15 мереж (7-47с), а
/// гаманців 507. Спроба зробити це з браузера забивала всі 6 з'єднань на години й не давала жодного
/// результату. Тут це робить той, у кого є час: обхід іде малими партіями, з паузами, а фронт читає
/// готовий файл миттєво.
///
/// Ввічливість важливіша за швидкість: BATCH одночасних обходів + пауза між партіями. Зовнішні API
/// (Blockscout/DefiLlama/RPC) спільні з рештою воркерів, і зайва агресія тут відбирає квоту в них.
/// Keeps per-wallet detail caches (balances + score) warm for every tracked
/// wallet, so a user click is instant instead of a 30-45s cold Blockscout fetch.
/// Read-only: only populates in-memory caches, writes no dataset file.
pub async fn wallet_warm_tick(app: &Arc<App>) {
    let wallets: Vec<String> = app
        .store
        .wallets()
        .as_array()
        .map(|ws| {
            ws.iter()
                .filter_map(|x| x["address"].as_str().map(String::from))
                .filter(|a| a.starts_with("0x") && a.len() == 42)
                .collect()
        })
        .unwrap_or_default();
    for addr in wallets {
        let _ = crate::balances::wallet_balances(&addr).await;
        let _ = compute_wallet_score(&addr).await;
        let _ = flow_of(&addr).await;
        let _ = compute_wallet_netflow(&addr).await;
        let _ = compute_ml_predict(app, &addr).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

pub async fn balances_tick(app: &Arc<App>) {
    let wallets: Vec<String> = app
        .store
        .wallets()
        .as_array()
        .map(|ws| ws.iter().filter_map(|w| w["address"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    if wallets.is_empty() {
        tracing::warn!("[balances] під наглядом жодного гаманця — файл не чіпаю");
        return;
    }

    /// Скільки гаманців обходимо одночасно. Свідомо мало: кожен — це десятки зовнішніх запитів.
    const BATCH: usize = 3;
    /// Пауза між партіями — дати дихати спільним зовнішнім API.
    const GAP_MS: u64 = 1500;

    let started = now_ms();
    let mut fetched: Vec<balances_cache::Fetched> = Vec::with_capacity(wallets.len());
    let mut failed = 0usize;

    for chunk in wallets.chunks(BATCH) {
        let results = futures::future::join_all(chunk.iter().map(|addr| async move {
            let ty = nonevm::chain_type(addr);
            if ty == "unknown" {
                return balances_cache::Fetched {
                    address: addr.clone(), total_usd: None, tokens: 0, chains: 0, source: "unknown".into(),
                };
            }
            if ty != "evm" {
                let b = nonevm::non_evm_balances(addr).await;
                return balances_cache::Fetched {
                    address: addr.clone(),
                    total_usd: b["totalUsd"].as_f64(), // джерело мовчить → None, а не 0
                    tokens: b["tokens"].as_array().map_or(0, Vec::len),
                    chains: 1,
                    source: b["chain"].as_str().unwrap_or(&ty).to_string(),
                };
            }
            let bal = balances::wallet_balances(&addr.to_lowercase()).await;
            balances_cache::Fetched {
                address: addr.clone(),
                total_usd: bal["totalUsd"].as_f64(),
                tokens: bal["tokens"].as_array().map_or(0, Vec::len),
                chains: bal["chainsScanned"].as_i64().unwrap_or(0),
                source: "blockscout+defillama".into(),
            }
        }))
        .await;

        failed += results.iter().filter(|f| f.total_usd.is_none()).count();
        fetched.extend(results);
        tokio::time::sleep(std::time::Duration::from_millis(GAP_MS)).await;
    }

    let prev = app.dataset.read("wallet-balances.json").unwrap_or_else(|| json!({}));
    let next = balances_cache::merge(&prev, &fetched, &wallets, now_ms());

    // Те саме правило, що й для кластерів (d3b43e0): порожній результат не має права затерти
    // непорожній файл. Якщо цикл не дізнався НІЧОГО, а на диску лежать баланси — краще старі дані
    // й гучний лог, ніж чистий нуль, який виглядає як правда.
    let known = next["count"].as_u64().unwrap_or(0);
    let prev_known = prev["count"].as_u64().unwrap_or(0);
    if known == 0 && prev_known > 0 {
        tracing::error!("[balances] цикл не дав ЖОДНОГО балансу (гаманців {}, збоїв {failed}), а на диску лежить {prev_known} — файл НЕ чіпаю", wallets.len());
        return;
    }

    app.dataset.write("wallet-balances.json", &next);
    let secs = (now_ms() - started) / 1000;
    tracing::info!(
        "[balances] обхід завершено за {secs}с: гаманців {}, відомих балансів {known}, збоїв {failed}, сума ${:.0}",
        wallets.len(),
        next["totalUsd"].as_f64().unwrap_or(0.0)
    );
    history::record("balances", &json!({"count": known, "totalUsd": next["totalUsd"]}), now_ms());
    history::persist();
}


/// wallet-asset-scan.js `run()` — for every watched wallet, take the tokens it actually
/// ACQUIRED and hunt for coordinated whale groups in each. Accumulative: assets merge by
/// contract across runs, because the cross-asset history is what conductor detection
/// feeds on.
pub async fn wallet_scan_tick(app: &Arc<App>) {
    if wallet_scan::is_running() {
        return;
    }
    let addr_re = regex::Regex::new(r"^0x[0-9a-fA-F]{40}$").expect("compile-time constant");
    let wallets: Vec<(String, String)> = app
        .store
        .wallets()
        .as_array()
        .map(|ws| {
            ws.iter()
                .filter_map(|w| {
                    let a = w["address"].as_str()?;
                    if !addr_re.is_match(a) {
                        return None;
                    }
                    // label || address.slice(0, 8) — how the wallet is named in the alert.
                    let label = w["label"].as_str().filter(|s| !s.is_empty()).map(String::from);
                    Some((a.to_string(), label.unwrap_or_else(|| a.chars().take(8).collect())))
                })
                .collect()
        })
        .unwrap_or_default();
    if wallets.is_empty() {
        return;
    }
    let px = prices::eth_price().await;

    // contract → (symbol, which watched wallets hold it), first-seen order.
    let mut amap: indexmap::IndexMap<String, (String, indexmap::IndexSet<String>)> = indexmap::IndexMap::new();
    for (address, label) in &wallets {
        let addr = address.to_lowercase();
        let (balances, transfers) =
            tokio::join!(bs::token_balances_raw("eth", &addr), bs::transfers_rich("eth", &addr, 5));
        let (acquired, rejected) =
            wallet_scan::classify_tokens(&balances, &transfers, &addr, wallet_scan::min_position_usd());
        if !rejected.is_empty() {
            let short: String = addr.chars().take(10).collect();
            tracing::info!("[wscan] {short}…: відкинуто {} airdrop-токенів, лишилось {}", rejected.len(), acquired.len());
        }
        for tk in &acquired {
            let Some(contract) = tk["contract"].as_str() else { continue };
            let e = amap
                .entry(contract.to_string())
                .or_insert_with(|| (tk["symbol"].as_str().unwrap_or("").to_string(), indexmap::IndexSet::new()));
            e.1.insert(label.clone());
        }
    }

    wallet_scan::begin(amap.len());
    for (contract, (symbol, holders)) in &amap {
        // A 2-minute fuse per asset: a single slow token must not stall the whole scan.
        let groups = match tokio::time::timeout(
            std::time::Duration::from_secs(120),
            detect_groups_live(contract, px),
        )
        .await
        {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("[wscan] {symbol}: asset timeout");
                json!([])
            }
        };
        let empty = vec![];
        let gs = groups.as_array().unwrap_or(&empty);
        let top = gs.first().cloned().unwrap_or(Value::Null);
        let wallets_list: Vec<String> = holders.iter().cloned().collect();
        let item = json!({
            "symbol": symbol,
            "contract": contract,
            "wallets": wallets_list.clone(),
            "hasGroups": !gs.is_empty(),
            "topShare": if top["combinedShare"].is_null() { json!(0) } else { top["combinedShare"].clone() },
            "topBacker": if top["commonFunders"][0]["avgUsd"].is_null() { json!(0) } else { top["commonFunders"][0]["avgUsd"].clone() },
            "groups": gs.iter().take(2).cloned().collect::<Vec<Value>>(),
        });
        if wallet_scan::record(item.clone(), now_ms()) {
            let a = wallet_scan::trace_alert(&item, &wallets_list, now_ms());
            let tg = wallet_scan::trace_telegram(&item, &wallets_list);
            group_alert(app, &a, &tg);
        }
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    }
    wallet_scan::finish();

    let scanned = wallet_scan::get();
    let assets = scanned["assets"].as_array().cloned().unwrap_or_default();
    let with_groups = assets.iter().filter(|a| a["hasGroups"] == json!(true)).count();
    tracing::info!("[wscan] активів проскановано: {}, з китовими групами: {with_groups}", assets.len());
}


/// backtest.js `run()` — replay each whale asset's transfer history into entry events and
/// score them on a fixed forward horizon against ETH.
///
/// The intermediate result is deliberately NOT published: the old code rewrote `signal`
/// after every asset, so the UI could not tell a 36-of-520 run from a finished one.
/// /api/backtest now serves the last COMPLETE run, and exposes progress via status.running.
pub async fn backtest_tick(app: &Arc<App>) {
    let Some(whales) = app.dataset.read("cmc-whale-assets.json") else { return };
    let assets: Vec<Value> = whales["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|x| {
                    let p = x["top10pct"].as_f64().unwrap_or(0.0);
                    (15.0..=90.0).contains(&p)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if assets.is_empty() {
        return;
    }

    let total = assets.len();
    let mut all_rows: Vec<Value> = Vec::new();
    let mut per_asset: Vec<Value> = Vec::new();
    let (mut unresolved, mut skipped) = (0i64, 0i64);

    for a in &assets {
        let contract = a["contract"].as_str().unwrap_or("").to_lowercase();
        let (series, bench, transfers) = tokio::join!(
            prices::price_chart_daily(&contract, "eth"),
            prices::benchmark_chart_daily(),
            bs::token_transfers_global(&contract, backtest::TRANSFER_PAGES)
        );
        // Too little price history to measure a 30-day horizon honestly.
        if series.len() < 30 || bench.is_empty() || transfers.is_empty() {
            skipped += 1;
            tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
            continue;
        }
        let events = backtest::build_entry_events(&transfers, &series, &contract, backtest::WHALE_USD, &app.labels);
        if events.is_empty() {
            skipped += 1;
            tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
            continue;
        }
        let f = backtest::forward_rows(&events, &series, &bench, backtest::HORIZON_DAYS, now_ms() as f64);
        unresolved += f.unresolved;
        // The JS computes noPrice and drops it; log it, because a high count means the
        // chart does not reach far enough back and the sample is quietly thinner.
        if f.no_price > 0 {
            tracing::debug!("[backtest] {}: {} подій без ціни в допуску", a["symbol"], f.no_price);
        }
        let exc: Vec<f64> = f.rows.iter().map(|r| r["excess"].as_f64().unwrap_or(0.0)).collect();
        per_asset.push(json!({
            "symbol": a["symbol"].clone(),
            "rank": a["rank"].clone(),
            "events": js_num(events.len() as f64),
            "resolved": js_num(f.rows.len() as f64),
            "meanExcess": backtest::mean(&exc),
            "medianExcess": backtest::median(&exc),
        }));
        all_rows.extend(f.rows);
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    }

    let signal = backtest::aggregate(
        &all_rows,
        &[
            ("unresolvedEvents", js_num(unresolved as f64)),
            ("assetsScored", js_num(per_asset.len() as f64)),
            ("assetsSkipped", js_num(skipped as f64)),
        ],
    );
    per_asset.sort_by(|x, y| {
        let f = |v: &Value| v["meanExcess"].as_f64().unwrap_or(-999.0);
        f(y).partial_cmp(&f(x)).unwrap_or(std::cmp::Ordering::Equal)
    });

    app.dataset.write(
        "backtest.json",
        &json!({
            "generated": js_num(now_ms() as f64),
            "status": {"running": false, "processed": js_num(total as f64), "total": js_num(total as f64)},
            "method": backtest::method(),
            "signal": signal.clone(),
            "assets": per_asset.clone(),
        }),
    );
    let ci = signal["hitRateCi95"]
        .as_array()
        .map(|c| format!("{}–{}", c[0], c[1]))
        .unwrap_or_else(|| "—".into());
    tracing::info!(
        "[backtest] завершено: {} подій входу на {} активах · hit-rate {}% (CI {ci}) · надлишок над ETH {}%",
        signal["samples"], per_asset.len(), signal["hitRate"], signal["meanExcess"]
    );
}


/// conductor-finder.js `rejectReason(addr)` — why this candidate is NOT a conductor,
/// or None if it survives. Precision beats speed here, so every candidate costs one
/// metadata fetch. No attribution at all is not a rejection: the behavioural filter
/// has already done the real work, and most true conductors are unlabelled.
pub async fn conductor_reject_reason(app: &Arc<App>, addr: &str) -> Option<String> {
    if let Some(lab) = app.labels.infra_reason(addr) {
        return Some(lab);
    }
    let m = bs::address_meta("eth", addr).await?;
    if m["isContract"] == json!(true) {
        return Some("contract".into());
    }
    let name = m["name"].as_str().unwrap_or("");
    if meta_infra_re().is_match(name) {
        return Some(format!("meta:{name}"));
    }
    let tag = m["tags"].as_array()?.iter().filter_map(|t| t.as_str()).find(|t| meta_infra_re().is_match(t))?;
    Some(format!("meta-tag:{tag}"))
}


/// conductor-finder.js `buildConductors` — aggregate the funder graph out of the two
/// forensics datasets, keep the coordinators who reuse the SAME wallets across assets,
/// then verify each one on-chain. Writes conductors.json (served verbatim by
/// /api/conductors) and alerts once per newly-seen 4+-asset conductor.
pub async fn conductors_tick(app: &Arc<App>) {
    // The candidate pool is the union of both forensics files, forensics first.
    let mut assets: Vec<Value> = Vec::new();
    for file in ["asset-forensics.json", "wallet-asset-scan.json"] {
        if let Some(v) = app.dataset.read(file) {
            if let Some(a) = v["assets"].as_array() {
                assets.extend(a.iter().cloned());
            }
        }
    }
    let cands = conductor_finder::build_conductor_candidates(&assets, &app.labels);
    let dropped = cands.dropped;

    let mut conductors: Vec<Value> = Vec::new();
    for mut c in cands.list {
        let Some(funder) = c["funder"].as_str().map(String::from) else { continue };
        if conductor_reject_reason(app, &funder).await.is_some() {
            continue;
        }
        c["verification"] = conductor_finder::verification(&c);
        conductors.push(c);
        if conductors.len() >= 50 {
            break;
        }
    }

    let now = now_ms();
    let result = json!({
        "generated": js_num(now as f64),
        "count": js_num(conductors.len() as f64),
        "droppedAsDispensers": js_num(dropped as f64),
        "conductors": conductors,
    });
    app.dataset.write("conductors.json", &result);

    // Alert once per conductor, not once per cycle: `pushed` is seeded from the file we
    // just read so a restart does not re-announce everyone.
    for c in &conductors {
        let (Some(funder), Some(n)) = (c["funder"].as_str(), c["assetCount"].as_i64()) else { continue };
        if n < 4 || !app.pushed_conductors.lock().unwrap().insert(funder.to_string()) {
            continue;
        }
        let syms: Vec<&str> = c["assets"].as_array().map_or(vec![], |a| a.iter().take(6).filter_map(|s| s.as_str()).collect());
        let usd = c["maxBackerUsd"].as_f64().unwrap_or(0.0);
        let backer = if usd > 0.0 { format!(" · бекер до ${}M", jsmath::to_fixed(usd / 1e6, 2)) } else { String::new() };
        let short: String = funder.chars().take(10).collect();
        let holders = c["holderCount"].clone();
        let tg = format!(
            "🎼 <b>Новий ДИРИГЕНТ</b> <code>{short}…</code>\nКоординує <b>{n} активів</b> ({})\nФінансує {holders} холдерів{backer}",
            syms.join(", ")
        );
        group_alert(
            app,
            &json!({
                "type": "group-alert",
                "category": "conductor",
                "icon": "🎼",
                "title": format!("Новий диригент {short}…"),
                "body": format!("Координує {n} активів ({}) · фінансує {holders} холдерів{backer}", syms.join(", ")),
                "address": funder,
                "assets": c["assets"].clone(),
                "valueUsd": js_num(usd),
                "ts": now,
            }),
            &tg,
        );
    }
    tracing::info!("[conductors] верифікованих диригентів: {} (відкинуто як диспенсерів: {dropped})", conductors.len());
}


/// conductor.js `startConductorMonitor` run() — poll every watchlist wallet labelled
/// "диригент" for new outgoing funding across the five chains. The persisted state is
/// what makes this safe to restart: it dedupes transfers and stays silent on an
/// address's first scan.
pub async fn conductor_monitor_tick(app: &Arc<App>) {
    let wallets = app.store.wallets();
    let addr_re = regex::Regex::new(r"^0x[0-9a-fA-F]{40}$").expect("compile-time constant");
    let conductors: Vec<(String, String)> = wallets
        .as_array()
        .map(|ws| {
            ws.iter()
                .filter_map(|w| {
                    let a = w["address"].as_str()?;
                    let l = w["label"].as_str().unwrap_or("");
                    (addr_re.is_match(a) && conductor::is_conductor_label(l)).then(|| (a.to_lowercase(), l.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    if conductors.is_empty() {
        return;
    }

    let file = app.dataset.root().join("conductor-state.json");
    let mut st = conductor::MonitorState::load(&file);
    let px = prices::eth_price().await;
    let mut alerts = 0usize;

    for (addr, label) in &conductors {
        let first = !st.init.contains(addr);
        for chain in conductor::CHAINS {
            let transfers = bs::native_transfers(chain, addr, 2).await;
            for ev in conductor::new_fundings(chain, addr, &transfers, px, first, &mut st.seen) {
                // Suppress internal redistribution: a conductor funding a wallet in
                // its OWN family (funding-graph component) is not a market signal.
                if crate::family_map::same_family(app.dataset.root(), addr, ev["to"].as_str().unwrap_or("")) {
                    continue;
                }
                let a = conductor::funding_alert(label, &ev, now_ms());
                let tg = conductor::funding_telegram(label, &ev);
                group_alert(app, &a, &tg);
                alerts += 1;
            }
        }
        st.init.insert(addr.clone());
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    st.persist(&file);
    tracing::info!("[conductor] моніторинг: {} диригентів, нових фінансувань {alerts}", conductors.len());
}


/// behavior-monitor.js `tick` — one pass: for every tracked wallet with an armed
/// baseline, recompute the flow snapshot, diff it against the baseline, and on any
/// change push an alert (ring + broadcast) and re-arm the baseline. Gated behind
/// ENABLE_BEHAVIOR_WORKER (off by default — shares the data.json baseline writes).
pub async fn behavior_tick(app: &Arc<App>) {
    let wallets = app.store.wallets();
    let empty: Vec<Value> = vec![];
    let addr_re = regex::Regex::new(r"^0x[0-9a-fA-F]{40}$").unwrap();
    let targets: Vec<Value> = wallets.as_array().unwrap_or(&empty).iter()
        .filter(|w| {
            let a = w["address"].as_str().unwrap_or("");
            addr_re.is_match(a) && !app.store.get_baseline(&a.to_lowercase()).is_null()
        })
        .cloned()
        .collect();
    for w in &targets {
        let addr = w["address"].as_str().unwrap_or("").to_lowercase();
        let snap = snap_of(&flow_of(&addr).await);
        let changes = behavior_monitor::diff(&app.store.get_baseline(&addr), &snap);
        if !changes.is_empty() {
            let now = now_ms();
            let label = w.get("label").cloned().filter(|v| !v.is_null()).unwrap_or(Value::Null);
            let alert = json!({"address": addr, "label": label, "changes": changes, "ts": now});
            {
                let mut ring = app.alerts.lock().unwrap();
                ring.insert(0, alert.clone());
                if ring.len() > 50 {
                    ring.truncate(50);
                }
            }
            // re-arm baseline = {...snap, ts: now}
            let mut new_base = snap.clone();
            if let Some(o) = new_base.as_object_mut() {
                o.insert("ts".into(), json!(now));
            }
            app.store.set_baseline(&addr, new_base);
            // broadcast {type:"behavior", ...alert}
            let mut msg = serde_json::Map::new();
            msg.insert("type".into(), json!("behavior"));
            if let Some(o) = alert.as_object() {
                for (k, v) in o {
                    msg.insert(k.clone(), v.clone());
                }
            }
            broadcast_msg(app, &Value::Object(msg));
            tracing::info!("[behavior] {} → {}", &addr[..addr.len().min(10)], changes.iter().filter_map(|c| c["type"].as_str()).collect::<Vec<_>>().join(", "));
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}


pub fn is_ethfam(sym: &str) -> bool {
    matches!(sym, "ETH" | "WETH" | "STETH" | "WSTETH" | "RETH" | "CBETH" | "WBETH" | "EETH" | "WEETH")
}


/// whale-finder.js `seeds` — candidate whale addresses from ETH-representative
/// token holders + whale-clusters + the top graph-db nodes, minus already-scanned
/// and CEX addresses.
pub async fn whale_seeds(app: &Arc<App>, done: &HashSet<String>) -> Vec<String> {
    // Токени-представники ETH (stETH/wstETH/WETH/rETH) — тег `workers:eth_tokens`.
    let eth_tokens = wr_store::snapshot().tokens_with_tag("workers:eth_tokens");
    if eth_tokens.is_empty() {
        // Порожній перелік тут означав би «китів не шукаємо» — і воркер
        // відпрацював би начисто без жодної помилки. Краще сказати вголос.
        tracing::error!("[whale-seeds] у базі немає тегу workers:eth_tokens — сіди не зібрати");
        return Vec::new();
    }
    let addr_re = regex::Regex::new(r"^0x[0-9a-f]{40}$").unwrap();
    let empty: Vec<Value> = vec![];
    let mut s: IndexSet<String> = IndexSet::new();
    for c in eth_tokens {
        let th = bs::token_holders(c, 40).await;
        for h in th["holders"].as_array().unwrap_or(&empty) {
            let a = h["address"].as_str().unwrap_or("");
            if !h["isContract"].as_bool().unwrap_or(false) && !cex::is_cex(a) && addr_re.is_match(a) {
                s.insert(a.to_string());
            }
        }
    }
    let wc = app.dataset.read_or("whale-clusters.json", json!({"clusters": []}));
    for cl in wc["clusters"].as_array().unwrap_or(&empty) {
        for m in cl["members"].as_array().unwrap_or(&empty) {
            if let Some(a) = m["addr"].as_str() {
                s.insert(a.to_lowercase());
            }
        }
    }
    let g = app.dataset.read_or("graph-db.json", json!({"nodes": {}}));
    if let Some(nodes) = g["nodes"].as_object() {
        let mut nv: Vec<(&String, f64)> = nodes.iter().map(|(k, v)| (k, v["v"].as_f64().unwrap_or(0.0))).collect();
        nv.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (a, _) in nv.into_iter().take(120) {
            s.insert(a.clone());
        }
    }
    s.into_iter().filter(|a| !done.contains(a) && !cex::is_cex(a)).collect()
}


/// whale-finder.js `run` + `clusterWhales` — scan candidate whales' balances,
/// keep $200M+ (ETH-share computed), then cluster $50M+ whales by shared ETH
/// funders. Refresh whale-finder.json. Gated behind ENABLE_WHALE_FINDER_WORKER
/// (default OFF). Telegram/alerts omitted. Fetch-worker — golden `cluster_whales`
/// reduction; the scan itself is smoke-tested, not byte-parity.
pub async fn whale_finder_tick(app: &Arc<App>) {
    const NOTABLE: f64 = 200_000_000.0;
    let mut result = app.dataset.read_or("whale-finder.json", json!({"generated": 0, "status": {"running": false, "processed": 0, "total": 0}, "whales": [], "groups": []}));
    let mut whales: Vec<Value> = result["whales"].as_array().cloned().unwrap_or_default();
    let done: HashSet<String> = whales.iter().filter_map(|w| w["addr"].as_str().map(String::from)).collect();

    let cands = whale_seeds(app, &done).await;
    if cands.is_empty() {
        return;
    }
    let total = cands.len();
    let sort_whales = |v: &mut Vec<Value>| v.sort_by(|a, b| b["totalUsd"].as_f64().unwrap_or(0.0).partial_cmp(&a["totalUsd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));

    for (i, addr) in cands.iter().enumerate() {
        let bal = balances::wallet_balances(addr).await;
        let empty: Vec<Value> = vec![];
        let eth_usd: f64 = bal["tokens"].as_array().unwrap_or(&empty).iter()
            .filter(|t| is_ethfam(t["symbol"].as_str().unwrap_or("")))
            .map(|t| t["valueUsd"].as_f64().unwrap_or(0.0)).sum();
        let total_usd = bal["totalUsd"].as_f64().unwrap_or(0.0);
        let eth_share = if total_usd > 0.0 { jsmath::js_round(eth_usd / total_usd * 100.0) } else { 0.0 };
        if total_usd >= NOTABLE {
            let top: Vec<Value> = bal["tokens"].as_array().unwrap_or(&empty).iter().take(5).map(|t| t["symbol"].clone()).collect();
            let w = json!({"addr": addr, "totalUsd": total_usd, "ethUsd": jsmath::js_num(jsmath::js_round(eth_usd)), "ethShare": jsmath::js_num(eth_share), "ethAccumulator": eth_share >= 40.0, "top": top, "ts": now_ms()});
            whales.retain(|x| x["addr"].as_str() != Some(addr.as_str()));
            whales.push(w);
            sort_whales(&mut whales);
            if whales.len() > 300 {
                whales.truncate(300);
            }
        }
        result["whales"] = json!(whales);
        result["status"] = json!({"running": true, "processed": i + 1, "total": total});
        result["generated"] = json!(now_ms());
        app.dataset.write("whale-finder.json", &result);
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }

    // clusterWhales
    let px = prices::eth_price().await;
    let mut hf = serde_json::Map::new();
    let cluster_input: Vec<Value> = whales.iter().filter(|w| w["totalUsd"].as_f64().unwrap_or(0.0) >= 50_000_000.0).take(60).cloned().collect();
    for w in &cluster_input {
        let a = w["addr"].as_str().unwrap_or("");
        let nat = bs::native_transfers("eth", a, 2).await;
        hf.insert(a.to_string(), asset_groups::funders_of_eth(a, &nat));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let groups = whale_finder::cluster_whales(&whales, &Value::Object(hf), px);
    result["groups"] = groups;
    result["status"] = json!({"running": false, "processed": total, "total": total});
    app.dataset.write("whale-finder.json", &result);
    tracing::info!("[whale-finder] whales {} (groups {})", whales.len(), result["groups"].as_array().map_or(0, |a| a.len()));
}


/// coordinated-moves.js `detectCoordinatedMoves` — for each coordinated network,
/// fetch members' recent eth transfers, attach as `_per`, and run the golden-tested
/// detect_coordinated_moves_core (>=3 members moving one asset in 72h). Refresh
/// coordinated-moves.json. Gated behind ENABLE_COORD_MOVES_WORKER (default OFF).
/// Telegram/alerts omitted. Fetch-worker — smoke-tested, not byte-parity.
pub async fn coordinated_moves_tick(app: &Arc<App>) {
    let cn = coordinated_networks_value(app).await;
    let empty: Vec<Value> = vec![];
    let nets = cn["networks"].as_array().unwrap_or(&empty);
    let total = nets.len();
    let mut nets_with_per: Vec<Value> = Vec::new();
    for net in nets.iter().take(10) {
        let members: Vec<String> = net["members"].as_array().unwrap_or(&empty).iter().take(15)
            .filter_map(|m| m["addr"].as_str().map(String::from)).collect();
        let per: Vec<Vec<Value>> = join_all(members.iter().map(|a| bs::transfers_rich("eth", a, 1))).await;
        let mut n = net.clone();
        n["_per"] = json!(per);
        nets_with_per.push(n);
    }
    let moves = coordinated_moves::detect_coordinated_moves_core(&nets_with_per);
    let result = json!({"generated": now_ms(), "moves": moves, "status": {"running": false, "processed": total, "total": total}});
    app.dataset.write("coordinated-moves.json", &result);
    tracing::info!("[coord-moves] synchronous moves: {}", result["moves"].as_array().map_or(0, |a| a.len()));
}


/// active-groups.js `memberActivity` — a member's fresh (>= cutoff) on-chain
/// activity across eth/base/arbitrum ERC-20 + eth native.
pub async fn member_activity(addr: &str, cutoff: i64) -> Value {
    const CHAINS: [&str; 3] = ["eth", "base", "arbitrum"];
    let (mut txs, mut in_usd, mut out_usd, mut last) = (0i64, 0.0f64, 0.0f64, 0i64);
    let per = join_all(CHAINS.iter().map(|&c| bs::transfers_rich(c, addr, 1))).await;
    for trs in &per {
        for t in trs {
            let ts = t["ts"].as_i64().unwrap_or(0);
            if ts == 0 || ts < cutoff {
                continue;
            }
            txs += 1;
            if ts > last {
                last = ts;
            }
            let usd = t["usd"].as_f64().unwrap_or(0.0);
            if t["to"].as_str() == Some(addr) {
                in_usd += usd;
            } else if t["from"].as_str() == Some(addr) {
                out_usd += usd;
            }
        }
    }
    for t in &bs::native_transfers("eth", addr, 1).await {
        let ts = t["ts"].as_i64().unwrap_or(0);
        if ts >= cutoff {
            txs += 1;
            if ts > last {
                last = ts;
            }
        }
    }
    json!({"txs": txs, "inUsd": in_usd, "outUsd": out_usd, "last": last})
}


/// active-groups.js `run` — analyse the top coordinated clusters' 7-day activity,
/// rank by heat, refresh active-groups.json (with live status). Gated behind
/// ENABLE_ACTIVE_GROUPS_WORKER (default OFF — writes the shared dataset file).
/// Telegram/hot-group alerts are omitted (the bot is not ported); the reduction
/// is the golden-tested active_groups::analyze_group.
pub async fn active_groups_tick(app: &Arc<App>) {
    let empty: Vec<Value> = vec![];
    let clusters_file = app.dataset.read_or("whale-clusters.json", json!({"clusters": []}));
    let clusters: Vec<Value> = clusters_file["clusters"].as_array().unwrap_or(&empty).iter().take(15).cloned().collect();
    if clusters.is_empty() {
        return;
    }
    let cutoff = now_ms() - 7 * 86_400_000;
    let total = clusters.len();
    let mut out: Vec<Value> = Vec::new();
    let mut processed = 0i64;
    for c in &clusters {
        let members: Vec<Value> = c["members"].as_array().unwrap_or(&empty).iter().take(8).cloned().collect();
        let mut acts: Vec<Value> = Vec::new();
        for m in &members {
            let addr = m["addr"].as_str().unwrap_or("");
            acts.push(member_activity(addr, cutoff).await);
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        let g = active_groups::analyze_group(c, &acts, now_ms());
        if g["txs7d"].as_i64().unwrap_or(0) > 0 {
            out.push(g);
        }
        processed += 1;
        let mut sorted = out.clone();
        sorted.sort_by(|a, b| b["heat"].as_i64().unwrap_or(0).cmp(&a["heat"].as_i64().unwrap_or(0)));
        let result = json!({"generated": now_ms(), "status": {"running": true, "processed": processed, "total": total}, "groups": sorted});
        app.dataset.write("active-groups.json", &result);
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
    out.sort_by(|a, b| b["heat"].as_i64().unwrap_or(0).cmp(&a["heat"].as_i64().unwrap_or(0)));
    let result = json!({"generated": now_ms(), "status": {"running": false, "processed": total, "total": total}, "groups": out});
    app.dataset.write("active-groups.json", &result);
    tracing::info!("[active] active whale groups: {}", result["groups"].as_array().map_or(0, |a| a.len()));
}


/// feed.js `pollBlockscoutOnce` — one polling pass over tracked wallets: fetch new
/// ERC-20 + native transfers per chain (past the stored cursor), emit each as an
/// event into the store + broadcast to /ws. Gated behind ENABLE_FEED_WORKER (off
/// by default — Node owns the shared data.json unless rust is the sole server).
pub async fn poll_blockscout_once(app: &Arc<App>) {
    let wallets = app.store.wallets();
    let empty: Vec<Value> = vec![];
    let addr_re = regex::Regex::new(r"^0x[0-9a-fA-F]{40}$").unwrap();
    let bs_chains = bs::bs_chains();
    for w in wallets.as_array().unwrap_or(&empty) {
        let addr = w["address"].as_str().unwrap_or("");
        if !addr_re.is_match(addr) {
            continue;
        }
        let wid = w["id"].as_str().unwrap_or("");
        let mut chains: Vec<String> = w["networks"].as_array().unwrap_or(&empty).iter()
            .filter_map(|c| c.as_str()).filter(|c| bs_chains.contains(c)).map(String::from).collect();
        if chains.is_empty() {
            chains.push("eth".to_string());
        }
        for chain in &chains {
            // ERC-20
            let trs = bs::token_transfers(chain, addr).await;
            if !trs.is_empty() {
                let key = format!("{wid}:{chain}");
                let cursor = app.store.get_cursor(&key);
                let max_block = trs.iter().map(|t| t["block"].as_i64().unwrap_or(0)).max().unwrap_or(0);
                let sk = |t: &Value| (t["block"].as_i64().unwrap_or(0), t["logIndex"].as_i64().unwrap_or(0));
                if cursor.is_null() {
                    let mut recent = trs.clone();
                    recent.sort_by_key(&sk);
                    let start = recent.len().saturating_sub(8);
                    for t in &recent[start..] {
                        let ev = feed::to_event(w, chain, t);
                        app.store.add_event(ev.clone());
                        broadcast_event(app, &ev);
                    }
                    app.store.set_cursor(&key, json!(max_block));
                    continue; // native skipped on the ERC-20 baseline cycle (matches JS)
                }
                let cur = cursor.as_i64().unwrap_or(0);
                let mut fresh: Vec<Value> = trs.iter().filter(|t| t["block"].as_i64().unwrap_or(0) > cur).cloned().collect();
                fresh.sort_by_key(&sk);
                for t in &fresh {
                    let ev = feed::to_event(w, chain, t);
                    app.store.add_event(ev.clone());
                    broadcast_event(app, &ev);
                }
                if max_block > cur {
                    app.store.set_cursor(&key, json!(max_block));
                }
            }
            // native
            let nat: Vec<Value> = bs::native_transfers(chain, addr, 1).await.into_iter()
                .filter(|t| !t["txHash"].as_str().unwrap_or("").is_empty() && t["block"].as_i64().unwrap_or(0) != 0)
                .collect();
            if !nat.is_empty() {
                let nk = format!("{wid}:{chain}:nat");
                let ncur = app.store.get_cursor(&nk);
                let nmax = nat.iter().map(|t| t["block"].as_i64().unwrap_or(0)).max().unwrap_or(0);
                let px = prices::native_price_usd(chain).await;
                let sym = feed::net_symbol(chain);
                let bk = |t: &Value| t["block"].as_i64().unwrap_or(0);
                if ncur.is_null() {
                    let mut s = nat.clone();
                    s.sort_by_key(&bk);
                    let start = s.len().saturating_sub(5);
                    for t in &s[start..] {
                        let ev = feed::native_event(w, chain, t, px, sym);
                        app.store.add_event(ev.clone());
                        broadcast_event(app, &ev);
                    }
                    app.store.set_cursor(&nk, json!(nmax));
                } else {
                    let nc = ncur.as_i64().unwrap_or(0);
                    let mut f: Vec<Value> = nat.iter().filter(|t| bk(t) > nc).cloned().collect();
                    f.sort_by_key(&bk);
                    for t in &f {
                        let ev = feed::native_event(w, chain, t, px, sym);
                        app.store.add_event(ev.clone());
                        broadcast_event(app, &ev);
                    }
                    if nmax > nc {
                        app.store.set_cursor(&nk, json!(nmax));
                    }
                }
            }
        }
    }
}

