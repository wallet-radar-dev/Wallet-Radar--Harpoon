#![allow(dead_code, unused_imports)]
use crate::workers::*;
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

/// server/index.js:286. The `node` field is dropped: nothing in web/app.ts reads
/// it, and there is no Node process to report.
pub async fn version() -> impl IntoResponse {
    Json(json!({"name": "Wallet Radar", "version": APP_VERSION, "runtime": concat!("rust ", env!("CARGO_PKG_VERSION"))}))
}


/// server/index.js:283-285. Node calls getEvents(9999), so the count saturates
/// at 9999 rather than reporting the true length.
pub async fn health(State(s): State<Ctx>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "status": "healthy",
        "version": APP_VERSION,
        "uptime": s.started.elapsed().as_secs(),
        "wallets": s.store.wallet_count(),
        "events": s.store.event_count().min(9999),
    }))
}


/// server/index.js:249-252. `role` is always "admin" for loopback callers.
///
/// `hasMoralis` питає «чи доступний Moralis», а не «чи лежить ключ у мене». У суворому режимі
/// ключа в кора НЕМАЄ за задумом — і без цієї гілки фронт показував би «Moralis не
/// налаштований» саме тоді, коли той працює. Точний стан кожного провайдера (у кого який ключ)
/// віддає /api/workers-health → satellites.
pub async fn config() -> impl IntoResponse {
    let has = |k: &str| std::env::var(k).map(|v| !v.is_empty()).unwrap_or(false);
    Json(json!({
        "hasMoralis": has("MORALIS_API_KEY") || satellite::strict(),
        "hasSentry": has("SENTRY_DSN"),
        "role": "admin",
        "readOnly": false,
    }))
}


pub async fn wallets(State(s): State<Ctx>) -> impl IntoResponse {
    Json(s.store.wallets())
}


/// index.js:291 `POST /api/wallets` — admin-gated (see `requires_admin`). Retrying
/// with the same `Idempotency-Key` replays the first answer instead of creating a
/// duplicate wallet; only successes are cached, so a rejected body can be retried
/// with the same key. Body fields other than the four named ones are dropped, as in
/// the JS, which rebuilds `w` from scratch.
pub async fn add_wallet(State(s): State<Ctx>, headers: HeaderMap, body: Option<Json<Value>>) -> Response {
    let now = now_ms();
    let idem_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();
    if !idem_key.is_empty() {
        if let Some((status, body)) = idem_get(&s, &idem_key, now) {
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::CREATED);
            return (code, [("Idempotent-Replay", "true")], Json(body)).into_response();
        }
    }

    let b = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let address = b["address"].as_str().unwrap_or("");
    let networks = b["networks"].as_array().cloned().unwrap_or_default();
    // `!address || !Array.isArray(networks) || !networks.length`
    if address.is_empty() || networks.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "address і networks обовʼязкові"}))).into_response();
    }

    // id = "w" + Date.now().toString(36). Keys go in the JS literal's order —
    // `preserve_order` carries it into both the response body and data.json.
    let mut w = serde_json::Map::new();
    w.insert("id".into(), json!(format!("w{}", radix36(now))));
    w.insert("address".into(), json!(address));
    w.insert("networks".into(), json!(networks));
    // `label: label || undefined` — a falsy label leaves the key off entirely.
    if let Some(l) = b["label"].as_str().filter(|l| !l.is_empty()) {
        w.insert("label".into(), json!(l));
    }
    w.insert("holdings".into(), b["holdings"].as_array().map_or_else(|| json!([]), |h| json!(h)));
    let w = Value::Object(w);

    s.store.add_wallet(w.clone());
    if !idem_key.is_empty() {
        idem_set(&s, idem_key, 201, w.clone(), now);
    }
    (StatusCode::CREATED, Json(w)).into_response()
}


/// index.js:255 — the macro layer (server/market.js). 502 on failure, as in Node,
/// where a throw inside the handler is caught and reported as a bad gateway.
pub async fn market_context(State(s): State<Ctx>) -> impl IntoResponse {
    let moves = s.dataset.read_or("coordinated-moves.json", json!({"moves": []}));
    Json(market::get_market_context(&moves, now_ms()).await).into_response()
}


/// index.js:257 — token-ai.js `answerToken(contract, q)`: classify the question,
/// return the matching slice of the asset-flow computation.
pub async fn token_ai_route(Query(q): Query<TokenAiQuery>) -> Response {
    let Some(contract) = q.contract.filter(|c| !c.is_empty()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "contract required"}))).into_response();
    };
    let flow = compute_asset_flow_value(&contract).await;
    if let Some(err) = flow.get("error").filter(|e| !e.is_null()) {
        return (StatusCode::BAD_GATEWAY, Json(json!({"error": err}))).into_response();
    }
    Json(token_ai::answer(&flow, q.q.as_deref().unwrap_or(""))).into_response()
}


#[derive(serde::Deserialize)]
pub struct TokenAiQuery {
    contract: Option<String>,
    q: Option<String>,
}


/// index.js:256 — the liquidation monitor's in-memory windows (server/liquidations.js).
pub async fn liquidations_route() -> impl IntoResponse {
    Json(liquidations::get_liquidations(now_ms()))
}


/// index.js:696 — worker liveness from the dataset files' mtimes (server/health.js).
///
/// Коли сателіти налаштовані, до відповіді додається `satellites` — хто живий і в кого який
/// ключ. Ключ додається ЛИШЕ в цьому режимі: без сателітів відповідь мусить лишатися
/// байт-у-байт такою, як у JS, інакше паритет порту стає неперевірюваним.
pub async fn workers_health(State(s): State<Ctx>) -> impl IntoResponse {
    let mut h = health::health(s.dataset.root(), now_ms());
    if satellite::strict() {
        h["satellites"] = json!(satellite::health().await);
    }
    Json(h)
}


/// index.js:673 `expandStatus()` — expand-worker.js:39. The crawler itself is not
/// ported, so there is no live in-memory state: we report exactly what the JS
/// reports on a cold start, i.e. its boot-time restore from graph-db.json (counts
/// re-derived from the file, `expanded`/`groups`/`updatedAt` still zero). Key order
/// follows the JS `{...status, nodes, edges, frontier, volumeUsd}` spread.
pub async fn graph_db_stats(State(s): State<Ctx>) -> impl IntoResponse {
    let g = s.dataset.read_or("graph-db.json", json!({}));
    let nodes = g["nodes"].as_object().map_or(0, |o| o.len());
    let edges = g["edges"].as_array().map_or(0, |a| a.len());
    let frontier = g["frontier"].as_array().map_or(0, |a| a.len());
    // totalVol = Σ edge.u, rounded once at the end (expand-worker.js:39).
    let vol: f64 = g["edges"].as_array().map_or(0.0, |a| a.iter().map(|e| e["u"].as_f64().unwrap_or(0.0)).sum());
    Json(json!({
        "running": false,
        "nodes": nodes,
        "edges": edges,
        "frontier": frontier,
        "expanded": 0,
        "groups": 0,
        "volumeUsd": js_num(round_fixed(vol, 0)),
        "updatedAt": 0,
    }))
}


/// index.js:667 `forensicsStatus()` — forensics-worker.js:145 returns its in-memory
/// `status`, which is the same object it persists into asset-forensics.json. The
/// worker is not ported, so we serve the persisted one (its cold-start default when
/// the file has none).
pub async fn forensics_status(State(s): State<Ctx>) -> impl IntoResponse {
    let f = s.dataset.read_or("asset-forensics.json", json!({}));
    let status = f["status"].clone();
    if status.is_object() {
        return Json(status);
    }
    Json(json!({"running": false, "cycle": 0, "lastAsset": "", "processed": 0, "total": 0, "holders": 50, "clusters": 0, "updatedAt": 0}))
}


/// index.js:671 `telegramStatus()` — telegram.js:47. `ready` flips to true once the
/// chat-resolver worker has picked up the user's chat from a /start message.
pub async fn telegram_status() -> impl IntoResponse {
    Json(telegram::status())
}


/// index.js:1164 — deliberately fails, so GlitchTip/Sentry capture can be tested.
/// The thrown message never reaches the client: Express's error handler answers 500
/// with a generic body, and that is what we reproduce.
pub async fn debug_error() -> impl IntoResponse {
    // tracing::error! підхоплює sentry-tracing → подія летить у GlitchTip. Саме для цього
    // ендпоінт і існує (перевірка наскрізного захоплення помилок).
    tracing::error!("навмисний тест Sentry/GlitchTip: /api/debug-error викликано");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Внутрішня помилка — спробуйте ще раз"})))
}


/// index.js:309 `DELETE /api/wallets/:id` — admin-gated, always `{ok:true}`.
pub async fn delete_wallet(State(s): State<Ctx>, Path(id): Path<String>) -> impl IntoResponse {
    s.store.remove_wallet(&id);
    Json(json!({"ok": true}))
}


/// server/index.js:254 — static CLARITY map + status metadata.
pub async fn clarity_tags() -> impl IntoResponse {
    Json(clarity::clarity_tags())
}


#[derive(Deserialize)]
pub struct EventsQuery {
    limit: Option<String>,
}


/// server/index.js — `GET /api/events?limit=100`, default 100.
pub async fn events(State(s): State<Ctx>, Query(q): Query<EventsQuery>) -> impl IntoResponse {
    let limit = q.limit.and_then(|l| l.parse::<usize>().ok()).unwrap_or(100);
    Json(s.store.events(limit))
}


#[derive(Deserialize)]
pub struct TransfersQuery {
    chain: Option<String>,
    address: Option<String>,
    pages: Option<u32>,
}


/// `GET /api/transfers` — ВИМКНЕНО 15.08.2026.
///
/// Роут віддавав клієнту рядки провайдера у СИРОМУ вигляді. Це була єдина точка продукту,
/// яка робила саме перепоширення чужого контенту, а не власні висновки з нього. Умови
/// постачальників відносять передачу назовні «in raw, processed, or substantially similar
/// form» до дій, що потребують окремої письмової ліцензії.
///
/// Перевірено перед вимкненням: власний фронтенд цим роутом не користувався (немає згадок
/// у `web/src/*.ts` і `web/*.html`), у специфікації OpenAPI його теж немає. Тобто вимкнення
/// нічого не ламає. 410 замість 404 — щоб зовнішній споживач, про якого ми не знаємо, дістав
/// чесну відповідь «прибрано навмисно», а не загадку.
///
/// Що НЕ змінюється: внутрішнє використання тих самих джерел для ПОХІДНИХ показників —
/// групи капіталу, потоки, координація, вердикти — лишається. Похідний висновок не є
/// перепоширенням рядків, і саме на ньому стоїть продукт.
pub async fn transfers(Query(_q): Query<TransfersQuery>) -> impl IntoResponse {
    (
        StatusCode::GONE,
        Json(json!({
            "error": "Сирі рядки провайдера назовні не віддаються",
            "since": "2026-08-15",
            "reason": "перепоширення контенту постачальника потребує окремої ліцензії",
            "useInstead": ["/api/wallet-assets", "/api/wallet-netflow", "/api/wallet-clusters"],
        })),
    )
        .into_response()
}


#[derive(Deserialize)]
pub struct AddressQuery {
    address: Option<String>,
}


/// Fetch native transfers across the conductor chains (4 pages each, capped to 3
/// inside), matching conductor.js `CHAINS.map(c => bs.nativeTransfers(c, a, 4))`.
pub async fn conductor_per_chain(address: &str) -> Vec<Value> {
    let (eth, base, arb, opt, poly) = tokio::join!(
        bs::native_transfers("eth", address, 4),
        bs::native_transfers("base", address, 4),
        bs::native_transfers("arbitrum", address, 4),
        bs::native_transfers("optimism", address, 4),
        bs::native_transfers("polygon", address, 4),
    );
    vec![Value::Array(eth), Value::Array(base), Value::Array(arb), Value::Array(opt), Value::Array(poly)]
}


/// `GET /api/conductor-network?address=0x…` — conductor.js `conductorNetwork`:
/// the funding network (outgoing native transfers → sub-wallets).
pub async fn conductor_network_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let a = q.address.unwrap_or_default();
    let a = a.trim();
    if !valid_addr(a) {
        return bad_addr().into_response();
    }
    let addr = a.to_lowercase();
    let px = prices::eth_price().await;
    let per = conductor_per_chain(&addr).await;
    Json(conductor::conductor_network(&addr, &per, px)).into_response()
}


/// `GET /api/coordination?address=0x…` — coordination.js `coordinationScore`:
/// the 0-100 coordination-confidence over the funding network.
pub async fn coordination_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let a = q.address.unwrap_or_default();
    let a = a.trim();
    if !valid_addr(a) {
        return bad_addr().into_response();
    }
    let addr = a.to_lowercase();
    let px = prices::eth_price().await;
    let per = conductor_per_chain(&addr).await;
    let net = conductor::conductor_network(&addr, &per, px);
    Json(conductor::coordination_score(&addr, &net)).into_response()
}


/// `GET /api/wallet-score?address=0x…` — realized PnL / ROI / win-rate. Ports
/// walletscore.js `computeWalletScore`: fetch ERC-20 transfers across 5 chains
/// (Blockscout), price them historically (DefiLlama), then run the kernel.
pub async fn wallet_score(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let raw = q.address.unwrap_or_default();
    let raw = raw.trim();
    if !valid_addr(raw) {
        return bad_addr().into_response();
    }
    let address = raw.to_lowercase();
    Json(compute_wallet_score(&address).await).into_response()
}


/// walletscore.js `computeWalletScore` network pipeline: fetch ERC-20 transfers
/// across 5 chains (Blockscout), price them historically (DefiLlama), run the
/// kernel. Shared by `/api/wallet-score` and `/api/report`.
pub async fn compute_wallet_score(address: &str) -> Value {
    // Score = ERC-20 transfers across 5 chains + DefiLlama historical pricing
    // (~45s cold). Shared by /api/wallet-score and /api/report — cache it.
    let ck = format!("wscore:{}", address.to_lowercase());
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(900)) {
        return v;
    }
    let v = compute_wallet_score_uncached(address).await;
    if !v.is_null() {
        crate::bs::cache_value(&ck, &v);
    }
    v
}

