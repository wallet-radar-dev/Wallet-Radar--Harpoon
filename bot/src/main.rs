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

use alert_engine::{Alert, AlertKind, Rule, Severity, Window};
use anyhow::Context;
use bot::config::{Config, PollConflictPolicy};
use bot::registry;
use broadcast::{BroadcastConfig, Broadcaster};
use privacy_filter::PrivacyFilter;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage::{SqliteStore, Store};
#[cfg(feature = "postgres")]
use storage::PostgresStore;
use telegram_gateway::{
    PollHealth, PollState, Router, RouterConfig, SnapshotCell, TeloxideNotifier, TeloxidePoller,
    UpdateSource,
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

/// Стан доступності джерела: поки воно недоступне, черга розсилки
/// **буферизується** (див. `docs/ARCHITECTURE.md` §8.3).
///
/// Логіка — з того самого розділу: `degraded` настає, коли REST джерела
/// недоступний понад 2 хвилини; далі черга тримає алерти, а якщо джерело не
/// ожило за межу буфера (`max_backlog_minutes`) — накопичене скидається.
struct SourceState {
    /// `false` — джерело недоступне, черга буферизується.
    gate: Arc<AtomicBool>,
    /// Коли джерело стало недоступним (epoch-мс); `0` — джерело доступне.
    down_since_ms: AtomicI64,
}

/// Скільки секунд джерело має бути недоступним, щоб це вважалося зббоєм,
/// а не коротким hiccup (§8.3: «REST недоступний > 2 хв»).
const SOURCE_DEGRADED_AFTER_SECS: i64 = 120;

impl SourceState {
    fn new(gate: Arc<AtomicBool>) -> SourceState {
        SourceState {
            gate,
            down_since_ms: AtomicI64::new(0),
        }
    }

    /// Джерело відповіло: буфер розблоковано.
    fn recovered(&self) {
        let was_down = self.down_since_ms.swap(0, Ordering::Relaxed);
        self.gate.store(true, Ordering::Relaxed);
        if was_down > 0 {
            tracing::info!(
                down_ms = now_ms() - was_down,
                "джерело ожило — буфер розсилки випускаємо"
            );
        }
    }

    /// Зріз не вдався. Повертає `true`, якщо джерело **щойно** перейшло у
    /// «недоступне» — тоді про перехід варто залогувати один раз.
    fn failed(&self, now: i64) -> bool {
        let since = self.down_since_ms.load(Ordering::Relaxed);
        let since = if since == 0 {
            self.down_since_ms.store(now, Ordering::Relaxed);
            now
        } else {
            since
        };
        // `swap` повертає попереднє значення: `true` → ми саме зараз
        // перемикали гейт, тож перехід треба залогувати.
        if now - since >= SOURCE_DEGRADED_AFTER_SECS * 1000
            && self.gate.swap(false, Ordering::SeqCst)
        {
            return true;
        }
        false
    }
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
    // Секрет підписування chat_ref: у аудит і логи йде HMAC, а не сам chat_id.
    let ref_secret = cfg.env(&cfg.bot.ref_secret_env).unwrap_or_default();
    if ref_secret.is_empty() {
        tracing::warn!(
            env = %cfg.bot.ref_secret_env,
            "секрет підписування chat_ref не задано — у аудиті буде простий хеш"
        );
    }

    // ─── Сховище ────────────────────────────────────────────────────
    //
    // Бекенд обирається конфігом (`storage.backend`): `sqlite` або `postgres`.
    // Код один і той самий для обох — різняються лише реалізація трейта
    // `Store`, яка мономорфізується під час компіляції.
    match cfg.storage.backend.trim().to_ascii_lowercase().as_str() {
        "sqlite" => {
            if let Some(dir) = Path::new(&cfg.storage.sqlite_path).parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("створюю каталог {}", dir.display()))?;
            }
            let store = SqliteStore::open_with_secret(
                &cfg.storage.sqlite_path,
                ref_secret.as_bytes(),
                cfg.storage.busy_timeout_ms,
            )
            .with_context(|| format!("відкриваю БД {}", cfg.storage.sqlite_path))?
            .with_lease(lease_ms(&cfg.storage));
            run(cfg, store).await
        }
        #[cfg(feature = "postgres")]
        "postgres" => {
            let url = cfg
                .env(&cfg.storage.postgres_url_env)
                .with_context(|| {
                    format!(
                        "читаю URL бази Postgres із змінної {}",
                        cfg.storage.postgres_url_env
                    )
                })?;
            let store = PostgresStore::connect_with(
                &url,
                ref_secret.as_bytes(),
                cfg.storage.postgres_max_connections,
                cfg.storage.postgres_connect_timeout_secs,
            )
            .await
            .with_context(|| format!("підключаюся до Postgres {}", redact_url(&url)))?
            .with_lease(lease_ms(&cfg.storage));
            run(cfg, store).await
        }
        other => {
            anyhow::bail!(
                "невідомий бекенд сховища: {other} (очікується sqlite або postgres)"
            )
        }
    }
}

