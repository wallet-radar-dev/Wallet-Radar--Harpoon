//! Точка входу: оркестрація, конфіг, фонові цикли, коректне завершення.
//!
//! Процес складається з чотирьох незалежних циклів:
//!
//! * **поллінг Telegram** — читає оновлення, дедуп за `update_id`, роутить;
//! * **зрізи джерела** — раз на хвилину забирає агрегати й оновлює клітинку;
//! * **стрічка подій** — WS до Wallet Radar плюс страхувальний REST-поллінг;
//! * **черга розсилки** — надсилає заплановане, з ретраями й лімітами.
//!
//! Жоден цикл не блокує інший: падіння джерела не зупиняє відповіді на
//! запити, а падіння Telegram не зупиняє збір даних.

mod config;

use alert_engine::{Alert, AlertKind, Rule, Severity, Window};
use anyhow::Context;
use broadcast::{BroadcastConfig, Broadcaster};
use config::Config;
use privacy_filter::PrivacyFilter;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage::{SqliteStore, Store};
use telegram_gateway::{
    Router, RouterConfig, SnapshotCell, TeloxideNotifier, TeloxidePoller, UpdateSource,
};
use teloxide::Bot;
use user_session::UserSession;
use wr_client::{ServerMsg, WrClient};

/// Стан джерела — для деградації та діагностики.
struct Health {
    last_snapshot_ms: AtomicI64,
    last_event_ms: AtomicI64,
    ws_connected: AtomicBool,
    degraded: AtomicBool,
    gap_detected: AtomicI64,
}