async fn compute_wallet_score_uncached(address: &str) -> Value {
    // Promise.all over the 5 chains — fetch each chain's transfers concurrently.
    let (eth, base, arb, opt, poly) = tokio::join!(
        bs::transfers_rich("eth", address, 3),
        bs::transfers_rich("base", address, 3),
        bs::transfers_rich("arbitrum", address, 3),
        bs::transfers_rich("optimism", address, 3),
        bs::transfers_rich("polygon", address, 3),
    );
    let mut by_chain: serde_json::Map<String, Value> = serde_json::Map::new();
    by_chain.insert("eth".into(), Value::Array(eth));
    by_chain.insert("base".into(), Value::Array(base));
    by_chain.insert("arbitrum".into(), Value::Array(arb));
    by_chain.insert("optimism".into(), Value::Array(opt));
    by_chain.insert("polygon".into(), Value::Array(poly));
    let eth_native: std::collections::HashSet<&str> = ["eth", "base", "arbitrum", "optimism"].into_iter().collect();
    prices::price_transfers_historical(&mut by_chain, &eth_native).await;
    walletscore::compute_wallet_score(address, &by_chain)
}


/// `GET /api/wallet-netflow?address=0x…` — conductor.js `walletCexNetflow`: a
/// wallet's direct net flow to/from exchanges. Reuses the golden-tested
/// `wallet_cex_netflow` kernel over live native + token transfers and ETH price.
pub async fn wallet_netflow_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let a = q.address.unwrap_or_default();
    let a = a.trim();
    if !valid_addr(a) {
        // This route's JS body is the shorter "Потрібна адреса 0x…".
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібна адреса 0x…"}))).into_response();
    }
    let addr = a.to_lowercase();
    Json(compute_wallet_netflow(&addr).await).into_response()
}


/// conductor.js `walletCexNetflow` network pipeline. Shared by `/api/wallet-netflow`
/// and `/api/report`.
pub async fn compute_wallet_netflow(addr: &str) -> Value {
    let ck = format!("wnetflow:{}", addr.to_lowercase());
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(600)) {
        return v;
    }
    let v = compute_wallet_netflow_uncached(addr).await;
    crate::bs::cache_value(&ck, &v);
    v
}

async fn compute_wallet_netflow_uncached(addr: &str) -> Value {
    let px = prices::eth_price().await;
    let (nat, tok) = tokio::join!(bs::native_transfers("eth", addr, 3), bs::transfers_rich("eth", addr, 2));
    conductor::wallet_cex_netflow(addr, &nat, &tok, px)
}


/// flow-features.js `_extractFlow` — the network half. Fetch ERC-20 + native
/// transfers across the six flow chains (2 pages each), value the native leg
/// (ETH-native chains at the ETH price, gnosis 1:1, others 0), then price tokens
/// at current DefiLlama prices before running the golden-tested `extract_flow`.
pub async fn flow_of(address: &str) -> Value {
    // 6-chain transfer flow (~20s cold). Cache it — used by /api/wallet-assets.
    let ck = format!("wflow:{}", address.to_lowercase());
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(600)) {
        return v;
    }
    let v = flow_of_uncached(address).await;
    if v["txTotal"].as_f64().unwrap_or(0.0) > 0.0 {
        crate::bs::cache_value(&ck, &v);
    }
    v
}

async fn flow_of_uncached(address: &str) -> Value {
    const CHAINS: [&str; 6] = ["eth", "base", "arbitrum", "optimism", "polygon", "gnosis"];
    const ETH_NATIVE: [&str; 4] = ["eth", "base", "arbitrum", "optimism"];
    let px = prices::eth_price().await;

    // Promise.all over chains; the two fetches per chain run in series, as in JS.
    let fetches = CHAINS.iter().map(|&chain| async move {
        let trs = bs::transfers_rich(chain, address, 2).await;
        let nat = bs::native_transfers(chain, address, 2).await;
        (chain, trs, nat)
    });
    let results = join_all(fetches).await;

    let mut by_chain: serde_json::Map<String, Value> = serde_json::Map::new();
    for (chain, trs, mut nat) in results {
        let usd_of = |amount: f64| {
            if ETH_NATIVE.contains(&chain) {
                amount * px
            } else if chain == "gnosis" {
                amount
            } else {
                0.0
            }
        };
        for t in nat.iter_mut() {
            let amount = t["amount"].as_f64().unwrap_or(0.0);
            if let Some(o) = t.as_object_mut() {
                o.insert("usd".into(), jsmath::js_num(usd_of(amount)));
            }
        }
        // all = trs.concat(nat); only keep chains with any activity.
        let mut all = trs;
        all.extend(nat);
        if !all.is_empty() {
            by_chain.insert(chain.to_string(), Value::Array(all));
        }
    }
    prices::price_transfers(&mut by_chain).await;
    flow_features::extract_flow(address, &by_chain)
}


/// index.js `snapOf` — reduce flow features to a behavioural snapshot: net stance,
/// dominant counterparty category (argmax over cex/dex/bridge/defi/eoa, first-wins
/// on ties in that order), and the raw share/flow fields the diff compares.
pub fn snap_of(f: &Value) -> Value {
    let shares = [
        ("cex", &f["cexShare"]),
        ("dex", &f["dexShare"]),
        ("bridge", &f["bridgeShare"]),
        ("defi", &f["defiShare"]),
        ("eoa", &f["eoaShare"]),
    ];
    let mut dom = "eoa";
    let mut dv = -1.0f64;
    for (k, v) in shares {
        // `v > dv`: NaN never wins, matching JS.
        if v.as_f64().map(|x| x > dv).unwrap_or(false) {
            dv = v.as_f64().unwrap();
            dom = k;
        }
    }
    let net = f["netUsd"].as_f64().unwrap_or(f64::NAN);
    json!({
        "stance": if net >= 0.0 { "accumulating" } else { "distributing" },
        "dominant": dom,
        "sellShare": f["sellShare"],
        "buyShare": f["buyShare"],
        "cexShare": f["cexShare"],
        "dexShare": f["dexShare"],
        "netUsd": f["netUsd"],
        "txPerDay": f["txPerDay"],
    })
}


#[derive(Deserialize)]
pub struct BehaviorQuery {
    address: Option<String>,
    arm: Option<String>,
}


/// `GET /api/behavior-check?address=0x…[&arm=1]` — index.js behaviour route:
/// snapshot a wallet's flow, and either arm it as the baseline (`arm=1`) or diff
/// the current snapshot against the stored baseline into a list of `changes`.
pub async fn behavior_check(State(s): State<Ctx>, Query(q): Query<BehaviorQuery>) -> impl IntoResponse {
    let raw = q.address.unwrap_or_default();
    let raw = raw.trim();
    if !valid_addr(raw) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Поведінкові алерти — лише EVM (0x…)"}))).into_response();
    }
    let address = raw.to_lowercase();
    let snap = snap_of(&flow_of(&address).await);

    if q.arm.as_deref() == Some("1") {
        let mut baseline = snap.clone();
        if let Some(o) = baseline.as_object_mut() {
            o.insert("ts".into(), json!(now_ms()));
        }
        s.store.set_baseline(&address, baseline);
        return Json(json!({"armed": true, "snapshot": snap})).into_response();
    }

    let base = s.store.get_baseline(&address);
    let mut changes: Vec<Value> = Vec::new();
    if truthy(&base) {
        if base["stance"] != snap["stance"] {
            changes.push(json!({"type": "stance", "from": base["stance"], "to": snap["stance"]}));
        }
        if base["dominant"] != snap["dominant"] {
            changes.push(json!({"type": "dominant", "from": base["dominant"], "to": snap["dominant"]}));
        }
        let snap_sell = snap["sellShare"].as_f64().unwrap_or(f64::NAN);
        if snap_sell - num_or0(&base["sellShare"]) >= 0.25 {
            changes.push(json!({"type": "sell_up"}));
        }
        let snap_buy = snap["buyShare"].as_f64().unwrap_or(f64::NAN);
        if snap_buy - num_or0(&base["buyShare"]) >= 0.25 {
            changes.push(json!({"type": "buy_up"}));
        }
        // Math.sign(base.netUsd||0) !== Math.sign(snap.netUsd||0) && |snap.netUsd| > 1000
        let sign = |x: f64| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 };
        let snap_net = snap["netUsd"].as_f64().unwrap_or(f64::NAN);
        if sign(num_or0(&base["netUsd"])) != sign(num_or0(&snap["netUsd"])) && snap_net.abs() > 1000.0 {
            changes.push(json!({"type": "netflow_flip"}));
        }
        let base_tpd = num_or0(&base["txPerDay"]);
        let snap_tpd = snap["txPerDay"].as_f64().unwrap_or(f64::NAN);
        if base_tpd > 0.1 && snap_tpd >= base_tpd * 2.0 {
            changes.push(json!({"type": "activity_spike"}));
        }
    }

    Json(json!({
        "snapshot": snap,
        "baseline": base,
        "changes": changes,
        "hasBaseline": truthy(&base),
    }))
    .into_response()
}


#[derive(Deserialize)]
pub struct ContractQuery {
    contract: Option<String>,
}


#[derive(Deserialize)]
pub struct CgPriceQuery {
    ids: Option<String>,
}


#[derive(Deserialize)]
pub struct CgSearchQuery {
    query: Option<String>,
}


#[derive(Deserialize)]
pub struct CgAssetQuery {
    contract: Option<String>,
    id: Option<String>,
}

/// `/api/cg-asset?contract=…` or `?id=…` — compact market snapshot (contract,
/// price, market cap, rank) used by the asset card header and to resolve a
/// CoinGecko search hit to an ERC-20 contract so it opens the full asset card.
pub async fn cg_asset_route(Query(q): Query<CgAssetQuery>) -> impl IntoResponse {
    Json(cg::cg_asset(q.contract.as_deref().unwrap_or(""), q.id.as_deref().unwrap_or("")).await)
}

/// index.js:259 `/api/cg-price` — CoinGecko simple-price proxy.
pub async fn cg_price_route(Query(q): Query<CgPriceQuery>) -> impl IntoResponse {
    Json(cg::cg_price(&q.ids.unwrap_or_default()).await)
}


/// index.js:270 `/api/cg-search` — CoinGecko search proxy.
pub async fn cg_search_route(Query(q): Query<CgSearchQuery>) -> impl IntoResponse {
    Json(cg::cg_search(&q.query.unwrap_or_default()).await)
}


/// Full asset-flow orchestration (fetches + kernel) → the assetflow.js
/// `computeAssetFlow` object. Mirrors asset_flow_route's pipeline so /api/strategy
/// can reuse it without editing that route.
///
/// assetflow.js:11 обгортає ВЕСЬ цей розрахунок у `cached("aflow:…", 600000)` — і порт цю
/// обгортку загубив. Пропуск найдорожчий з усіх: під ним ~120 мережевих запитів (holders,
/// transfers, ціни, market-data) ПЛЮС виклик LunarCrush. Воркер фаз проганяє 30 активів, а
/// /api/strategy, /api/asset-flow і /api/token-ai б'ють у той самий розрахунок — без кешу кожен
/// із них платить повну ціну заново.
pub async fn compute_asset_flow_value(contract: &str) -> Value {
    let key = format!("aflow:{}", contract.to_lowercase());
    if let Some(v) = bs::cached_value(&key, std::time::Duration::from_secs(600)) {
        return v;
    }
    let v = compute_asset_flow_uncached(contract).await;
    // Порожній/помилковий результат не закріплюємо на 10 хвилин.
    if v.is_object() {
        bs::cache_value(&key, &v);
    }
    v
}


pub async fn compute_asset_flow_uncached(contract: &str) -> Value {
    let (info_res, top, series, mc) = tokio::join!(
        bs::token_info(contract),
        bs::token_holders(contract, 40),
        prices::price_chart(contract, "eth"),
        prices::market_data(contract),
    );
    let info = info_res.unwrap_or_else(|_| json!({}));
    let empty = vec![];
    let holders: Vec<Value> = top["holders"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|h| {
            let addr = h["address"].as_str().unwrap_or("").to_lowercase();
            let share = h["share"].as_f64().filter(|s| *s != 0.0 && !s.is_nan()).unwrap_or(0.0);
            json!({"addr": addr, "share": jsmath::js_num(share)})
        })
        .filter(|h| {
            let a = h["addr"].as_str().unwrap_or("");
            a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && !assetflow_known_cex(a) && a != contract
        })
        .take(30)
        .collect();
    let per: Vec<Value> = join_all(holders.iter().map(|h| {
        let addr = h["addr"].as_str().unwrap_or("").to_string();
        let contract = contract.to_string();
        async move { Value::Array(bs::token_transfers_by_token("eth", &addr, &contract, 4).await) }
    }))
    .await;
    let symbol = info["symbol"].as_str().filter(|s| !s.is_empty()).or_else(|| mc["symbol"].as_str().filter(|s| !s.is_empty())).unwrap_or("").to_string();
    let mut social = prices::get_social(&symbol).await;
    if social.is_null() {
        social = assetflow::get_social_free(contract, &mc);
    }
    assetflow::compute_asset_flow(contract, &info, &holders, &Value::Array(per), &series, &mc, &social, now_ms() as f64)
}


/// `GET /api/strategy?contract=0x…` — strategy.js `assetStrategy`: DCA phase /
/// action from the whale exit-ratio E over the asset-flow object. The `rising`
/// trend is history-based in Node; this cold process uses the `distributed > 0`
/// fallback (no persisted history), then runs the golden-tested kernel.
pub async fn strategy_route(Query(q): Query<ContractQuery>) -> impl IntoResponse {
    let contract = q.contract.unwrap_or_default();
    let contract = contract.trim().to_lowercase();
    if !valid_addr(&contract) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібен ERC-20 контракт 0x… (40 hex)"}))).into_response();
    }
    let f = compute_asset_flow_value(&contract).await;
    let rising = f["distributed"].as_f64().unwrap_or(0.0) > 0.0;
    Json(strategy::asset_strategy(&f, rising)).into_response()
}