/// Основний цикл бота — одна й та сама логіка для будь-якого бекенда сховища.
///
/// `S` — конкретна реалізація [`Store`]: SQLite або Postgres.
async fn run<S>(cfg: Config, store: S) -> anyhow::Result<()>
where
    S: Store + Clone + Send + Sync + 'static,
{
    // ─── Секрети: тільки з оточення ───────────────────────────────
    let token = cfg
        .env(&cfg.bot.token_env)
        .with_context(|| format!("читаю токен Telegram із змінної {}", cfg.bot.token_env))?;
    let wr_password = cfg.env(&cfg.wr.password_env).unwrap_or_default();

    // ─── Фільтр ────────────────────────────────────────────────────
    let sanitizer = PrivacyFilter::with_options(
        cfg.privacy.mask.clone(),
        cfg.privacy.redact_ens,
        cfg.privacy.redact_base58,
        cfg.privacy.redact_explorer_urls,
        &cfg.privacy.extra_blocked_domains,
    );

    // ─── Telegram ──────────────────────────────────────────────────
    //
    // Клієнт teloxide за замовчуванням має таймаут 17 секунд, а довге
    // опитування Telegram триває `poll_timeout_secs`. Якщо не задати свій
    // клієнт, запит обриватиметься на кожному кроці — і бот не отримає
    // ЖОДНОГО оновлення (просто тихо не працюватиме).
    let http_timeout = Duration::from_secs(cfg.bot.poll_timeout_secs.saturating_add(20));
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(http_timeout)
        .tcp_nodelay(true)
        .build()
        .context("не вдалося зібрати HTTP-клієнт Telegram")?;
    tracing::info!(
        poll_timeout_secs = cfg.bot.poll_timeout_secs,
        http_timeout_secs = http_timeout.as_secs(),
        "довге опитування Telegram"
    );
    let mut bot = Bot::with_client(token.clone(), http);
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
    // Стан опитування Telegram: показується в `/stats` і дозволяє
    // повідомити адмінів, коли команди не обробляються через конфлікт
    // поллерів (у цей час `/stats` недоступний).
    let poll_health = Arc::new(PollHealth::new());

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
        // Згортання «широких» сигналів у зведений дайджест.
        digest_threshold_pct: cfg.broadcast.digest_threshold_pct,
        digest_window_secs: cfg.broadcast.digest_window_secs,
        // Межа буфера черги: скільки хвилин доставка може чекати.
        max_backlog_minutes: cfg.broadcast.max_backlog_minutes,
        ..BroadcastConfig::default()
    };
    let broadcaster = Arc::new(Broadcaster::new(
        store.clone(),
        notifier,
        sanitizer.clone(),
        bcfg,
    ));
    // Стан джерела: цикл зрізів керує ним, воркер розсилки — читає.
    let source_state = Arc::new(SourceState::new(broadcaster.source_gate()));

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
            poll_health: Some(poll_health.clone()),
        },
    ));

    // ─── Стан процесу ──────────────────────────────────────────────
    let stop = Arc::new(AtomicBool::new(false));
    // Помилка поллера Telegram: якщо поллер упав (409, некоректний токен),
    // процес має завершитися ненульовим кодом — інакше systemd не підніме
    // його знову, і бот лишиться мертвим.
    let telegram_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // Канал зупинки для поллера: довге опитування треба переривати, а не
    // чекати на нього до 25 секунд (див. обробку SIGTERM нижче).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_poll = shutdown_rx.clone();
    let shutdown_events = shutdown_rx.clone();
    let shutdown_snapshot = shutdown_rx.clone();
    let shutdown_registry = shutdown_rx.clone();
    let shutdown_rest = shutdown_rx.clone();

    // Чесно кажемо, які ручки ще не задіяні: краще попередження в лозі, ніж
    // тихий параметр, який нічого не робить.
    if cfg.features.assistant {
        tracing::warn!("features.assistant увімкнено, але асистент не реалізований — ігнорую");
    }
    if cfg.broadcast.digest_threshold_pct > 0 {
        tracing::info!(
            threshold = cfg.broadcast.digest_threshold_pct,
            window_secs = cfg.broadcast.digest_window_secs,
            "згортання широких сигналів у зведений дайджест увімкнено"
        );
    } else {
        tracing::info!("згортання сигналів у зведений дайджест вимкнено");
    }
    if cfg.broadcast.max_backlog_minutes > 0 {
        tracing::info!(
            max_backlog_minutes = cfg.broadcast.max_backlog_minutes,
            "черга розсилки буферизується, поки джерело недоступне; старіші за межу доставки скидаються"
        );
    } else {
        tracing::info!("межа буфера вимкнена — черга розсилки безстрокова");
    }

    tracing::info!(
        name = %cfg.bot.name,
        wr = %cfg.wr.base_url,
        backend = %cfg.storage.backend,
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
            source_state.failed(now_ms());
        }
    }

    let mut tasks = Vec::new();

    // 1. Поллінг Telegram.
    {
        let store = store.clone();
        let router = router.clone();
        let broadcaster = broadcaster.clone();
        let stop = stop.clone();
        let telegram_error = telegram_error.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = telegram_loop(
                store,
                router,
                broadcaster,
                poller,
                stop,
                shutdown_poll,
                // Тут — у мілісекундах: порівнюємо з «тиша Telegram», а не з
                // тривалістю поллінгу.
                PollCfg {
                    heartbeat_ms: cfg.bot.heartbeat_timeout_secs.max(30) * 1000,
                    conflict_policy: cfg.bot.poll_conflict_policy,
                    conflict_retry_secs: cfg.bot.poll_conflict_retry_secs.max(10),
                    preempt_secs: cfg.bot.poll_conflict_preempt_secs.max(1),
                    health: poll_health.clone(),
                },
            )
            .await
            {
                // Помилка зупинки Telegram (409 або некоректний токен) —
                // фіксуємо її для всього процесу: код виходу ненульовий,
                // systemd спробує підняти бота ще раз.
                telegram_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .replace(e.to_string());
            }
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
        let source = source_state.clone();
        let interval = Duration::from_secs(cfg.wr.poll_interval_secs.max(5));
        tasks.push(tokio::spawn(async move {
            snapshot_loop(
                wr,
                engine,
                broadcaster,
                snapshot_cell,
                health,
                store,
                source,
                interval,
                stop,
                shutdown_snapshot,
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
                shutdown_events,
                shutdown_rest,
            )
            .await
        }));
    }

    // 4. Синхронізація реєстру підключених із сервером Wallet Radar:
    //    сервер розсилає свої власні алерти саме цим списком.
    //    Тестові інстанси її вимикають — див. `registry_sync_enabled`.
    {
        let sync_token = cfg.env(&cfg.wr.sync_token_env).unwrap_or_default();
        if !cfg.wr.registry_sync_enabled {
            tracing::info!("синхронізація реєстру вимкнена конфігурацією");
        } else if sync_token.is_empty() {
            tracing::warn!(
                env = %cfg.wr.sync_token_env,
                "токен синхронізації реєстру не задано — серверні алерти йтимуть за старим списком"
            );
        } else {
            // Основний сервер + додаткові екземпляри Wallet Radar (якщо задані).
            //
            // Кожен із них — самостійний сервер зі своїм реєстром і своїм
            // Telegram-модулем. Реєстр у всіх однаковий, тож синхронізуємо
            // його в кожен: інакше підписки розходять, а серверні алерти
            // йдуть за старим списком.
            let mut targets = vec![cfg.wr.base_url.clone()];
            for extra in &cfg.wr.registry_sync_extra_urls {
                let trimmed = extra.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if !targets.iter().any(|t| t == trimmed) {
                    targets.push(trimmed.to_string());
                }
            }

            for url in targets {
                let sync = Arc::new(registry::RegistrySync::new(
                    store.clone(),
                    &url,
                    &sync_token,
                ));
                // Перша синхронізація — відразу, щоб сервер одразу бачив актуальний список.
                match sync.sync_once().await {
                    Ok(n) => {
                        tracing::info!(server = %url, users = n, "реєстр підключених синхронізовано із сервером")
                    }
                    Err(e) => {
                        tracing::warn!(server = %url, error = %e, "перша синхронізація реєстру не вдалась")
                    }
                }
                let stop = stop.clone();
                let interval = Duration::from_secs(cfg.wr.registry_sync_interval_secs.max(15));
                let shutdown_registry = shutdown_registry.clone();
                tasks.push(tokio::spawn(async move {
                    sync.run(interval, stop, shutdown_registry).await;
                }));
            }
        }
    }

    // 5. Черга розсилки.
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
        let shutdown = shutdown_tx.clone();
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
            // Сигнал для поллера: довге опитування треба **перервати**,
            // інакше процес чекатиме на нього до 25 секунд, systemd
            // вб'є його — і «мертве» getUpdates-з'єднання дасть наступному
            // інстансу 409 Conflict.
            let _ = shutdown.send(true);
        });
    }

    // Чекаємо завершення циклів.
    for t in tasks {
        let _ = t.await;
    }
    // Поллер упав? Тоді виходимо з помилкою: ненульовий код змусить
    // systemd підняти бота знову (обмежена кількість спроб).
    let poller_error = {
        let guard = telegram_error.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    if let Some(msg) = poller_error {
        tracing::info!("бот зупинено");
        return Err(anyhow::anyhow!("поллер Telegram зупинено: {msg}"));
    }
    tracing::info!("бот зупинено");
    Ok(())
}