impl Health {
    fn new() -> Health {
        Health {
            last_snapshot_ms: AtomicI64::new(0),
            last_event_ms: AtomicI64::new(0),
            ws_connected: AtomicBool::new(false),
            degraded: AtomicBool::new(false),
            gap_detected: AtomicI64::new(0),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let config_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "config.toml".to_string());

    let cfg = Config::load(Path::new(&config_path))
        .with_context(|| format!("читаю конфіг {config_path}"))?;

    init_tracing(&cfg.bot.log_level);

    // ─── Секрети: тільки з оточення ───────────────────────────────
    let token = cfg
        .env(&cfg.bot.token_env)
        .with_context(|| format!("читаю токен Telegram із змінної {}", cfg.bot.token_env))?;
    let wr_password = cfg.env(&cfg.wr.password_env).unwrap_or_default();

    // ─── Сховище ────────────────────────────────────────────────────
    if let Some(dir) = Path::new(&cfg.storage.sqlite_path).parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("створюю каталог {}", dir.display()))?;
    }
    // Секрет підписування chat_ref: у аудит і логи йде HMAC, а не сам chat_id.
    let ref_secret = cfg.env(&cfg.bot.ref_secret_env).unwrap_or_default();
    if ref_secret.is_empty() {
        tracing::warn!(
            env = %cfg.bot.ref_secret_env,
            "секрет підписування chat_ref не задано — у аудиті буде простий хеш"
        );
    }
    if !cfg.storage.backend.eq_ignore_ascii_case("sqlite") {
        anyhow::bail!(
            "поки підтримується лише сховище sqlite, а в конфізі: {}",
            cfg.storage.backend
        );
    }
    let store = SqliteStore::open_with_secret(
        &cfg.storage.sqlite_path,
        ref_secret.as_bytes(),
        cfg.storage.busy_timeout_ms,
    )
    .with_context(|| format!("відкриваю БД {}", cfg.storage.sqlite_path))?;

    // ─── Фільтр ────────────────────────────────────────────────────
    let sanitizer = PrivacyFilter::with_options(
        cfg.privacy.mask.clone(),
        cfg.privacy.redact_ens,
        cfg.privacy.redact_base58,
        cfg.privacy.redact_explorer_urls,
        &cfg.privacy.extra_blocked_domains,
    );

    // ─── Telegram ──────────────────────────────────────────────────
    let mut bot = Bot::new(token.clone());
    if let Some(api_url) = &cfg.telegram.api_url {
        if !api_url.is_empty() {
            let url = reqwest::Url::parse(api_url)
                .with_context(|| format!("розбираю telegram.api_url {api_url}"))?;
            bot = bot.set_api_url(url);
            tracing::warn!(
                api_url,
                "Telegram API перевизначено — це мок або тестове оточення"
            );
        }
    }
    let notifier = TeloxideNotifier::new(bot.clone());
    let poller = TeloxidePoller::new(bot, cfg.bot.poll_timeout_secs as u32);

    // ─── Джерело ───────────────────────────────────────────────────
    let wr = WrClient::with_timeout(
        cfg.wr.base_url.clone(),
        cfg.wr.username.clone(),
        wr_password,
        cfg.wr.request_timeout_secs,
    );

    // ─── Двигун алертів ────────────────────────────────────────────
    let rules = build_rules(&cfg);
    let engine = Arc::new(Mutex::new(alert_engine::Engine::new(rules)));
    let snapshot_cell = SnapshotCell::new();
    let health = Arc::new(Health::new());

    // ─── Розсилка ──────────────────────────────────────────────────
    let bcfg = BroadcastConfig {
        global_per_sec: cfg.rate_limits.telegram_global_per_sec,
        per_chat_min_interval_ms: cfg.rate_limits.telegram_per_chat_min_interval_ms,
        max_pending_per_chat: cfg.rate_limits.max_pending_per_chat,
        ..BroadcastConfig::default()
    };
    let broadcaster = Arc::new(Broadcaster::new(
        store.clone(),
        notifier,
        sanitizer.clone(),
        bcfg,
    ));

    // ─── Сесії, адмінка, роутер ────────────────────────────────────
    let session = UserSession::new(store.clone());
    let admins = cfg.admin.usernames.clone();
    let admin = admin::Admin::new(
        store.clone(),
        broadcaster.clone(),
        sanitizer.clone(),
        admins.clone(),
    );
    let router = Arc::new(Router::new(
        session,
        admin,
        snapshot_cell.clone(),
        RouterConfig {
            default_language: cfg.bot.default_language.clone(),
            default_timezone: cfg.bot.default_timezone.clone(),
            rate_limits: (
                cfg.rate_limits.per_user_per_min,
                cfg.rate_limits.admin_per_min,
                cfg.rate_limits.global_inbound_per_min,
            ),
            access_code: match cfg.admin.access_code_env.is_empty() {
                true => None,
                false => cfg.env(&cfg.admin.access_code_env).ok(),
            },
            admins,
        },
    ));

    // ─── Стан процесу ──────────────────────────────────────────────
    let stop = Arc::new(AtomicBool::new(false));

    // Чесно кажемо, які ручки ще не задіяні: краще попередження в лозі, ніж
    // тихий параметр, який нічого не робить.
    if cfg.features.assistant {
        tracing::warn!("features.assistant увімкнено, але асистент не реалізований — ігнорую");
    }
    if cfg.broadcast.digest_threshold_pct > 0 {
        tracing::warn!(
            threshold = cfg.broadcast.digest_threshold_pct,
            window_secs = cfg.broadcast.digest_window_secs,
            backlog_minutes = cfg.broadcast.max_backlog_minutes,
            "згортання дублів у дайджест ще не реалізовано — алерти йдуть як є"
        );
    }

    tracing::info!(
        name = %cfg.bot.name,
        wr = %cfg.wr.base_url,
        db = %cfg.storage.sqlite_path,
        "бот запускається"
    );

    // Перевірка, що джерело взагалі живе.
    match wr.snapshot().await {
        Ok(s) => {
            snapshot_cell.set(s.clone());
            health.last_snapshot_ms.store(now_ms(), Ordering::Relaxed);
            tracing::info!(
                assets = s.whale_assets.len(),
                groups = s.groups.len(),
                signals = s.signals.len(),
                "початковий зріз джерела отримано"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "початковий зріз не вдався — працюємо в degraded");
            health.degraded.store(true, Ordering::Relaxed);
        }
    }

    let mut tasks = Vec::new();

    // 1. Поллінг Telegram.
    {
        let store = store.clone();
        let router = router.clone();
        let broadcaster = broadcaster.clone();
        let stop = stop.clone();
        tasks.push(tokio::spawn(async move {
            telegram_loop(
                store,
                router,
                broadcaster,
                poller,
                stop,
                // Тут — у мілісекундах: порівнюємо з «тиша Telegram», а не з
                // тривалістю поллінгу.
                cfg.bot.heartbeat_timeout_secs.max(30) * 1000,
            )
            .await
        }));
    }

    // 2. Зрізи джерела.
    {
        let wr = wr.clone();
        let engine = engine.clone();
        let broadcaster = broadcaster.clone();
        let snapshot_cell = snapshot_cell.clone();
        let health = health.clone();
        let store = store.clone();
        let stop = stop.clone();
        let interval = Duration::from_secs(cfg.wr.poll_interval_secs.max(5));
        tasks.push(tokio::spawn(async move {
            snapshot_loop(
                wr,
                engine,
                broadcaster,
                snapshot_cell,
                health,
                store,
                interval,
                stop,
            )
            .await
        }));
    }

    // 3. Стрічка подій (WS) + страхувальний REST-поллінг.
    {
        let wr = wr.clone();
        let engine = engine.clone();
        let broadcaster = broadcaster.clone();
        let store = store.clone();
        let health = health.clone();
        let stop = stop.clone();
        let ws_path = cfg.wr.ws_path.clone();
        let poll_interval = Duration::from_secs(cfg.wr.event_poll_interval_secs.max(5));
        tasks.push(tokio::spawn(async move {
            event_loop(
                wr,
                engine,
                broadcaster,
                store,
                health,
                ws_path,
                poll_interval,
                stop,
            )
            .await
        }));
    }

    // 4. Черга розсилки.
    {
        let broadcaster = broadcaster.clone();
        let stop = stop.clone();
        tasks.push(tokio::spawn(async move {
            broadcaster.run_worker(stop).await;
        }));
    }

    // Зупинка за SIGTERM/SIGINT.
    {
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut sigterm =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(error = %e, "не вдалося підписатися на SIGTERM");
                        return;
                    }
                };
            tokio::select! {
                _ = sigterm.recv() => tracing::info!("отримано SIGTERM — завершуюся"),
                _ = tokio::signal::ctrl_c() => tracing::info!("отримано Ctrl-C — завершуюся"),
            }
            stop.store(true, Ordering::Relaxed);
        });
    }

    // Чекаємо завершення циклів.
    for t in tasks {
        let _ = t.await;
    }
    tracing::info!("бот зупинено");
    Ok(())
}