/// `GET /api/wallet-intel/{id}` — intel.js `walletIntel`: flow summary + behaviour
/// class + score over a tracked wallet's events, plus the optional model.js ML
/// overlay (extractFeaturesBS → classify) when the model is loaded and the wallet
/// has a valid EVM address. 404 when the id is not in the watchlist.
pub async fn wallet_intel_route(State(s): State<Ctx>, Path(id): Path<String>) -> impl IntoResponse {
    let wallets = s.store.wallets();
    let w = wallets.as_array().and_then(|a| a.iter().find(|w| w["id"].as_str() == Some(id.as_str())).cloned());
    let Some(w) = w else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "wallet not found"}))).into_response();
    };

    let events = s.store.events(9999);
    let empty = vec![];
    let mut base = intel::wallet_intel(&id, events.as_array().unwrap_or(&empty));

    // ML overlay: model.ready() && /^0x…40$/.test(address) → classify(extractFeaturesBS)
    let addr = w["address"].as_str().unwrap_or("");
    let ml = if valid_addr(addr) {
        if let Some(model) = model_js() {
            let items = bs::transfers_rich("eth", addr, 1).await;
            let native_eth = bs::rpc_native_amount("eth", addr).await.unwrap_or(0.0);
            let feat = features_bs::extract_features_bs(addr, &items, native_eth);
            model.classify(&feat)
        } else {
            Value::Null
        }
    } else {
        Value::Null
    };
    base["ml"] = ml;
    Json(base).into_response()
}


/// `GET /api/concentration?contract=0x…` — token holder concentration. Ports the
/// index.js route: tokenInfo + tokenHolders(50) + addressMeta(top 30), then the
/// deterministic `concentration::build`. Blockscout-only, so fully reproducible.
pub async fn concentration_route(Query(q): Query<ContractQuery>) -> impl IntoResponse {
    let raw = q.contract.unwrap_or_default();
    let contract = raw.trim().to_string();
    if !valid_addr(&contract) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібен ERC-20 контракт 0x… (40 hex)"}))).into_response();
    }

    // Promise.all([tokenInfo, tokenHolders]) — a tokenInfo error is a 502.
    let (info_res, top) = tokio::join!(bs::token_info(&contract), bs::token_holders(&contract, 50));
    let info = match info_res {
        Ok(i) => i,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    };
    let empty = vec![];
    let holders = top["holders"].as_array().unwrap_or(&empty);

    // Attribute the top holders concurrently. A COLD token means N live Blockscout
    // address lookups (keyless → rate-limited → slow: 12-34s for 30). Cap it: fewer
    // holders + a hard time budget. Labels that miss the budget resolve on a later
    // view once the per-address cache is warm (address_meta caches 60 min). The core
    // holder/share data is unaffected — only the attribution labels degrade.
    let meta_n = holders.len().min(16);
    let meta_futs = holders[..meta_n].iter().map(|h| {
        let addr = h["address"].as_str().unwrap_or("").to_string();
        async move { bs::address_meta("eth", &addr).await.unwrap_or(Value::Null) }
    });
    let metas: Vec<Value> =
        match tokio::time::timeout(std::time::Duration::from_secs(5), join_all(meta_futs)).await {
            Ok(m) => m,
            Err(_) => vec![Value::Null; meta_n], // over budget — attribute on next view
        };

    Json(concentration::build(&contract, &info, holders, &metas)).into_response()
}


/// `GET /api/exchange-netflow?contract=0x…` — token flow to/from exchanges →
/// accumulation/distribution verdict. Ports index.js:1065: fetch the price chart
/// (last point = current price) and the token's global transfers concurrently,
/// then run the golden-tested `exchange_netflow` kernel. Deterministic given the
/// two fetches; both degrade to empty on error, mirroring the JS graceful path.
pub async fn exchange_netflow_route(Query(q): Query<ContractQuery>) -> impl IntoResponse {
    let raw = q.contract.unwrap_or_default();
    let contract = raw.trim().to_lowercase();
    if !valid_addr(&contract) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібен ERC-20 контракт 0x… (40 hex)"}))).into_response();
    }
    // Promise.all([prices.priceChart(contract,"eth"), bs.tokenTransfersGlobal(contract,6)])
    let (series, raw_transfers) = tokio::join!(prices::price_chart(&contract, "eth"), bs::token_transfers_global(&contract, 6));
    let px = series.last().and_then(|s| s["price"].as_f64()).unwrap_or(0.0);
    let transfers: Vec<exchange_netflow::Transfer> =
        raw_transfers.iter().map(|v| serde_json::from_value(v.clone()).unwrap_or_default()).collect();
    Json(exchange_netflow::exchange_netflow(&contract, px, &transfers)).into_response()
}


/// `GET /api/wallet-graph?address=…` — counterparty graph (index.js:379-423).
/// EVM: aggregate counterparties from ERC-20 transfers across every Blockscout
/// chain (2 pages each). BTC/Tron/Solana: their explorer counterparty walks.
/// entityOf/labelOf are resolved here and injected, then the golden-tested
/// `wallet_graph` kernel assembles the top-40 node/edge graph.
pub async fn wallet_graph_route(State(s): State<Ctx>, Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let raw = q.address.unwrap_or_default();
    let raw = raw.trim();
    let ty = nonevm::chain_type(raw);
    if ty == "unknown" {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Невідомий формат адреси (EVM 0x…, BTC, Tron T…, Solana)"}))).into_response();
    }
    let address = if ty == "evm" { raw.to_lowercase() } else { raw.to_string() };

    let mut cp_json: Vec<Value> = Vec::new();
    let mut total_tx: i64 = 0;
    let mut active_chains: Vec<String> = Vec::new();

    if ty == "evm" {
        let futs = bs::bs_chains().iter().map(|&c| {
            let addr = address.clone();
            async move { (c, bs::transfers_rich(c, &addr, 2).await) }
        });
        let results: Vec<(&str, Vec<Value>)> = join_all(futs).await;
        // ordered counterparty map: (addr, count, name, contract, chains)
        let mut cp: Vec<(String, i64, String, bool, Vec<String>)> = Vec::new();
        for (chain, trs) in &results {
            for it in trs {
                total_tx += 1;
                let from = it["from"].as_str().unwrap_or("");
                let (other, name, contract) = if from == address {
                    (it["to"].as_str().unwrap_or(""), it["toName"].as_str().unwrap_or(""), it["toContract"].as_bool().unwrap_or(false))
                } else {
                    (from, it["fromName"].as_str().unwrap_or(""), it["fromContract"].as_bool().unwrap_or(false))
                };
                if other.is_empty() || other == address {
                    continue;
                }
                if let Some(e) = cp.iter_mut().find(|(a, _, _, _, _)| a == other) {
                    e.1 += 1;
                    if !name.is_empty() && e.2.is_empty() {
                        e.2 = name.to_string();
                    }
                    e.3 = e.3 || contract;
                    if !e.4.iter().any(|c| c == chain) {
                        e.4.push((*chain).to_string());
                    }
                } else {
                    cp.push((other.to_string(), 1, name.to_string(), contract, vec![(*chain).to_string()]));
                }
            }
        }
        for (chain, trs) in &results {
            if !trs.is_empty() {
                active_chains.push((*chain).to_string());
            }
        }
        cp_json = cp.into_iter().map(|(a, c, n, ct, ch)| json!({"addr": a, "count": c, "name": n, "contract": ct, "chains": ch})).collect();
    } else {
        let out = match ty {
            "btc" => nonevm::btc_counterparties(&address).await,
            "tron" => nonevm::tron_counterparties(&address).await,
            _ => nonevm::solana_counterparties(&address).await,
        };
        if let Some((cpj, tx, chain)) = out {
            cp_json = cpj;
            total_tx = tx;
            active_chains = vec![chain.to_string()];
        }
    }

    // resolve entityOf / labelOf and inject per counterparty
    let cp_resolved: Vec<Value> = cp_json
        .into_iter()
        .map(|mut e| {
            let addr = e["addr"].as_str().unwrap_or("").to_string();
            if let Some(ent) = entities::entity_of(&addr) {
                e["ent"] = json!({ "name": ent.name, "type": ent.kind });
            }
            if let Some(name) = s.labels.label_name(&addr) {
                e["label"] = json!({ "name": name });
            }
            e
        })
        .collect();

    Json(wallet_graph::wallet_graph(&address, &cp_resolved, total_tx, &json!(active_chains), ty)).into_response()
}


#[derive(Deserialize)]
pub struct ProfileQuery {
    profile: Option<String>,
}


/// index.js requireAdmin GET routes — viewers get 403 here.
pub const ADMIN_PATHS: &[&str] = &["/api/portfolio-backtest", "/api/strategy-backtest"];


/// index.js `requireAdmin` — the two GET routes above, plus every wallet mutation
/// (`POST /api/wallets`, `DELETE /api/wallets/:id`). GET /api/wallets stays open.
pub fn requires_admin(method: &axum::http::Method, path: &str) -> bool {
    ADMIN_PATHS.contains(&path)
        || (path.starts_with("/api/wallets")
            && (method == axum::http::Method::POST || method == axum::http::Method::DELETE))
}


/// index.js `clientIp` — first x-forwarded-for entry, else the socket peer.
pub fn client_ip(req: &Request, peer: SocketAddr) -> String {
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| peer.ip().to_string())
}


/// The index.js:122-172 gateway stack as one axum middleware, in Express order:
/// abuse-ban gate → Basic auth (+ brute-force lockout) → admin-path 403 →
/// per-IP rate limit for viewers. /webhook/moralis bypasses all of it.
pub async fn gate_mw(State(auth): State<Arc<auth::AuthState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let now = now_ms();
    let path = req.uri().path().to_string();
    // /webhook/moralis authenticates by x-signature; /ws is attached to the raw
    // HTTP server in Node (the `ws` lib bypasses Express middleware) — both skip auth.
    // Сторінка входу й самі ендпоінти входу мають бути доступні без авторизації,
    // інакше користувач не має де ввести пароль.
    if path == "/webhook/moralis" || path == "/ws"
        || path == "/login.html" || path == "/api/login" || path == "/api/logout" {
        return next.run(req).await;
    }
    let ip = client_ip(&req, peer);

    // 1) abuse-ban gate
    let until = auth.banned_until(&ip, now);
    if until != 0 {
        let s = ((until - now) as f64 / 1000.0).ceil() as i64;
        return (StatusCode::TOO_MANY_REQUESTS, [("Retry-After", s.to_string())], Json(json!({"error": "IP тимчасово заблоковано за зловживання. Спробуйте пізніше."}))).into_response();
    }

    // 2) auth (local mode = admin when no passwords set)
    let role = if auth.cfg.no_passwords() {
        auth::Role::Admin
    } else {
        let cookie = req.headers().get("cookie").and_then(|v| v.to_str().ok()).unwrap_or("");
        // 1) жива сесія з форми входу
        if let Some(r) = auth.check_session(cookie, now) {
            r
        } else {
            // 2) Basic — для скриптів і будь-яких програмних клієнтів
            let hdr = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
            // Браузер (просить HTML) отримує сторінку входу, а не нативний діалог:
            // саме WWW-Authenticate змушував Chrome показувати модальне вікно.
            let wants_html = req.headers().get("accept")
                .and_then(|v| v.to_str().ok())
                .map(|a| a.contains("text/html"))
                .unwrap_or(false);
            match auth.check_auth(&ip, hdr, now) {
                auth::AuthOutcome::Locked => return (StatusCode::TOO_MANY_REQUESTS, "🔒 Забагато невдалих спроб. Зачекайте кілька хвилин.").into_response(),
                auth::AuthOutcome::Unauthorized => {
                    return if wants_html {
                        axum::response::Redirect::to("/login.html").into_response()
                    } else {
                        (StatusCode::UNAUTHORIZED,
                         [("WWW-Authenticate", "Basic realm=\"Wallet Radar\"")],
                         "🔒 Wallet Radar — потрібен пароль").into_response()
                    };
                }
                auth::AuthOutcome::Ok(r) => r,
            }
        }
    };

    // 3) admin-only paths
    if role == auth::Role::Viewer && requires_admin(req.method(), &path) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "Режим перегляду — зміна даних недоступна"}))).into_response();
    }

    // 4) rate limit — viewers only
    if role == auth::Role::Viewer {
        let rr = auth.check_rate(&ip, now);
        if !rr.allowed {
            auth.record_abuse_strike(&ip, now);
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    ("Retry-After", rr.reset.to_string()),
                    ("RateLimit-Limit", rr.limit.to_string()),
                    ("RateLimit-Remaining", rr.remaining.to_string()),
                    ("RateLimit-Reset", rr.reset.to_string()),
                ],
                Json(json!({"error": "Забагато запитів — зачекайте трохи"})),
            ).into_response();
        }
        let mut resp = next.run(req).await;
        let h = resp.headers_mut();
        h.insert("RateLimit-Limit", rr.limit.to_string().parse().unwrap());
        h.insert("RateLimit-Remaining", rr.remaining.to_string().parse().unwrap());
        h.insert("RateLimit-Reset", rr.reset.to_string().parse().unwrap());
        return resp;
    }

    next.run(req).await
}


/// `GET /api/openapi.json` — index.js:287. The static OpenAPI 3.0 spec
/// (server/openapi.js). Served as the exact `JSON.stringify(openapiSpec)` bytes
/// so it is byte-identical to Express `res.json` — no reserialization.
pub async fn openapi_route() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json; charset=utf-8")],
        include_str!("openapi_spec.json"),
    )
}


/// portfolio-backtest.js CEX set — hot wallets excluded from the whale cohort.
/// Перелік у базі, тег `routes:pb_cex`.