/// Розсильник, який ми використовуємо.
type Notifier_ = TeloxideNotifier;
/// Повний тип роутера — щоб не повторювати його в підписах.
type AppRouter<S> = Router<
    UserSession<S>,
    admin::Admin<S, Arc<Broadcaster<S, Notifier_>>>,
>;

/// Налаштування циклу поллінгу Telegram.
struct PollCfg {
    /// Поріг «тиші Telegram» (epoch-мс), після якого пишемо попередження.
    heartbeat_ms: u64,
    /// Політика поведінки при конфлікті поллерів на токені.
    conflict_policy: PollConflictPolicy,
    /// Пауза між повторами опитування після 409.
    conflict_retry_secs: u64,
    /// Пауза перед повтором опитування в політиці `preempt`.
    preempt_secs: u64,
    /// Стан опитування — спільний із роутером (показ у `/stats`).
    health: Arc<PollHealth>,
}

/// Мінімальна тривалість конфлікту, з якої про нього варто повідомляти
/// адмінам: короткий збій — це нормальна річ, довгий — проблема.
const CONFLICT_NOTICE_MIN_MS: i64 = 120_000;
/// Не частіше ніж раз на 15 хвилин — конфлікт може «миготіти», і адмін
/// не має отримувати по десять повідомлень за хвилину.
const POLL_NOTICE_COOLDOWN_MS: i64 = 15 * 60_000;

