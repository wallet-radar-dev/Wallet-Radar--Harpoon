// Compute modules are complete and golden-tested but not yet wired into request
// paths (workers and network routes come in later slices) — allow dead_code
// until then rather than blanket-suppressing it crate-wide.
#[allow(dead_code)]
mod assistant;
mod glossary;
mod product_kb;
mod tools;
mod direct;
mod family_map;
mod auth;
mod backtest;
mod behavior_monitor;
mod backtest_math;
mod backup;
#[allow(dead_code)]
mod blockscout;
mod bs;
mod clarity;
#[allow(dead_code)]
mod cex;
mod active_groups;
mod asset_groups;
mod assetflow;
mod balances;
mod balances_cache;
mod concentration;
mod connpath;
mod coordgroup;
mod nonevm;
mod report;
mod smart_money;
#[allow(dead_code)] // coordination-forensics kernels wired to routes in a later slice
mod conductor;
#[allow(dead_code)] // coordinated-network clustering wired to a route in a later slice
mod coordinated_networks;
mod conductor_finder;
#[allow(dead_code)] // synchronous-move detector wired to a worker in a later slice
mod coordinated_moves;
mod dataset;
mod entities;
mod expand;
mod moralis;
mod satellite;
mod ethplorer;
mod exchange_netflow;
mod groupgraph;
mod wallet_graph;
mod cg;
mod strategy;
mod intel;
mod wallet_clusters;
mod phases;
mod portfolio;
mod portfolio_backtest;
mod universe;
mod groups;
mod feed;
mod forensics;
#[allow(dead_code)] // feature extractor wired to a route in a later slice
mod features_bs;
mod flow_features;
#[allow(dead_code)]
mod graph;
mod health;
mod history;
mod market;
#[allow(dead_code)] // used by compute modules that aren't wired to routes yet
mod jsmath;
mod labels;
mod liquidations;
#[allow(dead_code)]
mod ml;
mod ml_worker;
#[allow(dead_code)] // classifier wired to a route in a later slice
mod model;
mod prices;
mod atomicw;
mod store;
mod sectors;
mod strategy_backtest;
mod telegram;
mod watchdog;
mod token_ai;
mod token_groups;
mod two_hop;
mod wallet_scan;
mod walletscore;
#[allow(dead_code)]
mod whale_finder;
mod whale_positions;
#[allow(dead_code)]
mod whale_clusters;

use axum::{extract::State, http::StatusCode, middleware::{self}, response::IntoResponse, routing::{delete, get, post}, Json, Router};
use std::net::SocketAddr;
use dataset::Dataset;
use labels::Labels;
use regex::Regex;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use store::Store;
use tower_http::services::ServeDir;

mod state;
mod workers;
mod routes;
mod check;
/// Послужний список сигналів: чи справдились наші власні твердження.
mod trackrecord;
/// Тиск на вихід: позиція, виміряна в днях ринкового обороту, а не в доларах.
mod exitpressure;
/// Чи це рух ринку, чи біржа перекладає власні монети між своїми гаманцями.
mod cexmove;
/// Вік усіх наборів даних одним запитом: тільки метадані файлів, без читання вмісту.
mod dataages;
use crate::state::*;
use crate::workers::*;
use crate::routes::*;

/// Піднімає клієнт Sentry/GlitchTip, якщо заданий SENTRY_DSN. Повертає guard, який ловить
/// паніки й флашить чергу подій при drop. Без DSN — тихо вимкнено (як і Node-версія:
/// server/sentry.js). sample_rate=1.0 і traces_sample_rate=1.0 — «повний» режим на прохання.
fn init_sentry() -> Option<sentry::ClientInitGuard> {
    let dsn = std::env::var("SENTRY_DSN").ok().filter(|s| !s.trim().is_empty())?;
    let env = std::env::var("SENTRY_ENV").unwrap_or_else(|_| "production".into());
    let guard = sentry::init((
        dsn,
        sentry::ClientOptions {
            release: sentry::release_name!(),
            environment: Some(env.into()),
            sample_rate: 1.0,
            traces_sample_rate: 1.0,
            ..Default::default()
        },
    ));
    // NB: підписник ще не піднято, тож не логуємо тут — стартовий лог нижче все одно
    // покаже hasSentry. Guard просто повертаємо.
    Some(guard)
}


/// Static assets (index.html, app.js, app.css) get Cache-Control: no-cache so
/// the browser always revalidates (cheap 304 via etag) and never runs a stale
/// bundle after a rebuild. API / WS / webhook responses are left untouched.