/// portfolio-backtest.js `assetWhaleEvents(contract)` — reconstruct the whale
/// cohort's in/out transfers of ONE token (top 12 non-CEX holders × 3 pages),
/// sorted by ts, for the E-timeline. Fetch layer; the eTimeline fold is the
/// kernel's job.
pub async fn asset_whale_events(contract: &str) -> Vec<Value> {
    let top = bs::token_holders(contract, 24).await;
    let empty = vec![];
    let re = regex::Regex::new(r"^0x[0-9a-f]{40}$").unwrap();
    let holders: Vec<String> = top["holders"].as_array().unwrap_or(&empty).iter()
        .map(|h| h["address"].as_str().unwrap_or("").to_lowercase())
        .filter(|a| {
            re.is_match(a)
                && !wr_store::snapshot().has_tag("eth", a, wr_store::tags::ROUTES_PB_CEX)
                && a != contract
        })
        .take(12)
        .collect();
    let per: Vec<Vec<Value>> = join_all(holders.iter().map(|h| bs::token_transfers_by_token("eth", h, contract, 3))).await;
    let mut ev: Vec<Value> = Vec::new();
    for (i, h) in holders.iter().enumerate() {
        for tr in &per[i] {
            let ts = tr["ts"].as_f64().unwrap_or(0.0);
            if ts == 0.0 {
                continue;
            }
            let from = tr["from"].as_str().unwrap_or("");
            let to = tr["to"].as_str().unwrap_or("");
            if to == h && from != h {
                ev.push(json!({ "ts": tr["ts"], "side": "in", "amount": tr["amount"] }));
            } else if from == h && to != h {
                ev.push(json!({ "ts": tr["ts"], "side": "out", "amount": tr["amount"] }));
            }
        }
    }
    ev.sort_by(|a, b| a["ts"].as_f64().unwrap_or(0.0).partial_cmp(&b["ts"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    ev
}


/// `GET /api/portfolio-backtest?profile=…` — index.js:1002-1004 (admin-gated on
/// Node; the auth layer is P2). Build the portfolio (no flowTilt → deterministic),
/// fetch each holding's price history + the satellite cohort's whale events, then
/// run the golden-tested `portfolio_backtest` assembly. Whole-run live parity is
/// limited by the same rate-limited fetch fan-out as strategy-backtest; the kernel
/// is the correctness guarantee.
pub async fn portfolio_backtest_route(Query(q): Query<ProfileQuery>) -> impl IntoResponse {
    let profile = q.profile.unwrap_or_else(|| "moderate".to_string());
    let uni = universe::active_universe();
    let pf = portfolio::build_portfolio(&uni, &profile, &json!({}), 50);

    // contractOf : sym -> contract
    let mut contract_of = serde_json::Map::new();
    for a in &uni {
        if let Some(sym) = a["sym"].as_str() {
            contract_of.insert(sym.to_string(), a["c"].clone());
        }
    }

    // syms in weights order (core+sat+stable holdings)
    let mut syms: Vec<String> = Vec::new();
    for sl in ["core", "satellite", "stable"] {
        for h in pf["sleeves"][sl]["holdings"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let s = h["sym"].as_str().unwrap_or("").to_string();
            if !syms.contains(&s) {
                syms.push(s);
            }
        }
    }

    // charts per sym: stables -> "stable"; else priceChart if len>5
    let mut charts = serde_json::Map::new();
    for s in &syms {
        if portfolio_backtest::is_stable(s) {
            charts.insert(s.clone(), json!("stable"));
            continue;
        }
        let c = contract_of.get(s).and_then(|v| v.as_str()).unwrap_or("");
        if c.is_empty() {
            continue;
        }
        let ser = prices::price_chart(c, "eth").await;
        if ser.len() > 5 {
            charts.insert(s.clone(), Value::Array(ser));
        }
    }

    // whale events per satellite holder
    let mut whale = serde_json::Map::new();
    for h in pf["sleeves"]["satellite"]["holdings"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let sym = h["sym"].as_str().unwrap_or("").to_string();
        let c = contract_of.get(&sym).and_then(|v| v.as_str()).unwrap_or("");
        if !c.is_empty() {
            whale.insert(sym, Value::Array(asset_whale_events(c).await));
        }
    }

    Json(portfolio_backtest::portfolio_backtest(&pf, &Value::Object(contract_of), &Value::Object(charts), &Value::Object(whale))).into_response()
}


/// `GET /api/portfolio-strategy?profile=…` — index.js:996-999. Build the
/// index.js:996 — Core-Satellite portfolio for the requested risk profile, with the
/// live per-asset DCA `flowTilt` from the phase worker (accumulation → overweight,
/// distribution/exit → underweight). The tilt is only non-empty once that worker has
/// scanned at least once; an empty tilt leaves phaseW at 1.0, i.e. pure size/CLARITY
/// weighting, which is what this route used to return unconditionally.
pub async fn portfolio_strategy_route(Query(q): Query<ProfileQuery>) -> impl IntoResponse {
    let profile = q.profile.unwrap_or_else(|| "moderate".to_string());
    Json(portfolio::build_portfolio(&universe::active_universe(), &profile, &phases::flow_tilt(), 50)).into_response()
}


/// index.js `bump(ts,out,usd)` — fold a transfer into the monthly UTC buckets and
/// track first/last timestamps. `ts` is epoch-ms; `!ts` (0) is ignored.
pub fn cluster_bump(monthly: &mut serde_json::Map<String, Value>, first_ts: &mut f64, last_ts: &mut f64, ts: f64, out: bool, usd: f64) {
    if ts == 0.0 {
        return;
    }
    if *first_ts == 0.0 || ts < *first_ts {
        *first_ts = ts;
    }
    if ts > *last_ts {
        *last_ts = ts;
    }
    use chrono::{Datelike, TimeZone, Utc};
    let dt = Utc.timestamp_millis_opt(ts as i64).single().unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap());
    let key = format!("{}-{:02}", dt.year(), dt.month());
    let m = monthly.entry(key).or_insert_with(|| json!({"inUsd": 0.0, "outUsd": 0.0, "txIn": 0, "txOut": 0}));
    if out {
        m["outUsd"] = json!(m["outUsd"].as_f64().unwrap_or(0.0) + usd);
        m["txOut"] = json!(m["txOut"].as_i64().unwrap_or(0) + 1);
    } else {
        m["inUsd"] = json!(m["inUsd"].as_f64().unwrap_or(0.0) + usd);
        m["txIn"] = json!(m["txIn"].as_i64().unwrap_or(0) + 1);
    }
}


/// index.js:474-478 — upsert a counterparty into the ordered `cp` list (first-seen
/// order, matching the JS Map). Each entry accumulates name/contract/chains and
/// directional counts + USD.
pub fn cluster_upsert_cp(cp: &mut Vec<Value>, other: &str, name: &str, contract: bool, chain: &str, out: bool, usd: f64) {
    let idx = cp.iter().position(|e| e["addr"].as_str() == Some(other));
    let e = match idx {
        Some(i) => &mut cp[i],
        None => {
            cp.push(json!({"addr": other, "name": "", "contract": false, "inCount": 0, "outCount": 0, "inUsd": 0.0, "outUsd": 0.0, "chains": []}));
            cp.last_mut().unwrap()
        }
    };
    if !name.is_empty() && e["name"].as_str().unwrap_or("").is_empty() {
        e["name"] = json!(name);
    }
    let ec = e["contract"].as_bool().unwrap_or(false);
    e["contract"] = json!(ec || contract);
    let has = e["chains"].as_array().map(|a| a.iter().any(|c| c.as_str() == Some(chain))).unwrap_or(false);
    if !has {
        e["chains"].as_array_mut().unwrap().push(json!(chain));
    }
    if out {
        e["outCount"] = json!(e["outCount"].as_i64().unwrap_or(0) + 1);
        e["outUsd"] = json!(e["outUsd"].as_f64().unwrap_or(0.0) + usd);
    } else {
        e["inCount"] = json!(e["inCount"].as_i64().unwrap_or(0) + 1);
        e["inUsd"] = json!(e["inUsd"].as_f64().unwrap_or(0.0) + usd);
    }
}


/// `GET /api/wallet-clusters?address=…` — index.js:442-501. EVM: aggregate
/// counterparties from ERC-20 (8p) + native (4p) transfers across every
/// Blockscout chain, historically reprice for coverage, build monthly buckets and
/// a sampled EOA-cluster average balance (top-12 by turnover × RPC balance × ETH
/// price). Non-EVM: explorer counterparty walk. The golden-tested
/// `wallet_clusters` kernel then classifies clusters + intent + timeline.
pub async fn wallet_clusters_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let raw = q.address.unwrap_or_default();
    let raw = raw.trim();
    let ty = nonevm::chain_type(raw);
    if ty == "unknown" {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Невідомий формат адреси"}))).into_response();
    }
    let address = if ty == "evm" { raw.to_lowercase() } else { raw.to_string() };

    let mut cp: Vec<Value> = Vec::new();
    let mut active_chains: Vec<String> = Vec::new();
    let mut coverage = 0.0f64;
    let mut first_ts = 0.0f64;
    let mut last_ts = 0.0f64;
    let mut monthly: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut eoa_avg = Value::Null;

    if ty == "evm" {
        let eth_native: HashSet<&str> = ["eth", "base", "optimism", "arbitrum", "zksync", "scroll", "linea", "blast", "mode", "zora"].into_iter().collect();
        let futs = bs::bs_chains().iter().map(|&c| {
            let addr = address.clone();
            async move {
                let trs = bs::transfers_rich(c, &addr, 8).await;
                let nat = bs::native_transfers(c, &addr, 4).await;
                (c, trs, nat)
            }
        });
        let results: Vec<(&str, Vec<Value>, Vec<Value>)> = join_all(futs).await;
        let mut by_chain: serde_json::Map<String, Value> = serde_json::Map::new();
        for (chain, trs, nat) in results {
            let mut all = trs;
            all.extend(nat);
            if !all.is_empty() {
                by_chain.insert(chain.to_string(), Value::Array(all));
            }
        }
        coverage = prices::price_transfers_historical(&mut by_chain, &eth_native).await["coverage"].as_f64().unwrap_or(0.0);
        for (chain, trs) in by_chain.iter() {
            let empty = vec![];
            for it in trs.as_array().unwrap_or(&empty) {
                let out = it["from"].as_str() == Some(address.as_str());
                let other = if out { it["to"].as_str().unwrap_or("") } else { it["from"].as_str().unwrap_or("") };
                if other.is_empty() || other == address {
                    continue;
                }
                let name = if out { it["toName"].as_str().unwrap_or("") } else { it["fromName"].as_str().unwrap_or("") };
                let contract = if out { it["toContract"].as_bool().unwrap_or(false) } else { it["fromContract"].as_bool().unwrap_or(false) };
                let usd = it["usd"].as_f64().unwrap_or(0.0);
                let ts = it["ts"].as_f64().unwrap_or(0.0);
                cluster_upsert_cp(&mut cp, other, name, contract, chain, out, usd);
                cluster_bump(&mut monthly, &mut first_ts, &mut last_ts, ts, out, usd);
            }
        }
        active_chains = by_chain.keys().cloned().collect();

        // sampled average balance of the EOA cluster (real, ETH-mainnet)
        let eoa: Vec<Value> = cp.iter().filter(|e| wallet_clusters::category_of(e["name"].as_str().unwrap_or(""), e["contract"].as_bool().unwrap_or(false)) == "eoa").cloned().collect();
        if !eoa.is_empty() {
            let px = prices::eth_price().await;
            let mut sample = eoa;
            sample.sort_by(|a, b| {
                let sb = b["inUsd"].as_f64().unwrap_or(0.0) + b["outUsd"].as_f64().unwrap_or(0.0);
                let sa = a["inUsd"].as_f64().unwrap_or(0.0) + a["outUsd"].as_f64().unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });
            sample.truncate(12);
            let mut nz: Vec<f64> = Vec::new();
            for m in &sample {
                let addr = m["addr"].as_str().unwrap_or("");
                let b = bs::rpc_native_amount("eth", addr).await.map(|v| v * px).unwrap_or(0.0);
                if b > 0.0 {
                    nz.push(b);
                }
            }
            let avg = if nz.is_empty() { 0.0 } else { nz.iter().sum::<f64>() / nz.len() as f64 };
            eoa_avg = js_num(jsmath::js_round(avg));
        }
    } else {
        let out = match ty {
            "btc" => nonevm::btc_counterparties(&address).await,
            "tron" => nonevm::tron_counterparties(&address).await,
            _ => nonevm::solana_counterparties(&address).await,
        };
        if let Some((cpj, _tx, chain)) = out {
            for e in cpj {
                cp.push(json!({
                    "addr": e["addr"],
                    "name": e["name"].as_str().unwrap_or(""),
                    "contract": e["contract"].as_bool().unwrap_or(false),
                    "inCount": 0,
                    "outCount": e["count"],
                    "inUsd": 0.0,
                    "outUsd": 0.0,
                    "chains": [chain],
                }));
            }
            active_chains = vec![chain.to_string()];
        }
    }

    Json(wallet_clusters::wallet_clusters(&address, ty, &cp, &Value::Object(monthly), first_ts, last_ts, coverage, &json!(active_chains), &eoa_avg)).into_response()
}


/// server/index.js:326 `KNOWN` — the ten hard-coded exchange/burn addresses.
/// `HUBS = new Set(Object.keys(KNOWN))`: hubs are excluded from the neighbour
/// walk so a shared exchange doesn't collapse every wallet into one component.


/// server/index.js `neighborsOf(addr)` — the top-5 most-frequent counterparties
/// across every Blockscout chain (2 pages each), excluding contracts and hubs.
/// The frequency map is insertion-ordered (chain order, then page order) so the
/// stable count-desc sort keeps first-seen order on ties, matching V8.
pub async fn neighbors_of(addr: &str) -> Vec<String> {
    let futs = bs::bs_chains().iter().map(|c| bs::transfers_rich(c, addr, 2));
    let per: Vec<Vec<Value>> = join_all(futs).await;

    let mut freq: IndexMap<String, i64> = IndexMap::new();
    let mut is_contract: HashSet<String> = HashSet::new();
    for chain_items in &per {
        for it in chain_items {
            let out = it["from"].as_str() == Some(addr);
            let o = if out { it["to"].as_str().unwrap_or("") } else { it["from"].as_str().unwrap_or("") };
            if o.is_empty() || o == addr {
                continue;
            }
            *freq.entry(o.to_string()).or_insert(0) += 1;
            let contract = if out { truthy(&it["toContract"]) } else { truthy(&it["fromContract"]) };
            if contract {
                is_contract.insert(o.to_string());
            }
        }
    }

    let mut entries: Vec<(String, i64)> = freq
        .into_iter()
        .filter(|(o, _)| {
            !is_contract.contains(o)
                && !wr_store::snapshot().has_tag("eth", o, wr_store::tags::ROUTES_HUBS)
        })
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1)); // stable: ties keep first-seen order
    entries.into_iter().take(5).map(|(o, _)| o).collect()
}