/// Повідомити адмінів про зміну стану опитування.
///
/// Під час конфлікту поллерів сам `/stats` не надійде — сказати про проблему
/// можна лише окремим повідомленням.
async fn notify_poll_state<S: Store + Clone + Send + Sync + 'static>(
    store: &S,
    broadcaster: &Arc<Broadcaster<S, TeloxideNotifier>>,
    state: PollState,
) {
    let text = match state {
        PollState::Conflict => {
            "⚠️ Telegram: на токені вже працює інший поллер (409).\n\
             Команди бота не обробляються; алерти й далі доставляються.\n\
             Повтор спроби — щохвилини."
        }
        PollState::Healthy => "✅ Telegram: опитування відновилося — команди знову працюють.",
        _ => return,
    };
    let users = match store.list_users().await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "не вдалося отримати список адмінів для сповіщення");
            return;
        }
    };
    for u in users {
        if u.role != storage::Role::Admin || u.muted || u.blocked {
            continue;
        }
        if let Err(e) = broadcaster.reply(u.chat_id, text).await {
            tracing::warn!(error = %e, chat = u.chat_id, "не вдалося повідомити адміна");
        }
    }
}

/// Цикл поллінгу Telegram.
async fn telegram_loop<S: Store + Clone + Send + Sync + 'static>(
    store: S,
    router: Arc<AppRouter<S>>,
    broadcaster: Arc<Broadcaster<S, TeloxideNotifier>>,
    poller: TeloxidePoller,
    stop: Arc<AtomicBool>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    cfg: PollCfg,
) -> Result<(), anyhow::Error> {
    let mut last_poll_ok_ms = now_ms();
    let mut last_warn_ms = now_ms();
    let mut offset: i64 = store
        .cursor("telegram_offset")
        .await
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut backoff = Duration::from_secs(1);
    // Чи вже повідомляли про конфлікт поллерів: повторні 409 не повинні
    // засмічувати журнал помилками.
    let mut conflict_reported = false;
    // Коли останнього разу надсилали адмінам повідомлення про стан.
    let mut last_notice_ms = 0i64;

    while !stop.load(Ordering::Relaxed) {
        // Heartbeat: важлива не кількість оновлень, а **живість поллера**.
        // У спокійному бота оновлень може не бути годинами — і це нормально:
        // порожня відповідь на довгий poll теж є «кадром» (§8.4). Перевірка
        // тут, на початку циклу, — щоб видно було, що останнє вдале
        // опитування було давніше за heartbeat.
        let cycle_start = now_ms();
        if cycle_start - last_poll_ok_ms > cfg.heartbeat_ms as i64
            && cycle_start - last_warn_ms > 60_000
        {
            // Попереджаємо не частіше ніж раз на хвилину — інакше воно
            // перетвориться на спам у лозі.
            last_warn_ms = cycle_start;
            tracing::warn!(
                silent_ms = cycle_start - last_poll_ok_ms,
                "Telegram не відповідає на опитування довше, ніж heartbeat-таймаут"
            );
        }
        // Опитування перериваємо на зупинці: інакше процес чекатиме на
        // відповідь Telegram до 25 секунд, systemd вб'є його — і «мертве»
        // getUpdates-з'єднання дасть наступному інстансу 409 Conflict.
        let polled = tokio::select! {
            res = poller.poll(offset) => Some(res),
            _ = shutdown.changed() => None,
        };
        let polled = match polled {
            Some(res) => res,
            None => {
                tracing::info!("опитування Telegram перервано зупинкою");
                return Ok(());
            }
        };
        match polled {
            Ok(updates) => {
                backoff = Duration::from_secs(1);
                // Стан опитування: успішний poll — значить, команди працюють.
                let prev = cfg.health.state();
                let prev_since = cfg.health.since_ms();
                let now = now_ms();
                cfg.health.mark_ok(now);
                if prev == PollState::Conflict
                    && now - prev_since >= CONFLICT_NOTICE_MIN_MS
                    && now - last_notice_ms >= POLL_NOTICE_COOLDOWN_MS
                {
                    // Конфлікт був не миготінням — кажу адмінам, що все
                    // налагодилося.
                    last_notice_ms = now;
                    notify_poll_state(&store, &broadcaster, PollState::Healthy).await;
                }
                // Heartbeat оновлюється успішним опитуванням: порожня
                // відповідь — теж кадр (див. перевірку на початку циклу).
                last_poll_ok_ms = now;
                for upd in updates {
                    offset = offset.max(upd.update_id + 1);
                    // Повторний update не виконуємо двічі.
                    //
                    // `seen_update` повертає true, коли update ЗАПИСАНО вперше
                    // (тобто він новий), і false, коли це дубль. Тут легко
                    // переплутати полярність — і тоді бот обробляє лише
                    // дублікати, а нові пропускає: команди не відповідають.
                    match store.seen_update(upd.update_id).await {
                        Ok(true) => {}
                        Ok(false) => continue,
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
                // На токені вже працює інший полер. Два варіанти:
                //   * exit — завершити процес (суворіше: нічого не дублюємо);
                //   * keep_running — призупинити лише опитування команд, а
                //     збір даних, згортання й доставку алертів лишити
                //     працювати. Опитування повторюється автоматично, тож
                //     коли конфлікт зникне, оживе й обробка команд.
                let now = now_ms();
                let changed = cfg.health.set(PollState::Conflict, now);
                if changed && now - last_notice_ms >= POLL_NOTICE_COOLDOWN_MS {
                    // Новий епізод конфлікту — кажу адмінам: під час
                    // конфлікту `/stats` недоступний, тож повідомлення —
                    // єдиний спосіб дізнатися про причину.
                    last_notice_ms = now;
                    notify_poll_state(&store, &broadcaster, PollState::Conflict).await;
                }
                if cfg.conflict_policy == PollConflictPolicy::Exit {
                    tracing::error!(
                        "Telegram відповів 409: на цьому токені вже працює інший полер. \
                         Політика poll_conflict_policy=exit — зупиняюсь."
                    );
                    stop.store(true, Ordering::Relaxed);
                    return Err(anyhow::anyhow!(
                        "409 Conflict: на токені вже працює інший полер Telegram"
                    ));
                }
                // Пауза перед наступною спробою. Для `preempt` вона коротка:
                // Telegram віддає 409 **старшому** з двох запитів, тож наш
                // повтор (за кілька секунд) переб'є чужий запит — і оновлення
                // забиратимемо ми. Для `keep_running` пауза довга, щоб не
                // смикати API й не засмічувати журнал.
                let delay = bot::config::conflict_retry_delay(
                    cfg.conflict_policy,
                    cfg.preempt_secs,
                    cfg.conflict_retry_secs,
                );
                if !conflict_reported {
                    conflict_reported = true;
                    tracing::error!(
                        "Telegram відповів 409: на цьому токені вже працює інший полер. \
                         Опитування команд призупинено; збір даних і доставка алертів \
                         продовжуються. Повтор опитування через {} с.",
                        delay.as_secs()
                    );
                } else {
                    tracing::warn!(
                        "опитування Telegram все ще недоступне (409) — повтор через {} с",
                        delay.as_secs()
                    );
                }
                // Пауза перед наступною спробою: інакше отримаємо 409 у циклі
                // і засмимо журнал та ліміт Telegram.
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.changed() => {
                        tracing::info!("опитування Telegram перервано зупинкою");
                        return Ok(());
                    }
                }
                continue;
            }
            Err(telegram_gateway::PollerError::InvalidToken(msg)) => {
                cfg.health.set(PollState::Error, now_ms());
                tracing::error!(%msg, "некоректний токен Telegram — зупиняюсь");
                stop.store(true, Ordering::Relaxed);
                return Err(anyhow::anyhow!("некоректний токен Telegram: {msg}"));
            }
            Err(e) => {
                cfg.health.set(PollState::Error, now_ms());
                tracing::warn!(error = %e, "помилка поллінгу Telegram — повторю");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
    Ok(())
}

/// Цикл зрізів джерела.
#[allow(clippy::too_many_arguments)]
async fn snapshot_loop<S: Store + Clone + Send + Sync + 'static>(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<S, TeloxideNotifier>>,
    snapshot_cell: Arc<SnapshotCell>,
    health: Arc<Health>,
    store: S,
    source: Arc<SourceState>,
    interval: Duration,
    stop: Arc<AtomicBool>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match wr.snapshot().await {
            Ok(snap) => {
                // Секції зрізу деградують у порожні, тож сам зріз майже
                // ніколи не падає. Про доступність джерело каже окремо.
                if snap.source_reachable {
                    source.recovered();
                } else if source.failed(now_ms()) {
                    tracing::warn!(
                        "джерело недоступне понад {} с — черга розсилки буферизується",
                        SOURCE_DEGRADED_AFTER_SECS
                    );
                }
                health.last_snapshot_ms.store(now_ms(), Ordering::Relaxed);
                snapshot_cell.set(snap.clone());
                if let Ok(payload) = serde_json::to_string(&snap) {
                    let _ = store.put_snapshot(snap.as_of, &payload).await;
                }
                let alerts = {
                    let mut e = engine.lock().unwrap_or_else(|err| err.into_inner());
                    // Джерело не відповіло — алерти з цього зрізу неповні.
                    e.set_degraded(!snap.source_reachable);
                    e.apply_snapshot(&snap)
                };
                publish_all(&broadcaster, &alerts).await;
                health.degraded.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::warn!(error = %e, "зріз джерела недоступний");
                health.degraded.store(true, Ordering::Relaxed);
                // Джерело недоступне понад grace-період — черга буферизується:
                // алерти чекають, поки джерело оживе (або їх скине межа буфера).
                if source.failed(now_ms()) {
                    tracing::warn!(
                        "джерело недоступне понад {} с — черга розсилки буферизується",
                        SOURCE_DEGRADED_AFTER_SECS
                    );
                }
                let mut e = engine.lock().unwrap_or_else(|err| err.into_inner());
                e.set_degraded(true);
            }
        }
        // Пауза між зрізами переривається зупинкою: інакше процес чекав би
        // на неї до 60 секунд, systemd вбив би його — і «мертвий» процес
        // залишив би після себе некоректний WAL.
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                tracing::info!("цикл зрізів перервано зупинкою");
                return;
            }
        }
    }
}