/// Вхід через форму: пароль → сесійна cookie.
///
/// Окремий облік невдач тут не ведеться — його вже робить `check_auth` для
/// Basic; дублювати означало б блокувати скрипти через помилки людини.
async fn login_route(
    State(auth): State<Arc<auth::AuthState>>,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let pass = body["password"].as_str().unwrap_or("");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    match auth.role_for_password(pass) {
        Some(role) => {
            // токен із системного джерела випадковості
            // ЛИШЕ 24 байти: std::fs::read на /dev/urandom читав би нескінченно
            // й вішав обробник — саме на цьому сервер віддавав 502.
            let token = {
                use std::io::Read;
                let mut buf = [0u8; 24];
                std::fs::File::open("/dev/urandom")
                    .and_then(|mut f| f.read_exact(&mut buf))
                    .map(|_| buf.iter().map(|x| format!("{x:02x}")).collect::<String>())
                    .unwrap_or_else(|_| format!("{now:x}{:x}", std::process::id()))
            };
            auth.create_session(role, now, token.clone());
            let is_admin = role == auth::Role::Admin;
            // HttpOnly — недоступна для JS, тож XSS не викраде сесію.
            // SameSite=Strict — не піде на сторонні запити.
            let cookie = format!(
                "wr_session={token}; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age={}",
                auth::AuthState::SESSION_MS / 1000
            );
            (
                StatusCode::OK,
                [("Set-Cookie", cookie)],
                Json(json!({"ok": true, "role": if is_admin { "admin" } else { "viewer" }})),
            )
                .into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "error": "Невірний пароль"})),
        )
            .into_response(),
    }
}

/// Вихід: гасимо сесію на сервері й прибираємо cookie.
async fn logout_route(
    State(auth): State<Arc<auth::AuthState>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let cookie = headers.get("cookie").and_then(|v| v.to_str().ok()).unwrap_or("");
    auth.drop_session(cookie);
    (
        StatusCode::OK,
        [("Set-Cookie", "wr_session=; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age=0")],
        Json(json!({"ok": true})),
    )
        .into_response()
}