/// server/index.js `edgeDetail(a, b)` — the first native-or-token transfer between
/// `a` and `b`, scanning chains in order (native first, then ERC-20). Native
/// amounts round to 5 dp, token amounts to 4 dp, both via JS `toFixed`.
pub async fn edge_detail(a: &str, b: &str) -> Option<Value> {
    let pair = |t: &Value| -> bool {
        let (f, to) = (t["from"].as_str(), t["to"].as_str());
        (f == Some(a) && to == Some(b)) || (f == Some(b) && to == Some(a))
    };
    for chain in bs::bs_chains() {
        let nat = bs::native_transfers(chain, a, 2).await;
        if let Some(ni) = nat.iter().find(|t| pair(t)) {
            let amount = js_num(round_fixed(ni["amount"].as_f64().unwrap_or(0.0), 5));
            let dir = if ni["from"].as_str() == Some(a) { "out" } else { "in" };
            return Some(json!({
                "chain": chain, "amount": amount, "asset": "native", "native": true,
                "ts": ni["ts"].clone(), "dir": dir
            }));
        }
        let trs = bs::transfers_rich(chain, a, 2).await;
        if let Some(it) = trs.iter().find(|t| pair(t)) {
            let amount = js_num(round_fixed(it["amount"].as_f64().unwrap_or(0.0), 4));
            let dir = if it["from"].as_str() == Some(a) { "out" } else { "in" };
            return Some(json!({
                "chain": chain, "amount": amount, "asset": it["asset"].clone(),
                "usd": it["usd"].clone(), "ts": it["ts"].clone(), "dir": dir
            }));
        }
    }
    None
}


#[derive(Deserialize)]
pub struct ConnPathQuery {
    from: Option<String>,
    to: Option<String>,
    #[serde(rename = "maxHops")]
    max_hops: Option<u32>,
}


/// `GET /api/connection-path?from=0x…&to=0x…&maxHops=8` — the bidirectional
/// neighbour BFS between two wallets, then per-edge attribution and a peel-chain
/// verdict. Reuses `connpath::connection_path` (deterministic BFS core) over the
/// live `neighbors_of`, then decorates the found path with `edges` + `pattern`.
pub async fn connection_path_route(Query(q): Query<ConnPathQuery>) -> impl IntoResponse {
    let from = q.from.unwrap_or_default();
    let from = from.trim();
    let to = q.to.unwrap_or_default();
    let to = to.trim();
    if !valid_addr(from) || !valid_addr(to) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібні дві EVM-адреси"}))).into_response();
    }
    // Math.min(15, Math.max(2, Number(maxHops) || 8)) — 0/absent/NaN all → 8.
    let max_hops = q.max_hops.filter(|&v| v != 0).unwrap_or(8).clamp(2, 15) as usize;

    let mut r = connpath::connection_path(from, to, max_hops, |node| async move { neighbors_of(&node).await }).await;

    let attach = r["found"].as_bool() == Some(true) && r["path"].as_array().map(|p| p.len() > 1).unwrap_or(false);
    if attach {
        let path: Vec<String> =
            r["path"].as_array().unwrap().iter().map(|x| x.as_str().unwrap_or("").to_string()).collect();
        let mut edges: Vec<Value> = Vec::new();
        for i in 0..path.len() - 1 {
            // { from, to, ...(edgeDetail() || {}) } — spread keeps from/to first.
            let mut edge = json!({"from": path[i], "to": path[i + 1]});
            if let Some(detail) = edge_detail(&path[i], &path[i + 1]).await {
                if let (Some(eo), Some(dobj)) = (edge.as_object_mut(), detail.as_object()) {
                    for (k, v) in dobj {
                        eo.insert(k.clone(), v.clone());
                    }
                }
            }
            edges.push(edge);
        }
        let pattern = connpath::peel_analysis(&edges);
        if let Some(ro) = r.as_object_mut() {
            ro.insert("edges".into(), Value::Array(edges));
            ro.insert("pattern".into(), pattern);
        }
    }
    Json(r).into_response()
}


/// `GET /api/ml-status` — ml-smart-money.js `mlStatus`: model metadata + the
/// top-5 features by weight. Read from sm-model.json / sm-train.json (no worker).
pub async fn ml_status_route(State(s): State<Ctx>) -> impl IntoResponse {
    let model = smart_money::model_raw(s.dataset.root()).cloned().unwrap_or(Value::Null);
    let samples = smart_money::train_len(s.dataset.root()) as i64;
    Json(smart_money::ml_status_build(&model, samples))
}


/// `GET /api/sm-predict?address=0x…` — ml-smart-money.js `mlPredict`: P(smart
/// money) from cheap structural features (flow + holdings + graph position).
/// `{available:false}` when the model isn't trained/present.
pub async fn sm_predict(State(s): State<Ctx>, Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let raw = q.address.unwrap_or_default();
    let raw = raw.trim();
    if !valid_addr(raw) {
        return bad_addr().into_response();
    }
    let addr = raw.to_lowercase();
    Json(compute_ml_predict(&s, &addr).await).into_response()
}


/// ml-smart-money.js `mlPredict` — the feature-fetch + logistic pipeline, shared
/// by `/api/sm-predict` and `/api/report`. `{available:false}` with no model.
pub async fn compute_ml_predict(s: &App, addr: &str) -> Value {
    // Smart-money prediction: fetches the wallet flow + tokens then runs the
    // model (~15-20s cold). Cache it — used by /api/sm-predict and /api/report.
    let ck = format!("mlpred:{}", addr.to_lowercase());
    if let Some(v) = crate::bs::cached_value(&ck, std::time::Duration::from_secs(600)) {
        return v;
    }
    let v = compute_ml_predict_uncached(s, addr).await;
    if v["available"].as_bool() == Some(true) {
        crate::bs::cache_value(&ck, &v);
    }
    v
}

async fn compute_ml_predict_uncached(s: &App, addr: &str) -> Value {
    let Some(model) = smart_money::model(s.dataset.root()) else {
        return json!({"available": false});
    };
    // features(): extractFlow + tokenBalances(eth/base/arbitrum), concurrently.
    let (flow, eth_b, base_b, arb_b) = tokio::join!(
        flow_of(addr),
        bs::token_balances("eth", addr),
        bs::token_balances("base", addr),
        bs::token_balances("arbitrum", addr),
    );
    let mut tokens = eth_b;
    tokens.extend(base_b);
    tokens.extend(arb_b);
    let x = smart_money::sm_features(&flow, &tokens);
    let p = smart_money::sm_prob(&model.w, model.b, &model.mean, &model.std, &x);
    // +(prob * 100).toFixed(1)
    json!({"available": true, "prob": jsmath::js_num(jsmath::round_fixed(p * 100.0, 1))})
}


#[derive(Deserialize)]
pub struct MembersQuery {
    members: Option<String>,
}


#[derive(Deserialize)]
pub struct AddressesQuery {
    addresses: Option<String>,
}


/// `GET /api/wallet-2hop?address=0x…` — index.js: the wallet's EOA neighbourhood,
/// clustered by direct links and ≥2 shared external sources. Fetches the target's
/// multi-chain transfers for the neighbours, then each neighbour's counterparties.
pub async fn wallet_2hop_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let raw = q.address.unwrap_or_default();
    let raw = raw.trim();
    if !valid_addr(raw) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "2-hop підтримує лише EVM-адреси (0x…)"}))).into_response();
    }
    let address = raw.to_lowercase();
    const C2: [&str; 5] = ["eth", "base", "arbitrum", "optimism", "polygon"];
    const NC: [&str; 3] = ["eth", "base", "arbitrum"];

    let trs: Vec<Value> = join_all(C2.iter().map(|&c| bs::transfers_rich(c, &address, 3))).await.into_iter().flatten().collect();
    let neighbors = two_hop::neighbors_2hop(&address, &trs);
    let neigh_arr = neighbors.as_array().cloned().unwrap_or_default();

    // Each neighbour's counterparty set (all from/to across NC chains, minus self).
    let cps_entries = join_all(neigh_arr.iter().map(|n| {
        let naddr = n["addr"].as_str().unwrap_or("").to_string();
        async move {
            let arr: Vec<Value> = join_all(NC.iter().map(|&c| bs::transfers_rich(c, &naddr, 2))).await.into_iter().flatten().collect();
            let mut s: HashSet<String> = HashSet::new();
            for it in &arr {
                if let Some(f) = it["from"].as_str() {
                    if !f.is_empty() {
                        s.insert(f.to_string());
                    }
                }
                if let Some(t) = it["to"].as_str() {
                    if !t.is_empty() {
                        s.insert(t.to_string());
                    }
                }
            }
            s.remove(&naddr);
            (naddr.clone(), Value::Array(s.into_iter().map(Value::String).collect()))
        }
    }))
    .await;
    let mut neigh_cps = serde_json::Map::new();
    for (k, v) in cps_entries {
        neigh_cps.insert(k, v);
    }

    Json(two_hop::clusters_2hop(&address, &neigh_arr, &Value::Object(neigh_cps))).into_response()
}


/// `GET /api/group-coordination?addresses=0x…,0x…` — index.js: shared-funder
/// coordination over ≥2 members. Fetches each member's native funders across all
/// Blockscout chains, then runs the golden-tested reduction.
pub async fn group_coordination_route(Query(q): Query<AddressesQuery>) -> impl IntoResponse {
    let raw = q.addresses.unwrap_or_default();
    // split(",") → trim → lowercase → filter(0x…40 hex) → slice(0, 8). No dedup.
    let addrs: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|a| a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .take(8)
        .collect();
    if addrs.len() < 2 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібно ≥2 EVM-адреси"}))).into_response();
    }

    // info[a] = fundersOf(a) — native transfers across every chain, per member.
    let mut info = serde_json::Map::new();
    for a in &addrs {
        let results: Vec<Value> = join_all(bs::bs_chains().iter().map(|&c| {
            let a = a.clone();
            async move { json!({"c": c, "tr": Value::Array(bs::native_transfers(c, &a, 3).await)}) }
        }))
        .await;
        info.insert(a.clone(), coordgroup::funders_of(a, &results));
    }
    let px = prices::native_prices_coord().await;
    Json(coordgroup::group_coordination(&addrs, &Value::Object(info), &px)).into_response()
}


/// groupStats/groupFlow member parsing: `new Set(parts.map(lowercase).filter(0x…40
/// hex))` then `slice(0, cap)` — lowercased, valid, unique (first-seen), capped.
pub fn dedup_valid(parts: &[&str], cap: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in parts {
        let lc = p.to_lowercase();
        let valid = lc.len() == 42 && lc.starts_with("0x") && lc[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if valid && seen.insert(lc.clone()) {
            out.push(lc);
            if out.len() >= cap {
                break;
            }
        }
    }
    out
}


/// `String(members).split(",").map(trim).filter(Boolean)` — the trimmed non-empty
/// parts, whose emptiness the routes 400 on before validation.
pub fn member_parts(raw: &str) -> Vec<&str> {
    raw.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).collect()
}


/// `GET /api/group-stats?members=0x…,0x…` — group-stats.js `groupStats`: group
/// volume + combined holdings over up to 25 members' balances.
pub async fn group_stats_route(Query(q): Query<MembersQuery>) -> impl IntoResponse {
    let raw = q.members.unwrap_or_default();
    let parts = member_parts(&raw);
    if parts.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "members?"}))).into_response();
    }
    let addrs = dedup_valid(&parts, 25);
    let per: Vec<Value> =
        join_all(addrs.iter().map(|a| async move { json!({"addr": a, "b": balances::wallet_balances(a).await}) })).await;
    Json(groups::group_stats(&per)).into_response()
}


/// `GET /api/group-flow?members=0x…,0x…` — group-flow.js `groupFlow`: the group's
/// combined exchange deposit/withdrawal graph over up to 20 members.
pub async fn group_flow_route(Query(q): Query<MembersQuery>) -> impl IntoResponse {
    let raw = q.members.unwrap_or_default();
    let parts = member_parts(&raw);
    if parts.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "members?"}))).into_response();
    }
    let addrs = dedup_valid(&parts, 20);
    let px = prices::eth_price().await;
    let per: Vec<Value> = join_all(addrs.iter().map(|a| async move {
        let (nat, tok) = tokio::join!(bs::native_transfers("eth", a, 2), bs::transfers_rich("eth", a, 2));
        json!({"addr": a, "flows": groups::group_flow_member(a, &nat, &tok, px)})
    }))
    .await;
    Json(groups::group_flow_reduce(&per)).into_response()
}


#[derive(Deserialize)]
pub struct NativeBalanceQuery {
    chain: Option<String>,
    address: Option<String>,
}


/// `GET /api/native-balance?chain=eth&address=0x…` — native-coin amount (float)
/// from a public RPC node. A fetch failure is a 502, as index.js does.
pub async fn native_balance_route(Query(q): Query<NativeBalanceQuery>) -> impl IntoResponse {
    let chain = q.chain.unwrap_or_else(|| "eth".to_string());
    let chain = chain.trim();
    let address = q.address.unwrap_or_default();
    let address = address.trim();
    if !valid_addr(address) {
        return bad_addr().into_response();
    }
    match bs::rpc_native_amount(chain, address).await {
        // address is echoed as received (trimmed, original case), matching Node.
        Some(balance) => Json(json!({"chain": chain, "address": address, "balance": jsmath::js_num(balance)})).into_response(),
        None => {
            let msg = if bs::rpc_has(chain) { "усі вузли недоступні".to_string() } else { format!("Непідтримувана мережа: {chain}") };
            (StatusCode::BAD_GATEWAY, Json(json!({"error": msg}))).into_response()
        }
    }
}