/// Цикл стрічки подій: WS + страхувальний REST-поллінг.
#[allow(clippy::too_many_arguments)]
async fn event_loop<S: Store + Clone + Send + Sync + 'static>(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<S, TeloxideNotifier>>,
    store: S,
    health: Arc<Health>,
    ws_path: String,
    poll_interval: Duration,
    stop: Arc<AtomicBool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    shutdown_rest: tokio::sync::watch::Receiver<bool>,
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
        tokio::spawn(async move {
            ws_loop(
                wr,
                engine,
                broadcaster,
                store,
                health,
                ws_path,
                stop,
                shutdown_rx.clone(),
            )
            .await
        })
    };

    let poll_handle = {
        let wr = wr.clone();
        let engine = engine.clone();
        let broadcaster = broadcaster.clone();
        let store = store.clone();
        let health = health.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            rest_poll_loop(
                wr,
                engine,
                broadcaster,
                store,
                health,
                RestPollCfg {
                    interval: poll_interval,
                    stop,
                    shutdown: shutdown_rest,
                },
            )
            .await
        })
    };

    let _ = ws_handle.await;
    let _ = poll_handle.await;
}

/// WS-споживач.
#[allow(clippy::too_many_arguments)]
async fn ws_loop<S: Store + Clone + Send + Sync + 'static>(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<S, TeloxideNotifier>>,
    store: S,
    health: Arc<Health>,
    ws_path: String,
    stop: Arc<AtomicBool>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_secs(1);
    while !stop.load(Ordering::Relaxed) {
        match wr.connect_ws(&ws_path).await {
            Ok(mut stream) => {
                tracing::info!("стрічка подій підключена");
                health.ws_connected.store(true, Ordering::Relaxed);
                backoff = Duration::from_secs(1);

                loop {
                    // Очікування кадру теж переривається: інакше цикл
                    // висить на WS-з'єднанні й процес не завершується.
                    let frame = tokio::select! {
                        f = stream.next_message() => f,
                        _ = shutdown.changed() => {
                            tracing::info!("стрічка подій перервана зупинкою");
                            return;
                        }
                    };
                    match frame {
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
        // Пауза перед наступним підключенням теж переривається зупинкою.
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => {
                tracing::info!("стрічка подій перервана зупинкою");
                return;
            }
        }
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

/// Параметри REST-страховки подій.
struct RestPollCfg {
    interval: Duration,
    stop: Arc<AtomicBool>,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

/// REST-страховка: відновлення подій після розриву й заповнення дірок.
async fn rest_poll_loop<S: Store + Clone + Send + Sync + 'static>(
    wr: WrClient,
    engine: Arc<Mutex<alert_engine::Engine>>,
    broadcaster: Arc<Broadcaster<S, TeloxideNotifier>>,
    store: S,
    health: Arc<Health>,
    cfg: RestPollCfg,
) {
    let RestPollCfg {
        interval,
        stop,
        mut shutdown,
    } = cfg;
    while !stop.load(Ordering::Relaxed) {
        // Пауза теж переривається зупинкою — щоб процес завершувався
        // одразу, а не чекав на повний інтервал.
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                tracing::info!("REST-поллінг подій перервано зупинкою");
                return;
            }
        }
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
async fn ingest<S: Store + Clone + Send + Sync + 'static>(
    store: &S,
    engine: &Arc<Mutex<alert_engine::Engine>>,
    broadcaster: &Arc<Broadcaster<S, TeloxideNotifier>>,
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
        // `seen_event` — true, коли подію записано вперше (тобто вона нова).
        match store.seen_event(&ev.id, ev.ts).await {
            Ok(true) => fresh.push(ev),
            Ok(false) => continue,
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
async fn publish_all<S: Store + Clone + Send + Sync + 'static>(
    broadcaster: &Arc<Broadcaster<S, TeloxideNotifier>>,
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
            Ok(broadcast::PublishOutcome::Folded { recipients }) => {
                // Сигнал згорнуто: підписники отримають одне зведене
                // повідомлення за вікно замість окремих на кожен алерт.
                tracing::info!(
                    kind = alert.kind.as_str(),
                    asset = %alert.asset.symbol,
                    recipients,
                    "сигнал згорнуто у зведений дайджест"
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

/// Тривалість оренди доставки (lease), мс.
fn lease_ms(cfg: &bot::config::StorageConfig) -> i64 {
    (cfg.delivery_lease_secs.max(1) as i64).saturating_mul(1000)
}

/// Приховати пароль у URL бази даних — щоб він не потрапив у логи.
///
/// `postgres://user:secret@host/db` → `postgres://user:***@host/db`.
fn redact_url(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(scheme_end), Some(at)) if at > scheme_end => {
            let (creds, rest) = url.split_at(at);
            let user = match creds.split_once(':') {
                Some((u, _)) => u,
                None => creds,
            };
            format!("{}://{user}:***{rest}", &url[..scheme_end])
        }
        _ => url.to_string(),
    }
}