async fn no_cache_static(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let p = req.uri().path();
    let dynamic = p.starts_with("/api") || p.starts_with("/ws") || p.starts_with("/webhook");
    let mut resp = next.run(req).await;
    if !dynamic {
        resp.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        );
    }
    resp
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Paths resolve relative to the repo root, matching server/index.js:648
    // (`join(__dirname, "..", "dataset")`) and store.js:8. APP_ROOT must be read
    // before the .env is loaded, or the .env location depends on the cwd. .env також
    // мусить прочитатися ДО Sentry — DSN приходить саме звідти.
    let root = std::env::var("APP_ROOT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".."));
    let _ = dotenvy::from_path(root.join(".env"));

    // Sentry/GlitchTip. Guard мусить жити до кінця main (drop = flush подій), тому тримаємо
    // його у змінній. init ДО підписника: sentry-tracing нижче чіпляється на той самий шар.
    let _sentry_guard = init_sentry();

    {
        use tracing_subscriber::prelude::*;
        // RUST_LOG (systemd) керує рівнем; дефолт info, щоб не сипати TRACE (дампи TLS-сертів).
        let log_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        tracing_subscriber::registry()
            .with(log_filter)
            .with(tracing_subscriber::fmt::layer().with_target(false))
            .with(sentry_tracing::layer())
            .init();
    }

    // Перевірка каналу до сателітів без підняття 20 воркерів: `rust-server --probe-satellites`.
    // Стоїть саме тут — після .env, до всього важкого, — щоб бачити ту саму конфігурацію,
    // що й прод.
    if std::env::args().any(|a| a == "--probe-satellites") {
        return satellite::probe().await;
    }

    // Довідники wr-store мусять бути завантажені ДО першого запиту. Інакше
    // `wr_store::snapshot()` панікує посеред обробника (напр. /api/wallet-graph), і
    // браузер отримує обірване з'єднання — граф гаманця «висів» на завантаженні.
    // Як і обіцяє README: немає бази — сервер не стартує, а пояснює, що робити.
    let store = wr_store::init().map_err(|e| {
        anyhow::anyhow!(
            "wr-store: довідники недоступні ({e}). Шлях: WR_DB={}. Полагодити: `wr-import load`.",
            wr_store::db_path().display()
        )
    })?;
    // Брак тегів — не привід лягати: частина маршрутів працює і без них. Але мовчати
    // не можна, інакше порожні відповіді виглядатимуть як «даних немає».
    if let Err(e) = store.validate() {
        tracing::warn!("[wr-store] {e}");
    }

    let data_dir = std::env::var("DATA_DIR").map(PathBuf::from).unwrap_or_else(|_| root.join("server"));

    let dataset_dir = root.join("dataset");
    history::init(&dataset_dir);
    phases::init(&dataset_dir);
    wallet_scan::init(&dataset_dir);
    // Соц-кеш мусить пережити рестарт: квота LunarCrush — 100 викликів на ДОБУ, а холодний
    // старт коштує 30 (по одному на актив у циклі воркера фаз). Кілька рестартів = вигоріла квота.
    prices::init(&dataset_dir);
    ml_worker::init(&dataset_dir);
    let labels = Labels::load(&dataset_dir);
    tracing::info!("[labels] loaded {} from disk", labels.len());

    let (events_tx, _) = tokio::sync::broadcast::channel::<String>(256);
    let webhook_secret = std::env::var("MORALIS_STREAM_SECRET").ok().filter(|s| !s.is_empty())
        .or_else(|| std::env::var("MORALIS_API_KEY").ok())
        .unwrap_or_default();

    // conductor-finder.js:47 — seed `pushed` with the 4+-asset conductors already on
    // disk, so a restart re-announces nobody.
    let dataset = Dataset::new(&dataset_dir);
    let pushed_conductors: HashSet<String> = dataset
        .read("conductors.json")
        .and_then(|v| v["conductors"].as_array().cloned())
        .map(|cs| {
            cs.iter()
                .filter(|c| c["assetCount"].as_i64().unwrap_or(0) >= 4)
                .filter_map(|c| c["funder"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let forensics_state = ForensicsState::load(&dataset);
    let graph = expand::Graph::load(&dataset_dir.join("graph-db.json"));
    tracing::info!("[forensics] loaded {} assets, {} fingerprints", forensics_state.results.len(), forensics_state.fingerprints.len());
    tracing::info!("[expand] loaded graph: {} nodes, frontier {}", graph.nodes.len(), graph.frontier.len());

    let app = Arc::new(App {
        dataset,
        store: Store::load(data_dir.clone()),
        labels,
        cex_label: Regex::new(CEX_LABEL_RE).expect("CEX_LABEL_RE is a compile-time constant"),
        started: Instant::now(),
        events_tx,
        seen: std::sync::Mutex::new(HashMap::new()),
        webhook_secret,
        alerts: std::sync::Mutex::new(Vec::new()),
        idem: std::sync::Mutex::new(HashMap::new()),
        pushed_conductors: std::sync::Mutex::new(pushed_conductors),
        forensics: Arc::new(std::sync::Mutex::new(forensics_state)),
        graph: Arc::new(std::sync::Mutex::new(graph)),
        alert_seq: std::sync::atomic::AtomicU64::new(0),
    });

    let port: u16 = std::env::var("RUST_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3001);

    // feed.js startBlockscout — the live event poller (30s). OFF by default so it
    // never double-writes the shared data.json while Node owns it; enable only when
    // rust is the sole server (ENABLE_FEED_WORKER=1).
    if std::env::var("ENABLE_FEED_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            loop {
                poll_blockscout_once(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
        tracing::info!("[feed] Blockscout poller ENABLED (30s polling)");
    } else {
        tracing::info!("[feed] Blockscout poller OFF (set ENABLE_FEED_WORKER=1 when rust is sole server)");
    }

    // behavior-monitor.js — periodic behaviour-change scan of armed wallets. Also
    // gated (writes baselines to the shared data.json). Interval = BEHAVIOR_INTERVAL_MIN
    // (min 2, default 20).
    if std::env::var("ENABLE_BEHAVIOR_WORKER").ok().as_deref() == Some("1") {
        let mins = std::env::var("BEHAVIOR_INTERVAL_MIN").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(20).max(2);
        let app2 = app.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(mins * 60)).await;
                behavior_tick(&app2).await;
            }
        });
        tracing::info!("[behavior] monitor ENABLED (every {mins} min, armed wallets only)");
    } else {
        tracing::info!("[behavior] monitor OFF (set ENABLE_BEHAVIOR_WORKER=1 when rust is sole server)");
    }

    // active-groups.js — refresh active-groups.json from whale-clusters + 7d activity.
    // Gated (writes the shared dataset file). ~every 1.5h, first after 60s.
    if std::env::var("ENABLE_ACTIVE_GROUPS_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                active_groups_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(90 * 60)).await;
            }
        });
        tracing::info!("[active] active-groups worker ENABLED (~1.5h cycle)");
    } else {
        tracing::info!("[active] active-groups worker OFF (set ENABLE_ACTIVE_GROUPS_WORKER=1 when rust is sole server)");
    }

    // coordinated-moves.js — synchronous-move detector. Gated (writes the shared
    // dataset file). ~every 15 min, first after 2.5 min.
    if std::env::var("ENABLE_COORD_MOVES_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(150)).await;
            loop {
                coordinated_moves_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
            }
        });
        tracing::info!("[coord-moves] worker ENABLED (~15 min cycle)");
    } else {
        tracing::info!("[coord-moves] worker OFF (set ENABLE_COORD_MOVES_WORKER=1 when rust is sole server)");
    }

    // universe.js — daily CoinGecko re-sizing of the asset universe. Gated: rewrites
    // the shared universe.json. Without it the portfolio's asset sizes freeze.
    if std::env::var("ENABLE_UNIVERSE_WORKER").ok().as_deref() == Some("1") {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(45)).await;
            loop {
                universe::build_universe().await;
                tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
            }
        });
        tracing::info!("[universe] worker ENABLED (~24h cycle)");
    } else {
        tracing::info!("[universe] worker OFF (set ENABLE_UNIVERSE_WORKER=1 when rust is sole server)");
    }

    // strategy-phases.js — scan each satellite candidate's DCA phase. This is what
    // /api/portfolio-strategy's flowTilt reads. Gated (writes strategy-phases.json).
    if std::env::var("ENABLE_PHASES_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(90)).await;
            loop {
                phases_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
            }
        });
        tracing::info!("[phases] worker ENABLED (~6h cycle)");
    } else {
        tracing::info!("[phases] worker OFF (set ENABLE_PHASES_WORKER=1 when rust is sole server)");
    }

    // forensics-worker.js — the deep continuous scan that produces asset-forensics.json
    // and whale-clusters.json, which conductor-finder and the whole coordination story
    // are built on. Gated: it rewrites both files (and alpha-feed.json).
    if std::env::var("ENABLE_FORENSICS_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(8)).await;
            loop {
                forensics_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
            }
        });
        tracing::info!("[forensics] worker ENABLED (top-{} holders/asset, ~30 min cycle)", forensics::HOLDERS);
    } else {
        tracing::info!("[forensics] worker OFF (set ENABLE_FORENSICS_WORKER=1 when rust is sole server)");
    }

    // expand-worker.js — the frontier graph crawler (graph-db.json + discovered-groups.json).
    // Gated, and gentle by construction: one node per 6s.
    if std::env::var("ENABLE_EXPAND_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let (mut since_p, mut since_c, mut last_c) = (0u32, 0u32, 0i64);
            loop {
                expand_tick(&app2, &mut since_p, &mut since_c, &mut last_c).await;
                tokio::time::sleep(std::time::Duration::from_millis(expand::TICK_MS)).await;
            }
        });
        tracing::info!("[expand] crawler ENABLED (1 node / ~{}s)", expand::TICK_MS / 1000);
    } else {
        tracing::info!("[expand] crawler OFF (set ENABLE_EXPAND_WORKER=1 when rust is sole server)");
    }

    // Кеш балансів усіх гаманців → wallet-balances.json (його читає фронт замість того, щоб
    // добувати 507 балансів із браузера). Повний обхід довгий (десятки хвилин на 500+ гаманців),
    // тож цикл — раз на 2 години; перший старт відкладено, щоб не битися з рештою воркерів за
    // зовнішні API одразу після запуску.
    if std::env::var("ENABLE_BALANCES_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            loop {
                balances_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(2 * 3600)).await;
            }
        });
        tracing::info!("[balances] worker ENABLED (кеш балансів, цикл 2 год)");
    } else {
        tracing::info!("[balances] worker OFF (set ENABLE_BALANCES_WORKER=1)");
    }

    // wallet-asset-scan.js — every watched wallet's acquired tokens, scanned for whale
    // groups. Gated: it rewrites wallet-asset-scan.json, which conductor-finder reads.
    if std::env::var("ENABLE_WALLET_SCAN_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(75)).await;
            loop {
                wallet_scan_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(3 * 3600)).await;
            }
        });
        tracing::info!("[wscan] worker ENABLED (~3h cycle)");
    } else {
        tracing::info!("[wscan] worker OFF (set ENABLE_WALLET_SCAN_WORKER=1 when rust is sole server)");
    }

    // backtest.js — the event backtest of the "whale entered" signal. Gated: writes
    // backtest.json. Slow by design (2.5s between assets), so it runs every 3h.
    if std::env::var("ENABLE_BACKTEST_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            loop {
                backtest_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(3 * 3600)).await;
            }
        });
        tracing::info!("[backtest] worker ENABLED (horizon 30d, benchmark ETH, ~3h cycle)");
    } else {
        tracing::info!("[backtest] worker OFF (set ENABLE_BACKTEST_WORKER=1 when rust is sole server)");
    }

    // conductor-finder.js — the cross-asset conductor detector. Gated: it rewrites the
    // shared conductors.json, which /api/conductors serves straight from disk.
    if std::env::var("ENABLE_CONDUCTORS_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(90)).await;
            loop {
                conductors_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
            }
        });
        tracing::info!("[conductors] worker ENABLED (~10 min cycle)");
    } else {
        tracing::info!("[conductors] worker OFF (set ENABLE_CONDUCTORS_WORKER=1 when rust is sole server)");
    }

    // conductor.js `startConductorMonitor` — watchlist wallets labelled "диригент"
    // are polled for NEW funding. Gated: it writes conductor-state.json, and running
    // it alongside Node would double-alert.
    if std::env::var("ENABLE_CONDUCTOR_MONITOR_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(50)).await;
            loop {
                conductor_monitor_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
            }
        });
        tracing::info!("[conductor] monitor ENABLED (new funding → alert, ~30 min cycle)");
    } else {
        tracing::info!("[conductor] monitor OFF (set ENABLE_CONDUCTOR_MONITOR_WORKER=1 when rust is sole server)");
    }

    // ml-smart-money.js — the self-supervised trainer. Gated: it writes sm-train.json,
    // sm-model.json and sm-forward.json.
    if std::env::var("ENABLE_ML_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                ml_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            }
        });
        // Forward labels mature slowly — once every 30 min is plenty.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(180)).await;
            loop {
                ml_mature_tick().await;
                tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
            }
        });
        tracing::info!("[ml] trainer ENABLED ({} samples on disk)", ml_worker::dataset_len());
    } else {
        tracing::info!("[ml] trainer OFF (set ENABLE_ML_WORKER=1 when rust is sole server)");
    }

    // telegram.js — signal delivery. Read-only w.r.t. the dataset, so it always runs; it
    // does nothing at all unless TELEGRAM_BOT_TOKEN is set.
    telegram::start();

    // Tracked-wallet detail pre-warm — read-only, keeps balances/score caches hot
    // so clicking a watched wallet is instant. Always on (no dataset writes).
    {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(45)).await;
            loop {
                wallet_warm_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(8 * 60)).await;
            }
        });
        tracing::info!("[wwarm] tracked-wallet detail pre-warm started (~8 min)");
    }

    // health.js `startWatchdog` — log stalled workers. Read-only, so it always runs.
    {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5 * 60)).await;
            loop {
                let h = health::health(app2.dataset.root(), now_ms());
                let stale: Vec<String> = h["workers"]
                    .as_array()
                    .map(|ws| {
                        ws.iter()
                            .filter(|w| w["exists"] == json!(true) && w["ok"] == json!(false))
                            .map(|w| format!("{}({}хв)", w["label"].as_str().unwrap_or(""), w["ageMin"]))
                            .collect()
                    })
                    .unwrap_or_default();
                if !stale.is_empty() {
                    tracing::warn!("[watchdog] застій: {}", stale.join(", "));
                }
                tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
            }
        });
        tracing::info!("[watchdog] worker monitoring started");
    }

    // backup.js — daily copy of the dataset into dataset/.backups/<day>. Gated: it
    // writes into the shared dataset tree.
    if std::env::var("ENABLE_BACKUP_WORKER").ok().as_deref() == Some("1") {
        let (ds, dd) = (dataset_dir.clone(), data_dir.clone());
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                // `new Date().toISOString().slice(0, 10)` — the UTC day.
                let day = chrono::DateTime::from_timestamp_millis(now_ms())
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                backup::backup_once(&ds, &dd, &day);
                tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
            }
        });
        tracing::info!("[backup] worker ENABLED (daily, last 7 days)");
    } else {
        tracing::info!("[backup] worker OFF (set ENABLE_BACKUP_WORKER=1 when rust is sole server)");
    }

    // market.js `startMarketWorker` — keep the 30-minute macro cache warm. Gated:
    // computing the context RECORDS dominance into history.json (that is how the
    // BTC→ETH rotation warms up), so it writes to the shared dataset.
    if std::env::var("ENABLE_MARKET_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            loop {
                let moves = app2.dataset.read_or("coordinated-moves.json", json!({"moves": []}));
                market::get_market_context(&moves, now_ms()).await;
                tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await;
            }
        });
        tracing::info!("[market] context worker ENABLED (~30min cycle)");
    } else {
        tracing::info!("[market] context worker OFF (set ENABLE_MARKET_WORKER=1 when rust is sole server)");
    }

    // labels.js — re-import the public label set every 12h (first pass after 20s if
    // the disk cache is empty). Gated: it rewrites the shared labels-cache.json.
    if std::env::var("ENABLE_LABELS_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            // Порожній АБО старий кеш — обидва випадки вимагають імпорту одразу.
            // Раніше умовою була лише порожнеча, і 33-денні мітки вважались справними:
            // воркер вмикали, а перший імпорт відкладався ще на 12 годин. Мітки живлять
            // назви сутностей усюди, тож «є, але старі» — це не робочий стан.
            let age = app2.labels.age_secs();
            if app2.labels.len() == 0 || age < 0 || age > 12 * 3600 {
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                tracing::info!("[labels] кеш віком {age} с — імпортую одразу, не чекаючи циклу");
                app2.labels.refresh().await;
            }
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(12 * 3600)).await;
                app2.labels.refresh().await;
            }
        });
        tracing::info!("[labels] import worker ENABLED (every 12h)");
    } else {
        tracing::info!("[labels] import worker OFF (set ENABLE_LABELS_WORKER=1 when rust is sole server)");
    }

    // liquidations.js — Bybit public WS. In-memory only (no dataset file), so it
    // needs no ENABLE_* gate: it can run next to Node without double-writing anything.
    {
        let tx = app.events_tx.clone();
        tokio::spawn(async move { liquidations::run(tx).await });
    }

    // whale-finder.js — $1B+ ETH-whale hunter. Gated (writes the shared dataset
    // file). ~every 2h, first after 90s.
    if std::env::var("ENABLE_WHALE_FINDER_WORKER").ok().as_deref() == Some("1") {
        let app2 = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(90)).await;
            loop {
                whale_finder_tick(&app2).await;
                tokio::time::sleep(std::time::Duration::from_secs(2 * 3600)).await;
            }
        });
        tracing::info!("[whale-finder] worker ENABLED (~2h cycle)");
    } else {
        tracing::info!("[whale-finder] worker OFF (set ENABLE_WHALE_FINDER_WORKER=1 when rust is sole server)");
    }

    // index.js:94-172 gateway: Basic auth (admin/viewer) + brute-force lockout +
    // abuse-ban + per-IP rate limit. In-memory, resets on restart, exactly like Node.
    let auth_state = Arc::new(auth::AuthState::new(auth::AuthConfig::from_env()));
    if auth_state.cfg.no_passwords() {
        tracing::info!("[auth] local mode — no passwords set (full access)");
    } else {
        tracing::info!("[auth] password protection ON (admin+viewer roles, rate-limited)");
    }

    let router = Router::new()
        .route("/api/version", get(version))
        .route("/api/health", get(health))
        .route("/api/config", get(config))
        .route("/api/clarity-tags", get(clarity_tags))
        .route("/api/wallets", get(wallets).post(add_wallet))
        .route("/api/workers-health", get(workers_health))
        .route("/api/market-context", get(market_context))
        .route("/api/liquidations", get(liquidations_route))
        .route("/api/token-ai", get(token_ai_route))
        .route("/api/graph-db-stats", get(graph_db_stats))
        .route("/api/forensics-status", get(forensics_status))
        .route("/api/telegram-status", get(telegram_status))
        .route("/api/debug-error", get(debug_error))
        .route("/api/wallets/{id}", delete(delete_wallet))
        .route("/api/events", get(events))
        .route("/api/whale-assets", get(whale_assets))
        .route("/api/sector-flow", get(sector_flow_route))
        .route("/api/sector-rotation", get(|s: State<Ctx>| passthrough(s, "sector-rotation.json", json!({"months": [], "sectors": {}}))))
        .route("/api/families", get(|s: State<Ctx>| passthrough(s, "families.json", json!({"familiesCount": 0, "families": []}))))
        .route("/api/ask", get(assistant::ask_route))
        .route("/api/wallet-family", get(wallet_family_route))
        .route("/api/signals", get(signals_route))
        .route("/api/cross-asset", get(cross_asset))
        .route("/api/transfers", get(transfers))
        .route("/api/wallet-score", get(wallet_score))
        .route("/api/conductor-network", get(conductor_network_route))
        .route("/api/coordination", get(coordination_route))
        .route("/api/concentration", get(concentration_route))
        .route("/api/exchange-netflow", get(exchange_netflow_route))
        .route("/api/group-graph", get(routes::group_graph_route))
        .route("/api/wallet-graph", get(wallet_graph_route))
        .route("/api/wallet-clusters", get(wallet_clusters_route))
        .route("/api/portfolio-strategy", get(portfolio_strategy_route))
        .route("/api/portfolio-backtest", get(portfolio_backtest_route))
        .route("/api/check", get(check::check_route))
        // Масова перевірка списком: те, чого бракує навіть промисловим розслідувальним
        // інструментам. Невідомі адреси в зведенні рахуються ОКРЕМО й ніколи не
        // додаються до «чистих».
        .route("/api/check/bulk", post(check::check_bulk_route))
        // Послужний список: чи працювали наші сигнали. Порівняння йде з випадковим
        // входом у той самий актив, а не з нулем — інакше дрейф активу читався б як едж.
        .route("/api/track-record", get(trackrecord::route))
        // Хто роздає позицію, з якої не може вийти: розмір міряється днями обороту,
        // а не доларами. Позиція з невідомим обсягом не публікується взагалі.
        .route("/api/exit-pressure", get(exitpressure::route))
        // Найпоширеніша хиба китових стрічок: біржа пересуває власні монети між
        // власними гаманцями, а це подається як рух ринку. Адреса без мітки дає
        // «не знаємо», а не «не біржа».
        .route("/api/move-check", get(cexmove::route))
        // Вік усіх наборів: показує, які панелі живляться вимкненими воркерами.
        .route("/api/data-ages", get(dataages::route))
        .route("/api/openapi.json", get(openapi_route))
        .route("/api/cg-price", get(cg_price_route))
        .route("/api/cg-search", get(cg_search_route))
        .route("/api/cg-asset", get(cg_asset_route))
        .route("/api/strategy", get(strategy_route))
        .route("/api/wallet-intel/{id}", get(wallet_intel_route))
        .route("/api/connection-path", get(connection_path_route))
        .route("/api/wallet-netflow", get(wallet_netflow_route))
        .route("/api/money-trace", get(money_trace_route))
        .route("/api/behavior-check", get(behavior_check))
        .route("/api/wallet-assets", get(wallet_assets))
        .route("/api/wallet-asset-groups", get(wallet_asset_groups_route))
        .route("/api/holder-assets", get(holder_assets_route))
        .route("/api/asset-flow", get(asset_flow_route))
        .route("/api/asset-groups", get(asset_groups_route))
        .route("/api/strategy-backtest", get(strategy_backtest_route))
        .route("/api/sm-predict", get(sm_predict))
        .route("/api/ml-status", get(ml_status_route))
        // index.js:678 — the forward track. It reports "not enough matured samples" rather
        // than a number, until there genuinely are enough.
        .route("/api/ml-forward", get(|| async { Json(ml_worker::forward_status()) }))
        .route("/api/report", get(report_route))
        .route("/api/native-balance", get(native_balance_route))
        .route("/api/evm-balances", get(evm_balances_route))
        .route("/api/group-stats", get(group_stats_route))
        .route("/api/group-flow", get(group_flow_route))
        .route("/api/group-coordination", get(group_coordination_route))
        .route("/api/wallet-2hop", get(wallet_2hop_route))
        // Готові баланси всіх гаманців (їх рахує balances_tick). Порожній дефолт — чесний:
        // «воркер ще не відпрацював», а не «у всіх нуль».
        .route("/api/wallet-balances", get(|s: State<Ctx>| passthrough(s, "wallet-balances.json", json!({"count": 0, "tracked": 0, "totalUsd": 0, "wallets": {}}))))
        // AVOID-прапорець: токени, де кити у топ-децилі накопичення по частці емісії
        // (nextaction avoid.rs -> dataset/whale-avoid.json). Історично ~66% недоперформ за 14д.
        .route("/api/whale-avoid", get(|s: State<Ctx>| passthrough(s, "whale-avoid.json", json!({"count": 0, "tokens": []}))))
        // Виробнича стратегія: market-neutral SHORT-кандидати (топ-децил USD-накопичення напрямних китів, MM виключено).
        .route("/api/whale-short", get(|s: State<Ctx>| passthrough(s, "strategy-short.json", json!({"count": 0, "tokens": []}))))
        // Волатильний сигнал: P(великий рух) для топ-децила накопичення (навчена модель, AUC 0.57).
        .route("/api/whale-volatility", get(|s: State<Ctx>| passthrough(s, "strategy-volatility.json", json!({"count": 0, "tokens": []}))))
        // Координовані групи великого капіталу (поведінкова ко-активність, без прямих лінків).
        .route("/api/wallet-groups", get(|s: State<Ctx>| passthrough(s, "wallet-groups.json", json!({"totalGroups": 0, "groups": []}))))
        .route("/api/login", post(login_route).with_state(auth_state.clone()))
        .route("/api/logout", post(logout_route).with_state(auth_state.clone()))
        .route("/api/capital-groups", get(|s: State<Ctx>| passthrough(s, "capital-groups.json", json!({"groups": []}))))
        // Активи груп капіталу з часткою емісії (від 1%), відсортовані за спаданням.
        .route("/api/group-assets", get(|s: State<Ctx>| passthrough(s, "group-assets.json", json!({"groups": []}))))
        .route("/api/asset-forensics", get(|s: State<Ctx>| passthrough(s, "asset-forensics.json", json!({"assets": []}))))
        .route("/api/whale-clusters", get(|s: State<Ctx>| passthrough(s, "whale-clusters.json", json!({"clusters": []}))))
        .route("/api/discovered-groups", get(|s: State<Ctx>| passthrough(s, "discovered-groups.json", json!({"groups": []}))))
        .route("/api/featured-groups", get(|s: State<Ctx>| passthrough(s, "featured-groups.json", json!({"groups": []}))))
        // Worker-state routes served from the dataset file the worker persists on
        // every cycle (its in-memory `result` is loaded from and written back to
        // this file — see conductor-finder.js/active-groups.js/etc.). The defaults
        // mirror each worker's initial `result` for the file-absent case. Node's
        // res.json(getX()) is byte-equal to the file (verified for conductors).
        .route("/api/conductors", get(|s: State<Ctx>| passthrough(s, "conductors.json", json!({"generated": 0, "count": 0, "conductors": []}))))
        .route("/api/active-groups", get(|s: State<Ctx>| passthrough(s, "active-groups.json", json!({"generated": 0, "status": {"running": false, "processed": 0, "total": 0}, "groups": []}))))
        .route("/api/coordinated-moves", get(|s: State<Ctx>| passthrough(s, "coordinated-moves.json", json!({"generated": 0, "moves": [], "status": {"running": false, "processed": 0, "total": 0}}))))
        // === Мережевий-аналіз whale-мета-мережі (universe-кластери + граф + інституційні мітки) ===
        .route("/api/universe-groups", get(|s: State<Ctx>| passthrough(s, "backfill/groups-universe.json", json!({"band": 0, "universe": true, "archetypes": [], "communities": []}))))
        .route("/api/cohort-analysis", get(|s: State<Ctx>| passthrough(s, "backfill/cohort-analysis.json", json!({"band": 0, "archetypes": [], "communities": []}))))
        .route("/api/desk-master-graph", get(|s: State<Ctx>| passthrough(s, "backfill/master-graph.json", json!({"nodes": [], "edges": [], "counts": {}}))))
        .route("/api/desk-linked-network", get(|s: State<Ctx>| passthrough(s, "backfill/desk-linked-network.json", json!({"desk": [], "connected_recipients": []}))))
        .route("/api/network-labels", get(|s: State<Ctx>| passthrough(s, "backfill/network-labels.json", json!({"hubs_otc": {}, "exchange_sinks": {}}))))
        .route("/api/meta-subnets", get(|s: State<Ctx>| passthrough(s, "backfill/meta-subnets.json", json!({"band": 0, "archetypes": [], "communities": []}))))
        .route("/api/whale-finder", get(|s: State<Ctx>| passthrough(s, "whale-finder.json", json!({"generated": 0, "status": {"running": false, "processed": 0, "total": 0}, "whales": []}))))
        .route("/api/wallet-asset-scan", get(|s: State<Ctx>| passthrough(s, "wallet-asset-scan.json", json!({"generated": 0, "status": {"running": false, "processed": 0, "total": 0}, "assets": []}))))
        // Порожній сигнал — це `null`, а НЕ нулі: «0% влучань» на ненарахованих даних читалося б як
        // «сигнал не працює». Форма збігається з server/backtest.js (подієвий бектест, горизонт+бенчмарк).
        .route("/api/backtest", get(|s: State<Ctx>| passthrough(s, "backtest.json", json!({"generated": 0, "status": {"running": false, "processed": 0, "total": 0}, "method": null, "signal": null, "assets": []}))))
        .route("/api/whale-positions", get(whale_positions_route))
        .route("/api/coordinated-networks", get(coordinated_networks_route))
        .route("/api/alpha-feed", get(alpha_feed_route))
        .route("/api/behavior-alerts", get(behavior_alerts_route))
        .route("/api/history", get(history_route))
        .route("/ws", get(ws_handler))
        .route("/webhook/moralis", post(webhook_moralis))
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
        .fallback_service(ServeDir::new(root.join("web")))
        .layer(middleware::from_fn_with_state(auth_state.clone(), gate_mw))
        .layer(middleware::from_fn(no_cache_static))
        .with_state(app);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("wallet-radar (rust) listening on {addr}");
    watchdog::arm();
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}