/// `GET /api/evm-balances?address=0x…` — native balances across every RPC chain
/// at once; a per-chain failure yields `{chain, balance:null, error}`.
pub async fn evm_balances_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let address = q.address.unwrap_or_default();
    let address = address.trim();
    if !valid_addr(address) {
        return bad_addr().into_response();
    }
    let results: Vec<Value> = join_all(bs::rpc_chains().iter().map(|&c| async move {
        match bs::rpc_native_amount(c, address).await {
            Some(balance) => json!({"chain": c, "balance": jsmath::js_num(balance)}),
            None => json!({"chain": c, "balance": Value::Null, "error": "усі вузли недоступні"}),
        }
    }))
    .await;
    Json(json!({"address": address, "balances": results})).into_response()
}


/// `GET /api/report?address=0x…` — report.js `walletReport`: the synthesized
/// wallet report. Fetches balances, exchange netflow, PnL score and the ML
/// prediction concurrently (each with the JS per-source timeout + fallback),
/// looks up the conductor record, then runs the golden-tested synthesis kernel.
pub async fn report_route(State(s): State<Ctx>, Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let a = q.address.unwrap_or_default();
    let a = a.trim();
    if !valid_addr(a) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібна адреса 0x…"}))).into_response();
    }
    let addr = a.to_lowercase();

    use std::time::Duration;
    // Promise.all with withTimeout(p, ms, fallback) per source.
    let (bal, nf, score, ml) = tokio::join!(
        async {
            tokio::time::timeout(Duration::from_secs(50), balances::wallet_balances(&addr))
                .await
                .unwrap_or_else(|_| json!({"totalUsd": 0, "tokens": []}))
        },
        async {
            tokio::time::timeout(Duration::from_secs(40), compute_wallet_netflow(&addr))
                .await
                .unwrap_or_else(|_| json!({"netflows": []}))
        },
        async { tokio::time::timeout(Duration::from_secs(40), compute_wallet_score(&addr)).await.unwrap_or(Value::Null) },
        async {
            tokio::time::timeout(Duration::from_secs(20), compute_ml_predict(&s, &addr))
                .await
                .unwrap_or_else(|_| json!({"available": false}))
        },
    );

    // conductorOf: conductors.json → find funder === address.
    let cond = s
        .dataset
        .read("conductors.json")
        .and_then(|d| d["conductors"].as_array().and_then(|a| a.iter().find(|c| c["funder"].as_str() == Some(addr.as_str())).cloned()))
        .unwrap_or(Value::Null);

    Json(report::build_report(&addr, &bal, &nf, &score, &ml, &cond, now_ms())).into_response()
}


/// `GET /api/asset-groups?contract=0x…` — index.js: coordinated holder groups of
/// a token (union-find over shared external sources, hubs excluded). Fetches
/// tokenInfo + top-50 holders, then the top-18's counterparties.
pub async fn asset_groups_route(Query(q): Query<ContractQuery>) -> impl IntoResponse {
    let contract = q.contract.unwrap_or_default();
    let contract = contract.trim().to_string();
    if !valid_addr(&contract) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібен ERC-20 контракт 0x… (40 hex)"}))).into_response();
    }

    // Promise.all([tokenInfo, tokenHolders]) — a tokenInfo error is a 502.
    let (info_res, top) = tokio::join!(bs::token_info(&contract), bs::token_holders(&contract, 50));
    let info = match info_res {
        Ok(i) => i,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    };

    let empty = vec![];
    let holders: Vec<Value> = top["holders"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|h| {
            let address = h["address"].as_str().unwrap_or("").to_lowercase();
            let share = jsmath::round_fixed(h["share"].as_f64().filter(|s| *s != 0.0 && !s.is_nan()).unwrap_or(0.0), 3);
            json!({"address": address, "share": jsmath::js_num(share)})
        })
        .filter(|h| {
            let a = h["address"].as_str().unwrap_or("");
            a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .collect();

    let n = std::cmp::min(18, holders.len());
    let big: Vec<Value> = holders[..n].to_vec();

    // Each big holder's counterparty set (all from/to across 2 pages, minus self).
    let cps_entries = join_all(big.iter().map(|h| {
        let addr = h["address"].as_str().unwrap_or("").to_string();
        async move {
            let arr = bs::transfers_rich("eth", &addr, 2).await;
            let mut s: HashSet<String> = HashSet::new();
            for it in &arr {
                if let Some(f) = it["from"].as_str() {
                    if !f.is_empty() {
                        s.insert(f.to_string());
                    }
                }
                if let Some(t) = it["to"].as_str() {
                    if !t.is_empty() {
                        s.insert(t.to_string());
                    }
                }
            }
            s.remove(&addr);
            (addr.clone(), Value::Array(s.into_iter().map(Value::String).collect()))
        }
    }))
    .await;
    let mut cps = serde_json::Map::new();
    for (k, v) in cps_entries {
        cps.insert(k, v);
    }

    let symbol = info["symbol"].as_str().unwrap_or("");
    Json(token_groups::token_groups(&contract, symbol, &big, &Value::Object(cps))).into_response()
}



pub fn assetflow_known_cex(a: &str) -> bool {
    wr_store::snapshot().has_tag("eth", a, wr_store::tags::ROUTES_ASSETFLOW_KNOWN_CEX)
}


/// `GET /api/asset-flow?contract=0x…` — assetflow.js `computeAssetFlow`: whale
/// entry/exit points, PnL phase, liquidity tier, profit leaderboard and pump
/// signal for a token. Deterministic reduction over non-deterministic market/social.
///
/// Раніше цей роут ДУБЛЮВАВ увесь конвеєр compute_asset_flow_value — рядок у рядок, аж до
/// фільтрів холдерів. Два місця, де живе один розрахунок, неминуче розходяться; а коли я
/// повернув загублений кеш `aflow`, стало ще й гірше — він накрив лише одну з копій, і роут
/// далі платив повну ціну (~120 запитів + виклик LunarCrush) на КОЖЕН запит. У Node дубля немає:
/// там і роут, і strategy кличуть одну `computeAssetFlow`, обгорнуту в `cached()`.
pub async fn asset_flow_route(Query(q): Query<ContractQuery>) -> impl IntoResponse {
    let contract = q.contract.unwrap_or_default();
    let contract = contract.trim().to_lowercase();
    if !valid_addr(&contract) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібен ERC-20 контракт 0x… (40 hex)"}))).into_response();
    }
    Json(compute_asset_flow_value(&contract).await).into_response()
}


/// strategy-backtest.js UNIVERSE — the fixed liquid-ERC-20 backtest universe.



/// strategy-backtest.js KNOWN_CEX — its own 9-address exchange/burn set.
pub fn strat_known_cex(a: &str) -> bool {
    wr_store::snapshot().has_tag("eth", a, wr_store::tags::ROUTES_STRAT_KNOWN_CEX)
}


/// strategy-backtest.js `priceAt(ts)` — the series price nearest `ts` (first wins
/// on ties). Returns the raw price value.
pub fn strat_price_at(ts: f64, series: &[Value]) -> Value {
    let mut best = &Value::Null;
    let mut bd = f64::INFINITY;
    for p in series {
        let d = (p["ts"].as_f64().unwrap_or(0.0) - ts).abs();
        if d < bd {
            bd = d;
            best = &p["price"];
        }
    }
    if best.is_null() {
        jsmath::js_num(0.0)
    } else {
        best.clone()
    }
}