/// Розсильник, який ми використовуємо.
type Notifier_ = TeloxideNotifier;
/// Повний тип роутера — щоб не повторювати його в підписах.
type AppRouter = Router<
    UserSession<SqliteStore>,
    admin::Admin<SqliteStore, Arc<Broadcaster<SqliteStore, Notifier_>>>,
>;

/// Цикл поллінгу Telegram.
async fn telegram_loop(
    store: SqliteStore,
    router: Arc<AppRouter>,
    broadcaster: Arc<Broadcaster<SqliteStore, TeloxideNotifier>>,
    poller: TeloxidePoller,
    stop: Arc<AtomicBool>,
    cfg_heartbeat_ms: u64,
) {
    let mut last_update_ms = now_ms();
    let mut last_warn_ms = now_ms();
    let mut offset: i64 = store
        .cursor("telegram_offset")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut backoff = Duration::from_secs(1);

    while !stop.load(Ordering::Relaxed) {
        match poller.poll(offset).await {
            Ok(updates) => {
                backoff = Duration::from_secs(1);
                if updates.is_empty() {
                    let silent = now_ms() - last_update_ms;
                    // Попереджаємо не частіше ніж раз на хвилину — інакше
                    // «тиша Telegram» перетвориться на спам у лозі.
                    if silent > cfg_heartbeat_ms as i64 && now_ms() - last_warn_ms > 60_000 {
                        last_warn_ms = now_ms();
                        tracing::warn!(
                            silent_ms = silent,
                            "Telegram не надсилає оновлень довше, ніж heartbeat-таймаут"
                        );
                    }
                } else {
                    last_update_ms = now_ms();
                }
                for upd in updates {
                    offset = offset.max(upd.update_id + 1);
                    // Повторний update не виконуємо двічі.
                    match store.seen_update(upd.update_id).await {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!(error = %e, "не вдалося зафіксувати update");
                        }
                    }
                    if let Some(reply) = router.handle(&upd).await {
                        match reply.reply_to {
                            Some(parent) => {
                                let _ = broadcaster
                                    .reply_to(reply.chat_id, &reply.text, parent)
                                    .await;
                            }
                            None => {
                                let _ = broadcaster.reply(reply.chat_id, &reply.text).await;
                            }
                        }
                    }
                }
                if let Err(e) = store
                    .set_cursor("telegram_offset", &offset.to_string())
                    .await
                {
                    tracing::warn!(error = %e, "не вдалося зберегти offset");
                }
            }
            Err(telegram_gateway::PollerError::Conflict) => {
                tracing::error!(
                    "Telegram відповів 409: на цьому токені вже працює інший полер. \
                     Другий бот на тому самому токені запускати не можна — зупиняюсь."
                );
                stop.store(true, Ordering::Relaxed);
                return;
            }
            Err(telegram_gateway::PollerError::InvalidToken(msg)) => {
                tracing::error!(%msg, "некоректний токен Telegram — зупиняюсь");
                stop.store(true, Ordering::Relaxed);
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "помилка поллінгу Telegram — повторю");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

/// Цикл зрізів джерела.
#[allow(clippy::too_many_arguments)]
async fn snapshot_loop(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<SqliteStore, TeloxideNotifier>>,
    snapshot_cell: Arc<SnapshotCell>,
    health: Arc<Health>,
    store: SqliteStore,
    interval: Duration,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match wr.snapshot().await {
            Ok(snap) => {
                health.last_snapshot_ms.store(now_ms(), Ordering::Relaxed);
                snapshot_cell.set(snap.clone());
                if let Ok(payload) = serde_json::to_string(&snap) {
                    let _ = store.put_snapshot(snap.as_of, &payload).await;
                }
                let alerts = {
                    let mut e = engine.lock().unwrap_or_else(|err| err.into_inner());
                    e.apply_snapshot(&snap)
                };
                publish_all(&broadcaster, &alerts).await;
                health.degraded.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!(error = %e, "зріз джерела недоступний");
                health.degraded.store(true, Ordering::Relaxed);
                let mut e = engine.lock().unwrap_or_else(|err| err.into_inner());
                e.set_degraded(true);
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Цикл стрічки подій: WS + страхувальний REST-поллінг.
#[allow(clippy::too_many_arguments)]
async fn event_loop(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<SqliteStore, TeloxideNotifier>>,
    store: SqliteStore,
    health: Arc<Health>,
    ws_path: String,
    poll_interval: Duration,
    stop: Arc<AtomicBool>,
) {
    // Водночас: WS-споживач і REST-страховка. Дублікати не страшні — дедуп за
    // `seen_events`.
    let ws_handle = {
        let wr = wr.clone();
        let engine = engine.clone();
        let broadcaster = broadcaster.clone();
        let store = store.clone();
        let health = health.clone();
        let stop = stop.clone();
        tokio::spawn(
            async move { ws_loop(wr, engine, broadcaster, store, health, ws_path, stop).await },
        )
    };

    let poll_handle = {
        let wr = wr.clone();
        let engine = engine.clone();
        let broadcaster = broadcaster.clone();
        let store = store.clone();
        let health = health.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            rest_poll_loop(wr, engine, broadcaster, store, health, poll_interval, stop).await
        })
    };

    let _ = ws_handle.await;
    let _ = poll_handle.await;
}

/// WS-споживач.
#[allow(clippy::too_many_arguments)]
async fn ws_loop(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<SqliteStore, TeloxideNotifier>>,
    store: SqliteStore,
    health: Arc<Health>,
    ws_path: String,
    stop: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_secs(1);
    while !stop.load(Ordering::Relaxed) {
        match wr.connect_ws(&ws_path).await {
            Ok(mut stream) => {
                tracing::info!("стрічка подій підключена");
                health.ws_connected.store(true, Ordering::Relaxed);
                backoff = Duration::from_secs(1);

                while !stop.load(Ordering::Relaxed) {
                    match stream.next_message().await {
                        Some(Ok(ServerMsg::Init { events })) => {
                            tracing::info!(count = events.len(), "init-кадр стрічки");
                            ingest(&store, &engine, &broadcaster, &health, events).await;
                        }
                        Some(Ok(ServerMsg::Event(ev))) => {
                            ingest(&store, &engine, &broadcaster, &health, vec![ev]).await;
                        }
                        Some(Ok(ServerMsg::Other)) => {}
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "помилка стрічки");
                            break;
                        }
                        None => {
                            tracing::info!("стрічка закрилася");
                            break;
                        }
                    }
                }
                health.ws_connected.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!(error = %e, "не вдалося підключитися до стрічки");
                health.degraded.store(true, Ordering::Relaxed);
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

/// REST-страховка: відновлення подій після розриву й заповнення дірок.
async fn rest_poll_loop(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<SqliteStore, TeloxideNotifier>>,
    store: SqliteStore,
    health: Arc<Health>,
    interval: Duration,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(interval).await;
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match wr.events(500).await {
            Ok(events) => {
                let watermark: i64 = store
                    .cursor("event_watermark_ms")
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                // Дірка: найстаріша подія, яку джерело ще памʼятає, новіша за
                // наш watermark — значить, частину подій ми втратили назавжди.
                if let Some(oldest) = events.iter().map(|e| e.ts).min() {
                    if watermark > 0 && oldest > watermark + 1000 {
                        health.gap_detected.store(oldest, Ordering::Relaxed);
                        tracing::warn!(
                            watermark,
                            oldest,
                            "дірка в стріці: частину подій втрачено — алерти позначатимемо як неповні"
                        );
                    }
                }

                let fresh: Vec<_> = events.into_iter().filter(|e| e.ts > watermark).collect();
                if !fresh.is_empty() {
                    ingest(&store, &engine, &broadcaster, &health, fresh).await;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "REST-поллінг подій не вдався");
                health.degraded.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Прийняти події, відфільтрувати дублікати, переоцінити вікна, розіслати.
async fn ingest(
    store: &SqliteStore,
    engine: &Arc<Mutex<alert_engine::Engine>>,
    broadcaster: &Arc<Broadcaster<SqliteStore, TeloxideNotifier>>,
    health: &Arc<Health>,
    events: Vec<wr_client::RawEvent>,
) {
    let now = now_ms();
    let mut fresh = Vec::new();
    let mut max_ts = 0i64;

    for ev in events {
        if ev.ts > max_ts {
            max_ts = ev.ts;
        }
        match store.seen_event(&ev.id, ev.ts).await {
            Ok(true) => continue,
            Ok(false) => fresh.push(ev),
            Err(e) => {
                tracing::warn!(error = %e, "не вдалося перевірити дубль події");
            }
        }
    }

    if fresh.is_empty() {
        return;
    }
    health.last_event_ms.store(now, Ordering::Relaxed);

    let alerts = {
        let mut e = engine.lock().unwrap_or_else(|err| err.into_inner());
        for ev in &fresh {
            e.ingest_event(ev);
        }
        e.evaluate(now)
    };
    publish_all(broadcaster, &alerts).await;

    let _ = store
        .set_cursor("event_watermark_ms", &max_ts.to_string())
        .await;
}

/// Опублікувати алерти.
async fn publish_all(
    broadcaster: &Arc<Broadcaster<SqliteStore, TeloxideNotifier>>,
    alerts: &[Alert],
) {
    for alert in alerts {
        match broadcaster.publish(alert).await {
            Ok(broadcast::PublishOutcome::Queued { recipients }) => {
                tracing::info!(
                    kind = alert.kind.as_str(),
                    asset = %alert.asset.symbol,
                    recipients,
                    "алерт поставлено в чергу"
                );
            }
            Ok(broadcast::PublishOutcome::AlreadyKnown) => {
                tracing::debug!(
                    kind = alert.kind.as_str(),
                    "алерт уже відомий — дубль відкинуто"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "не вдалося поставити алерт у чергу");
            }
        }
    }
}

/// Правила алертів із конфіга; якщо правил немає — дефолтні.
fn build_rules(cfg: &Config) -> Vec<Rule> {
    if cfg.alerts.rules.is_empty() {
        return vec![
            Rule {
                kind: AlertKind::WhaleAccumulation,
                min_participants: 3,
                min_total_value_usd: 20_000.0,
                windows: vec![Window::W15m, Window::W1h],
                assets: vec![],
                severity: Severity::Medium,
            },
            Rule {
                kind: AlertKind::WhaleDistribution,
                min_participants: 3,
                min_total_value_usd: 20_000.0,
                windows: vec![Window::W1h],
                assets: vec![],
                severity: Severity::Medium,
            },
            Rule {
                kind: AlertKind::LiquidationSpike,
                min_participants: 1,
                min_total_value_usd: 250_000.0,
                windows: vec![Window::W1h],
                assets: vec![],
                severity: Severity::High,
            },
            Rule {
                kind: AlertKind::CrossAssetAnomaly,
                min_participants: 10,
                min_total_value_usd: 0.0,
                windows: vec![Window::W1h],
                assets: vec![],
                severity: Severity::Info,
            },
        ];
    }
    cfg.alerts
        .rules
        .iter()
        .filter_map(|r| {
            let kind = match r.kind.as_str() {
                "whale_accumulation" => AlertKind::WhaleAccumulation,
                "whale_distribution" => AlertKind::WhaleDistribution,
                "pump_phase" => AlertKind::PumpPhase,
                "liquidation_spike" => AlertKind::LiquidationSpike,
                "capital_group_change" => AlertKind::CapitalGroupChange,
                "cross_asset_anomaly" => AlertKind::CrossAssetAnomaly,
                other => {
                    tracing::warn!(kind = other, "невідомий вид правила — пропускаю");
                    return None;
                }
            };
            let windows = r
                .windows
                .iter()
                .filter_map(|w| Window::parse(w))
                .collect::<Vec<_>>();
            Some(Rule {
                kind,
                min_participants: r.min_participants,
                min_total_value_usd: r.min_total_value_usd,
                windows,
                assets: r.assets.clone(),
                severity: Severity::parse(&r.severity),
            })
        })
        .collect()
}

fn init_tracing(level: &str) {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| level.to_string());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_target(false)
        .try_init();
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