/// strategy-backtest.js `assetEvents(contract)` — reconstruct whale in/out events
/// from a token's top holders' transfers, priced at the nearest chart point.
/// Returns `[]` when there is too little price history (JS `null` → "мало даних").
pub async fn strat_asset_events(contract: &str) -> Vec<Value> {
    let (top, series) = tokio::join!(bs::token_holders(contract, 20), prices::price_chart(contract, "eth"));
    if series.len() < 5 {
        return vec![];
    }
    let empty = vec![];
    let holders: Vec<String> = top["holders"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|h| h["address"].as_str().unwrap_or("").to_lowercase())
        .filter(|a| {
            let valid = a.len() == 42 && a.starts_with("0x") && a[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            valid && !strat_known_cex(a) && a != contract
        })
        .take(15)
        .collect();

    let per = join_all(holders.iter().map(|h| bs::token_transfers_by_token("eth", h, contract, 3))).await;
    let mut events: Vec<Value> = Vec::new();
    for (i, h) in holders.iter().enumerate() {
        for tr in &per[i] {
            if tr["ts"].as_f64().unwrap_or(0.0) == 0.0 {
                continue; // !tr.ts
            }
            let price = strat_price_at(tr["ts"].as_f64().unwrap_or(0.0), &series);
            let (from, to) = (tr["from"].as_str(), tr["to"].as_str());
            let hs = Some(h.as_str());
            if to == hs && from != hs {
                events.push(json!({"ts": tr["ts"].clone(), "side": "in", "amount": tr["amount"].clone(), "price": price}));
            } else if from == hs && to != hs {
                events.push(json!({"ts": tr["ts"].clone(), "side": "out", "amount": tr["amount"].clone(), "price": price}));
            }
        }
    }
    events.sort_by(|a, b| a["ts"].as_f64().unwrap_or(0.0).partial_cmp(&b["ts"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    events
}


/// `GET /api/strategy-backtest` — strategy-backtest.js `runStrategyBacktest`: the
/// DCA-behind-a-whale backtest across the universe with threshold calibration.


pub async fn strategy_backtest_route() -> impl IntoResponse {
    const THR: [f64; 5] = [0.30, 0.40, 0.48, 0.55, 0.70];
    let mut per_asset: Vec<Value> = Vec::new();
    let snap = wr_store::snapshot();
    let su: Vec<(String, String)> = snap
        .tokens_tagged(wr_store::tags::ROUTES_STRAT_UNIVERSE)
        .into_iter()
        .map(|t| (t.symbol_or_contract().to_string(), t.contract.clone()))
        .collect();
    for (sym, contract) in &su {
        let events = strat_asset_events(contract).await;
        per_asset.push(strategy_backtest::asset_result(sym, &events, &THR));
    }
    Json(strategy_backtest::strategy_summary(&per_asset, &THR, su.len()))
}


/// `GET /api/holder-assets?address=0x…` — a wallet's Ethereum assets via Ethplorer
/// `getAddressInfo`, valued and sorted. A fetch failure is a 502.
pub async fn holder_assets_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let a = q.address.unwrap_or_default();
    let a = a.trim();
    if !valid_addr(a) {
        return bad_addr().into_response();
    }
    match ethplorer::address_info(a).await {
        Ok(d) => Json(ethplorer::holder_assets_build(a, &d)).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}


/// wallet-asset-groups.js `detectGroups` network wrapper: fetch a token's top-25
/// holders, filter to real non-exchange EOAs (top 15), fetch each one's ETH
/// funders, then run the golden-tested union-find clustering.
pub async fn detect_groups_live(contract: &str, px: f64) -> Value {
    let top = bs::token_holders(contract, 25).await;
    let empty = vec![];
    let mut holders: Vec<Value> = Vec::new();
    for h in top["holders"].as_array().unwrap_or(&empty) {
        let addr = h["address"].as_str().unwrap_or("");
        let valid = addr.len() == 42 && addr.starts_with("0x") && addr[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let is_contract = h["isContract"].as_bool().unwrap_or(false);
        if valid && !is_contract && !cex::is_cex(addr) && addr != contract {
            // share: Number(h.share) || 0
            let share = h["share"].as_f64().filter(|s| *s != 0.0 && !s.is_nan()).unwrap_or(0.0);
            holders.push(json!({"addr": addr, "share": jsmath::js_num(share), "isContract": is_contract}));
            if holders.len() >= 15 {
                break;
            }
        }
    }

    // hf: each holder's fundersOfEth (fetched concurrently, assembled in order).
    let hf_entries = join_all(holders.iter().map(|h| {
        let addr = h["addr"].as_str().unwrap_or("").to_string();
        async move {
            let nat = bs::native_transfers("eth", &addr, 2).await;
            (addr.clone(), asset_groups::funders_of_eth(&addr, &nat))
        }
    }))
    .await;
    let mut hf = serde_json::Map::new();
    for (k, v) in hf_entries {
        hf.insert(k, v);
    }

    asset_groups::detect_groups(&holders, &Value::Object(hf), px)
}


/// `GET /api/wallet-asset-groups?address=0x…` — wallet-asset-groups.js
/// `walletAssetGroups`: for the wallet's top-6 Ethereum assets, the coordinated
/// whale groups among each token's holders.
pub async fn wallet_asset_groups_route(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let a = q.address.unwrap_or_default();
    let a = a.trim();
    if !valid_addr(a) {
        return bad_addr().into_response();
    }
    let address = a.to_lowercase();
    let px = prices::eth_price().await;
    let toks = bs::token_balances_raw("eth", &address).await;

    // filter(t.contract).sort(usd desc).slice(0, 6)
    let mut assets: Vec<Value> = toks.into_iter().filter(|t| truthy(&t["contract"])).collect();
    assets.sort_by(|x, y| y["usd"].as_f64().unwrap_or(0.0).partial_cmp(&x["usd"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    assets.truncate(6);

    let mut out: Vec<Value> = Vec::new();
    for asset in &assets {
        let contract = asset["contract"].as_str().unwrap_or("");
        let groups = detect_groups_live(contract, px).await;
        let has_groups = groups.as_array().map(|g| !g.is_empty()).unwrap_or(false);
        out.push(json!({
            "symbol": asset["symbol"].clone(),
            "contract": asset["contract"].clone(),
            "valueUsd": jsmath::js_num(jsmath::js_round(asset["usd"].as_f64().unwrap_or(0.0))),
            "groups": groups,
            "hasGroups": has_groups,
        }));
    }
    let with_groups = out.iter().filter(|a| truthy(&a["hasGroups"])).count();
    Json(json!({"address": address, "assetsChecked": jsmath::js_num(assets.len() as f64), "withGroups": jsmath::js_num(with_groups as f64), "assets": out})).into_response()
}


/// `GET /api/wallet-assets?address=…` — a wallet's holdings valued in USD. EVM
/// addresses go through the 15-chain Blockscout+DefiLlama `wallet_balances` plus a
/// behaviour-flow summary; BTC/Tron/Solana route to their explorers; unknown
/// formats 400. Mirrors index.js.
pub async fn wallet_assets(Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let raw = q.address.unwrap_or_default();
    let raw = raw.trim().to_string();
    let ty = nonevm::chain_type(&raw);
    if ty == "unknown" {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Невідомий формат адреси (EVM 0x… / BTC / Solana / Tron)"}))).into_response();
    }

    if ty != "evm" {
        let b = nonevm::non_evm_balances(&raw).await;
        let tokens: Vec<Value> = b["tokens"].as_array().map(|a| a.iter().take(60).cloned().collect()).unwrap_or_default();
        return Json(json!({
            "address": raw,
            "source": b["chain"],
            "chainsScanned": 1,
            "totalUsd": b["totalUsd"],
            "txCount": 0,
            "tokens": tokens,
            "flow": Value::Null,
            "nonEvm": true,
        }))
        .into_response();
    }

    let address = raw.to_lowercase();
    // Promise.all([walletBalances, extractFlow.catch(()=>null)]) — run concurrently.
    let (bal, flow) = tokio::join!(balances::wallet_balances(&address), flow_of(&address));
    let tokens: Vec<Value> = bal["tokens"].as_array().map(|a| a.iter().take(60).cloned().collect()).unwrap_or_default();
    Json(json!({
        "address": address,
        "source": "blockscout+defillama",
        "chainsScanned": bal["chainsScanned"],
        "totalUsd": bal["totalUsd"],
        "txCount": 0,
        "tokens": tokens,
        "flow": {
            "inUsd": flow["inUsd"], "outUsd": flow["outUsd"], "txTotal": flow["txTotal"],
            "chains": flow["chains"], "timespanDays": flow["timespanDays"],
        },
    }))
    .into_response()
}


#[derive(Deserialize)]
pub struct MoneyTraceQuery {
    address: Option<String>,
    depth: Option<String>,
}


/// conductor.js `traceMoneyFlow` — the BFS driver. Walks the funding graph level
/// by level from `start` (native + ERC-20, 1 page each), capped at 200 nodes /
/// 15-per-level / a hard 45s deadline, delegating each node's reduction to
/// `conductor::money_trace_node` and the final shape to `money_trace_assemble`.
/// `visited` is updated after every node so peers in the same level see each
/// other's discoveries, exactly as the JS mutates the shared Set.
pub async fn trace_money_flow(address: &str, max_depth: i64) -> Value {
    const MAX_NODES: usize = 200;
    let start = address.to_lowercase();
    let px = prices::eth_price().await;

    let mut visited: Vec<String> = vec![start.clone()];
    let mut visited_set: HashSet<String> = HashSet::from([start.clone()]);
    let mut node_depth: HashMap<String, i64> = HashMap::from([(start.clone(), 0)]);
    let mut edges: Vec<Value> = Vec::new();
    let mut cex_exits: Vec<Value> = Vec::new();
    let mut cex_withdrawals: Vec<Value> = Vec::new();
    let mut explored: i64 = 0;
    let mut depth_reached: i64 = 0;
    let mut timed_out = false;
    let mut frontier: Vec<String> = vec![start.clone()];

    let deadline = Instant::now() + std::time::Duration::from_millis(45_000);
    let mut depth = 0i64;
    while depth < max_depth && !frontier.is_empty() && (explored as usize) < MAX_NODES {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining <= std::time::Duration::from_millis(500) {
            timed_out = true;
            break;
        }
        // batch = frontier.slice(0, min(MAX_NODES - explored, 15))
        let batch_n = (MAX_NODES - explored as usize).min(15).min(frontier.len());
        let batch: Vec<String> = frontier[..batch_n].to_vec();
        explored += batch.len() as i64;

        // Race the level's fetches against the remaining budget; a timeout returns
        // the partial trace built so far (JS `Promise.race([work, timeout])`).
        let work = async {
            tokio::join!(
                join_all(batch.iter().map(|a| bs::native_transfers("eth", a, 1))),
                join_all(batch.iter().map(|a| bs::transfers_rich("eth", a, 1))),
            )
        };
        let (nat_res, tok_res) = match tokio::time::timeout(remaining, work).await {
            Ok(v) => v,
            Err(_) => {
                timed_out = true;
                break;
            }
        };

        let mut next: Vec<String> = Vec::new();
        for (k, addr) in batch.iter().enumerate() {
            let node = conductor::money_trace_node(addr, &nat_res[k], &tok_res[k], px, depth, &visited_set);
            if let Some(a) = node["edges"].as_array() {
                edges.extend(a.iter().cloned());
            }
            if let Some(a) = node["cexExits"].as_array() {
                cex_exits.extend(a.iter().cloned());
            }
            if let Some(a) = node["cexWithdrawals"].as_array() {
                cex_withdrawals.extend(a.iter().cloned());
            }
            if let Some(nn) = node["newNodes"].as_array() {
                for v in nn {
                    if let Some(s) = v.as_str() {
                        visited.push(s.to_string());
                        visited_set.insert(s.to_string());
                        node_depth.insert(s.to_string(), depth + 1);
                        depth_reached = depth_reached.max(depth + 1);
                        next.push(s.to_string());
                    }
                }
            }
        }
        frontier = next;
        depth += 1;
    }

    conductor::money_trace_assemble(&start, max_depth, depth_reached, explored, timed_out, &edges, &cex_exits, &cex_withdrawals, &visited, &node_depth)
}


/// `GET /api/money-trace?address=0x…&depth=10` — traces funding flow up to 15 hops
/// and surfaces exchange deposits/withdrawals along the way.
pub async fn money_trace_route(Query(q): Query<MoneyTraceQuery>) -> impl IntoResponse {
    let a = q.address.unwrap_or_default();
    let a = a.trim();
    // Math.max(1, Math.min(15, Number(depth) || 10)) — 0/absent/NaN → 10.
    let n = q.depth.as_deref().map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::NAN);
    let n = if n == 0.0 || n.is_nan() { 10.0 } else { n };
    let depth = n.min(15.0).max(1.0) as i64;
    if !valid_addr(a) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Потрібна адреса 0x… (40 hex)"}))).into_response();
    }
    Json(trace_money_flow(a, depth).await).into_response()
}


pub async fn passthrough(State(s): State<Ctx>, file: &'static str, fallback: Value) -> impl IntoResponse {
    let mut v = s.dataset.read_or(file, fallback);
    // ВІК ДАНИХ У САМІЙ ВІДПОВІДІ. 25 маршрутів віддають файли з диска; частину з них
    // пишуть воркери, які вимкнено (forensics — 22 доби, balances, backtest). Досі це
    // виглядало однаково зі свіжими даними: код 200, повне тіло, жодної позначки. Тому
    // кожна відповідь тепер несе `_freshness` — і людині на екрані, і програмному
    // агенту видно, коли файл останній раз змінювався.
    //
    // Поле додається лише до об'єкта: якщо корінь файла — масив, вставити ключ нікуди,
    // і тоді краще віддати як є, ніж змінювати форму відповіді.
    if let Some(obj) = v.as_object_mut() {
        let path = s.dataset.root().join(file);
        let (exists, age_sec) = match std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()) {
            Some(t) => (true, t.elapsed().map(|d| d.as_secs() as i64).unwrap_or(-1)),
            None => (false, -1),
        };
        obj.insert("_freshness".into(), json!({
            "file": file,
            "exists": exists,
            "ageSec": age_sec,
            // Порогів тут навмисне немає: «застаріло» залежить від того, як часто
            // джерело мусить оновлюватись, і вирішує це той, хто показує панель.
            "asOf": if age_sec >= 0 { json!(now_ms() / 1000 - age_sec) } else { Value::Null },
        }));
    }
    Json(v)
}


/// `GET /api/whale-positions` — whale-positions.js `whalePositions`: a
/// deterministic aggregation over asset-forensics.json + wallet-asset-scan.json
/// (not worker in-memory state). The golden-tested kernel does the work.
pub async fn whale_positions_route(State(s): State<Ctx>) -> impl IntoResponse {
    let f = s.dataset.read_or("asset-forensics.json", json!({"assets": []}));
    let sc = s.dataset.read_or("wallet-asset-scan.json", json!({"assets": []}));
    let empty: Vec<Value> = vec![];
    let fa = f["assets"].as_array().map(|v| v.as_slice()).unwrap_or(&empty);
    let sca = sc["assets"].as_array().map(|v| v.as_slice()).unwrap_or(&empty);
    Json(whale_positions::whale_positions(&[fa, sca])).into_response()
}


/// `GET /api/coordinated-networks` — coordinated-networks.js `coordinatedNetworks`.
/// pos = whale-positions kernel; conductors = conductors.json. The golden-tested
/// `coordinated_networks_core` does the deterministic union-find clustering; the
/// route then applies step 4 (market-cap enrichment): symbolContracts across three
/// dataset files + marketData per shared/collective symbol. Every field is
/// byte-equal to Node EXCEPT the external `marketCap` values (CoinGecko, drifts).
pub async fn coordinated_networks_route(State(s): State<Ctx>) -> impl IntoResponse {
    Json(coordinated_networks_value(&s).await).into_response()
}


/// The `coordinatedNetworks()` result value ({count, networks}) — shared by the
/// route and the coordinated-moves worker.
pub async fn coordinated_networks_value(s: &Arc<App>) -> Value {
    let empty: Vec<Value> = vec![];
    // pos from the whale-positions kernel
    let f = s.dataset.read_or("asset-forensics.json", json!({"assets": []}));
    let sc = s.dataset.read_or("wallet-asset-scan.json", json!({"assets": []}));
    let fa = f["assets"].as_array().map(|v| v.as_slice()).unwrap_or(&empty);
    let sca = sc["assets"].as_array().map(|v| v.as_slice()).unwrap_or(&empty);
    let wpos = whale_positions::whale_positions(&[fa, sca]);
    let pos = wpos["positions"].as_array().cloned().unwrap_or_default();

    let cond_file = s.dataset.read_or("conductors.json", json!({"conductors": []}));
    let conductors = cond_file["conductors"].as_array().cloned().unwrap_or_default();

    let core = coordinated_networks::coordinated_networks_core(&pos, &conductors);
    let nets = core.as_array().cloned().unwrap_or_default();

    // symbolContracts: first-seen symbol → lowercase contract across three files
    let cmc = s.dataset.read_or("cmc-whale-assets.json", json!({"assets": []}));
    let mut sym_c: IndexMap<String, String> = IndexMap::new();
    for file in [&cmc, &f, &sc] {
        if let Some(assets) = file["assets"].as_array() {
            for a in assets {
                let sym = a["symbol"].as_str().unwrap_or("");
                let c = a["contract"].as_str().unwrap_or("");
                if !sym.is_empty() && !c.is_empty() && !sym_c.contains_key(sym) {
                    sym_c.insert(sym.to_string(), c.to_lowercase());
                }
            }
        }
    }

    // wanted = unique(sharedSyms + collective[:10].symbol) across networks, cap 60
    let mut seen = HashSet::new();
    let mut wanted: Vec<String> = Vec::new();
    for n in &nets {
        let mut syms: Vec<String> = n["sharedSyms"].as_array().unwrap_or(&empty).iter().filter_map(|v| v.as_str().map(String::from)).collect();
        for c in n["collective"].as_array().unwrap_or(&empty).iter().take(10) {
            if let Some(sy) = c["symbol"].as_str() {
                syms.push(sy.to_string());
            }
        }
        for sy in syms {
            if seen.insert(sy.clone()) {
                wanted.push(sy);
            }
        }
    }
    wanted.truncate(60);

    // capBySym via marketData (external — marketCap drifts)
    let caps: Vec<Option<Value>> = join_all(wanted.iter().map(|sym| {
        let contract = sym_c.get(sym).cloned();
        async move {
            match contract {
                Some(c) => Some(prices::market_data(&c).await["marketCap"].clone()),
                None => None,
            }
        }
    })).await;
    let mut cap_by_sym: HashMap<String, Value> = HashMap::new();
    for (sym, cap) in wanted.iter().zip(caps) {
        if let Some(v) = cap {
            cap_by_sym.insert(sym.clone(), v);
        }
    }
    let cap_of = |sym: &str| cap_by_sym.get(sym).cloned().unwrap_or(Value::Null);
    let contract_of = |sym: &str| sym_c.get(sym).map(|c| Value::String(c.clone())).unwrap_or(Value::Null);

    // step 4: {...n, sharedAssets, collective} — collective replaced in place,
    // sharedAssets appended (matches JS object-spread key order).
    let enriched: Vec<Value> = nets.into_iter().map(|mut n| {
        let shared: Vec<Value> = n["sharedSyms"].as_array().unwrap_or(&empty).iter().map(|s| {
            let sym = s.as_str().unwrap_or("");
            json!({"symbol": sym, "contract": contract_of(sym), "marketCap": cap_of(sym)})
        }).collect();
        let coll: Vec<Value> = n["collective"].as_array().unwrap_or(&empty).iter().map(|c| {
            let sym = c["symbol"].as_str().unwrap_or("");
            let mut o = c.as_object().cloned().unwrap_or_default();
            o.insert("contract".into(), contract_of(sym));
            o.insert("marketCap".into(), cap_of(sym));
            Value::Object(o)
        }).collect();
        let obj = n.as_object_mut().unwrap();
        obj.insert("collective".into(), Value::Array(coll));
        obj.insert("sharedAssets".into(), Value::Array(shared));
        n
    }).collect();

    json!({"count": enriched.len(), "networks": enriched})
}


/// `GET /api/alpha-feed` — index.js:670 `{ feed: getAlphaFeed() }`. The forensics
/// worker loads alphaFeed from alpha-feed.json at init and persists it, so the
/// file is the served state.
pub async fn alpha_feed_route(State(s): State<Ctx>) -> impl IntoResponse {
    let feed = s.dataset.read_or("alpha-feed.json", json!([]));
    Json(json!({"feed": feed})).into_response()
}


#[derive(Deserialize)]
pub struct HistoryQuery {
    key: Option<String>,
}


/// `GET /api/history?key=…` — index.js:669. history.js is file-backed
/// (`history.json`: key → time-series). `series(key)=store[key]||[]`,
/// `latest=last element or null`. Deterministic read.
pub async fn history_route(Query(q): Query<HistoryQuery>) -> impl IntoResponse {
    // Reads the in-memory singleton, not the file: index.js:669 calls
    // history.series()/latest(), and the recorders now write into that same store,
    // so a point recorded this cycle must be visible before the next persist().
    let key = q.key.unwrap_or_default();
    Json(json!({"key": key, "series": history::series(&key), "latest": history::latest(&key)})).into_response()
}


/// `GET /api/behavior-alerts` — index.js:645 `getRecentAlerts()` — the newest 50
/// behaviour-change alerts from the in-memory ring (filled by the gated monitor).
pub async fn behavior_alerts_route(State(s): State<Ctx>) -> impl IntoResponse {
    let a = s.alerts.lock().unwrap();
    Json(Value::Array(a.iter().take(50).cloned().collect())).into_response()
}


/// `GET /ws` — index.js:1170. On connect, send `{type:"init", events: last 60}`,
/// then forward every broadcast message (`{type:"event",event}` and alerts) to the
/// client until it disconnects.
pub async fn ws_handler(State(s): State<Ctx>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_conn(socket, s))
}


pub async fn ws_conn(mut socket: WebSocket, s: Ctx) {
    let init = json!({"type": "init", "events": s.store.events(60)}).to_string();
    if socket.send(Message::Text(init.into())).await.is_err() {
        return;
    }
    let mut rx = s.events_tx.subscribe();
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(text) => { if socket.send(Message::Text(text.into())).await.is_err() { break; } }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(_)) => {}      // ignore client frames (heartbeat/pong)
                _ => break,            // closed or error
            },
        }
    }
}


/// `POST /webhook/moralis` — index.js:1137. Verify the x-signature
/// (keccak256(rawBody+secret)), map confirmed deliveries to events for tracked
/// wallets (feed::moralis_webhook_events), dedup by id:walletId, store + broadcast.
/// Outside Basic Auth (the gateway middleware bypasses this path).
pub async fn webhook_moralis(State(s): State<Ctx>, headers: HeaderMap, body: Bytes) -> Response {
    if s.webhook_secret.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "секрет вебхука не налаштований (MORALIS_STREAM_SECRET/MORALIS_API_KEY)"}))).into_response();
    }
    let sig = headers.get("x-signature").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if sig.is_empty() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "нема заголовка x-signature"}))).into_response();
    }
    // expected = "0x" + keccak256(rawBody + secret)
    let mut hasher = tiny_keccak::Keccak::v256();
    tiny_keccak::Hasher::update(&mut hasher, &body);
    tiny_keccak::Hasher::update(&mut hasher, s.webhook_secret.as_bytes());
    let mut digest = [0u8; 32];
    tiny_keccak::Hasher::finalize(hasher, &mut digest);
    let expected = format!("0x{}", digest.iter().map(|b| format!("{b:02x}")).collect::<String>());
    if expected != sig {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "невірний підпис"}))).into_response();
    }

    let body_json: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let chain_id = body_json["chainId"].as_str().unwrap_or("?").to_string();
    if !truthy(&body_json["confirmed"]) {
        tracing::info!("[webhook] Moralis chain={chain_id} unconfirmed — ack без обробки");
        return (StatusCode::OK, Json(json!({"ok": true, "skipped": "unconfirmed"}))).into_response();
    }

    let wallets = s.store.wallets();
    let empty = vec![];
    let wl = wallets.as_array().unwrap_or(&empty);
    let events = feed::moralis_webhook_events(&body_json, wl, now_ms());
    let mut fresh = 0i64;
    for ev in &events {
        let key = format!("{}:{}", ev["id"].as_str().unwrap_or(""), ev["walletId"].as_str().unwrap_or(""));
        if mark_seen(&s, &key, now_ms()) {
            continue;
        }
        s.store.add_event(ev.clone());
        broadcast_event(&s, ev);
        fresh += 1;
    }
    let deduped = events.len() as i64 - fresh;
    tracing::info!("[webhook] Moralis chain={chain_id}: {} подій (нових {fresh}, дублів {deduped})", events.len());
    (StatusCode::OK, Json(json!({"ok": true, "mapped": events.len(), "fresh": fresh, "deduped": deduped}))).into_response()
}


/// server/index.js:650-654. Band thresholds are inclusive at both ends of 40-80;
/// a missing `top10pct` falls through every comparison and lands in "<40",
/// exactly as the JS does with `undefined`.
/// `GET /api/sector-flow` — the "narratives" overview: per sector (DePIN / memecoin
/// / DeFi / RWA / other) how many whale-assets sit there, the average top-10 whale
/// concentration, how many are highly concentrated, and recent momentum (fresh
/// whale entries from the alpha feed). Everything from local files + the sector
/// classifier — instant, no live upstream.
/// `GET /api/wallet-family?address=0x…` — which whale family (funding-graph
/// component) this wallet belongs to, plus that family's summary. Links the
/// per-wallet view to the group view.
pub async fn wallet_family_route(State(s): State<Ctx>, Query(q): Query<AddressQuery>) -> impl IntoResponse {
    let addr = q.address.unwrap_or_default().trim().to_lowercase();
    if addr.is_empty() {
        return Json(json!({"inFamily": false})).into_response();
    }
    match crate::family_map::family_of(s.dataset.root(), &addr) {
        Some(fid) => {
            let fams = s.dataset.read_or("families.json", json!({"families": []}));
            let summary = fams["families"]
                .as_array()
                .and_then(|a| a.iter().find(|f| f["root"].as_str() == Some(fid.as_str())).cloned())
                .unwrap_or(Value::Null);
            Json(json!({"inFamily": true, "familyId": fid, "family": summary})).into_response()
        }
        None => Json(json!({"inFamily": false})).into_response(),
    }
}

/// `GET /api/signals` — actionable digest: which whale FAMILIES are actively
/// distributing (selling to outside) or accumulating, with their dominant
/// narrative and top share-of-emission holdings. The "what should I watch" view.
pub async fn signals_route(State(s): State<Ctx>) -> impl IntoResponse {
    let fams = s.dataset.read_or("families.json", json!({"families": []}));
    let family_signals: Vec<Value> = fams["families"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| matches!(f["phase"].as_str(), Some("distributing") | Some("accumulating")))
                .take(20)
                .map(|f| {
                    json!({
                        "root": f["root"],
                        "size": f["size"],
                        "volumeUsd": f["volumeUsd"],
                        "phase": f["phase"],
                        "sector": f["dominantSector"],
                        "transfers": f["transfers"],
                        "topEmission": f["topEmission"],
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Json(json!({
        "familiesCount": fams["familiesCount"],
        "familySignals": family_signals,
    }))
    .into_response()
}

pub async fn sector_flow_route(State(s): State<Ctx>) -> impl IntoResponse {
    sectors::init(s.dataset.root());
    let order = ["depin", "memecoin", "defi", "rwa", "other"];
    let mut count: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut sum10: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut high: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut tops: std::collections::HashMap<String, Vec<(String, f64)>> = std::collections::HashMap::new();
    let mut entries: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    let wa = s.dataset.read_or("cmc-whale-assets.json", json!({"assets": []}));
    if let Some(arr) = wa["assets"].as_array() {
        for a in arr {
            let sym = a["symbol"].as_str().unwrap_or("");
            let name = a["name"].as_str().unwrap_or("");
            let contract = a["contract"].as_str().unwrap_or("");
            let sec = sectors::sector_of(sym, name, contract);
            let t10 = a["top10pct"].as_f64().unwrap_or(0.0);
            *count.entry(sec.clone()).or_default() += 1;
            *sum10.entry(sec.clone()).or_default() += t10;
            if t10 >= 40.0 {
                *high.entry(sec.clone()).or_default() += 1;
            }
            tops.entry(sec).or_default().push((sym.to_string(), t10));
        }
    }
    let af = s.dataset.read_or("alpha-feed.json", json!([]));
    if let Some(arr) = af.as_array() {
        for e in arr {
            if let Some(ts) = e["tokens"].as_array() {
                for tk in ts {
                    if let Some(t) = tk.as_str() {
                        *entries.entry(sectors::sector_of(t, "", "")).or_default() += 1;
                    }
                }
            }
        }
    }
    let out: Vec<Value> = order
        .iter()
        .map(|&sec| {
            let c = *count.get(sec).unwrap_or(&0);
            let avg = if c > 0 { sum10.get(sec).copied().unwrap_or(0.0) / c as f64 } else { 0.0 };
            let mut ta = tops.get(sec).cloned().unwrap_or_default();
            ta.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let top_syms: Vec<String> = ta.iter().take(6).map(|(x, _)| x.clone()).collect();
            json!({
                "sector": sec,
                "assets": c,
                "avgTop10": (avg * 10.0).round() / 10.0,
                "highConc": *high.get(sec).unwrap_or(&0),
                "recentEntries": *entries.get(sec).unwrap_or(&0),
                "topAssets": top_syms,
            })
        })
        .collect();
    Json(json!({ "sectors": out })).into_response()
}

pub async fn whale_assets(State(s): State<Ctx>) -> impl IntoResponse {
    sectors::init(s.dataset.root());
    let d = {
        let g = s.dataset.read_or("capital-group-assets.json", json!({"assets": []}));
        if g["assets"].as_array().map(|a| !a.is_empty()).unwrap_or(false) { g }
        else { s.dataset.read_or("cmc-whale-assets.json", json!({"assets": []})) }
    };
    let mut assets: Vec<Value> = d["assets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut a| {
            let band = match a["top10pct"].as_f64() {
                Some(p) if (40.0..=80.0).contains(&p) => "40-80",
                Some(p) if p > 80.0 => ">80",
                _ => "<40",
            };
            let sector = sectors::sector_of(
                a["symbol"].as_str().unwrap_or(""),
                a["name"].as_str().unwrap_or(""),
                a["contract"].as_str().unwrap_or(""),
            );
            if let Some(o) = a.as_object_mut() {
                o.insert("band".into(), json!(band));
                o.insert("sector".into(), json!(sector));
            }
            a
        })
        .collect();

    // Assets without a rank sort last; JS produces NaN here and leaves them in place.
    assets.sort_by(|x, y| {
        let rx = x["rank"].as_f64().unwrap_or(f64::INFINITY);
        let ry = y["rank"].as_f64().unwrap_or(f64::INFINITY);
        rx.partial_cmp(&ry).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Background: let CoinGecko resolve the tokens the curated map/cache missed.
    let misses: Vec<(String, String)> = assets
        .iter()
        .filter(|a| a["sector"] == "other")
        .filter_map(|a| {
            let c = a["contract"].as_str().unwrap_or("");
            (!c.is_empty()).then(|| (a["symbol"].as_str().unwrap_or("").to_string(), c.to_string()))
        })
        .collect();
    if !misses.is_empty() {
        tokio::spawn(async move { crate::sectors::enrich(misses).await; });
    }
    let band4080 = assets.iter().filter(|a| a["band"] == "40-80").count();
    Json(json!({"found": assets.len(), "band4080": band4080, "assets": assets}))
}


/// server/index.js:655-664. Strips infrastructure (exchanges, bridges, issuers,
/// contracts) so only genuine coordinators remain. Three independent rejects:
/// a contract flag, a known entity, a public infra label; then a label regex.
pub async fn cross_asset(State(s): State<Ctx>) -> impl IntoResponse {
    let d = s.dataset.read_or("cross-asset.json", json!({"multiAsset": []}));

    let clean: Vec<Value> = d["multiAsset"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            let a = m["address"].as_str().unwrap_or_default().to_lowercase();
            if truthy(&m["isContract"]) || entities::entity_of(&a).is_some() || s.labels.is_infra_addr(&a) {
                return false;
            }
            !s.cex_label.is_match(m["label"].as_str().unwrap_or_default())
        })
        .collect();

    // `{ ...d, multiAsset: clean, orchestrators }` — replacing an existing key
    // keeps its original position, so key order matches Express.
    let mut out = d;
    let orchestrators = clean.len();
    if let Some(o) = out.as_object_mut() {
        o.insert("multiAsset".into(), Value::Array(clean));
        o.insert("orchestrators".into(), json!(orchestrators));
    }
    Json(out)
}


#[derive(Deserialize)]
pub struct GroupGraphQuery {
    id: Option<String>,
    limit: Option<usize>,
}

/// `GET /api/group-graph?id=NET&limit=600` — група капіталу як структура.
///
/// Вітрина `/api/capital-groups` дає групі одне число. Тут — те, що це число
/// ховає: концентратори, кільця заправлених ними адрес і мости між під-кластерами.
/// Ролі рахуються зі ступеня всередині групи, а не приписуються ззовні.
///
/// Разом із графом повертається `coverage`: скільки адрес у складі, скільки з них
/// мають хоч одне ребро і скільки показано. Різниця цих чисел — не похибка
/// малювання, а властивість даних, і ховати її не можна.
pub async fn group_graph_route(Query(q): Query<GroupGraphQuery>) -> impl IntoResponse {
    let id = q.id.unwrap_or_default();
    let id = id.trim();
    if id.is_empty() || id.len() > 32 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':') {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "потрібен id групи, напр. NET"}))).into_response();
    }
    let limit = q.limit.unwrap_or(600);
    let root = std::env::var("APP_ROOT").unwrap_or_else(|_| "/opt/wallet-tracker".into());
    // Перший виклик прогріває три мапи з диска (~80 МБ); далі вони живуть у пам'яті.
    let id_owned = id.to_string();
    let v = tokio::task::spawn_blocking(move || crate::groupgraph::build(&root, &id_owned, limit))
        .await
        .unwrap_or_else(|e| json!({"error": format!("збірка графа впала: {e}")}));
    if v.get("error").is_some() {
        return (StatusCode::NOT_FOUND, Json(v)).into_response();
    }
    Json(v).into_response()
}
