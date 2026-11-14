//! Тести адмінської команди `/health`.
//!
//! `/health` — це «чи зараз усе живо» в одному екрані: стан опитування
//! Telegram, доступність джерела, свіжість зрізу, черга на відсилання.
//! `/stats` дає накопичені лічильники, а `/health` — поточний стан.
//!
//! Перевіряємо три речі:
//!
//! 1. **Доступ адміністру.** Команда не адмінська — не зветься навіть адміну.
//! 2. **Зміст.** У відповіді є стан опитування, джерело, свіжість зрізу й черга.
//! 3. **Чесність.** Коли джерело недоступне, бот каже про це прямо, а не
//!    мовчить або показує нульові цифри як робочі.

use admin::{AdminError, AdminService, AdminStats};
use storage::{
    AlertRecord, Language, NewUser, Purged, Settings, Timeframe, Topic, User,
};
use telegram_gateway::{ChatType, IncomingUpdate, Router, RouterConfig};
use user_session::{Session, SessionError};
use wr_client::Snapshot;

/// Мок сесії: усі методи повертають порожні/дефолтні значення.
struct MockSession;

impl Session for MockSession {
    async fn user(&self, _chat_id: i64) -> Result<User, SessionError> {
        Err(SessionError::NotFound(0))
    }
    async fn settings(&self, _chat_id: i64) -> Result<Settings, SessionError> {
        Ok(Settings {
            language: Language::Uk,
            timezone: "UTC".to_string(),
            timeframe: Timeframe::H24,
            muted: false,
        })
    }
    async fn register(&self, _u: NewUser) -> Result<User, SessionError> {
        Err(SessionError::NotFound(0))
    }
    async fn subscribe(&self, _chat_id: i64, _topic: Topic) -> Result<(), SessionError> {
        Ok(())
    }
    async fn unsubscribe(&self, _chat_id: i64, _topic: Topic) -> Result<(), SessionError> {
        Ok(())
    }
    async fn set_muted(&self, _chat_id: i64, _muted: bool) -> Result<(), SessionError> {
        Ok(())
    }
    async fn set_language(&self, _chat_id: i64, _lang: Language) -> Result<(), SessionError> {
        Ok(())
    }
    async fn set_timeframe(&self, _chat_id: i64, _tf: Timeframe) -> Result<(), SessionError> {
        Ok(())
    }
    async fn purge(&self, _user_id: i64) -> Result<Purged, SessionError> {
        Ok(Purged {
            users: 0,
            deliveries: 0,
            audits: 0,
        })
    }
    async fn alerts_since(
        &self,
        _chat_id: i64,
        _since_ms: i64,
    ) -> Result<Vec<AlertRecord>, SessionError> {
        Ok(Vec::new())
    }
}

/// Мок адмін-сервісу: адмін — `admin`, лічильники — задані.
struct MockAdmin {
    stats: AdminStats,
}

impl MockAdmin {
    fn new() -> MockAdmin {
        MockAdmin {
            stats: AdminStats {
                recipients: 3,
                alerts_published: 10,
                alerts_duplicate: 1,
                alerts_folded: 2,
                digests_sent: 1,
                messages_sent: 9,
                messages_failed: 1,
                messages_retried: 2,
                messages_skipped: 0,
                messages_expired: 0,
                backlog_pending: 4,
            },
        }
    }
}

impl AdminService for MockAdmin {
    fn is_admin(&self, username: Option<&str>) -> bool {
        username.map(|u| u.eq_ignore_ascii_case("admin")).unwrap_or(false)
    }
    async fn stats(&self) -> Result<AdminStats, AdminError> {
        Ok(self.stats.clone())
    }
    async fn broadcast_manual(
        &self,
        _actor_chat_id: i64,
        _actor_username: Option<&str>,
        _text: &str,
    ) -> Result<usize, AdminError> {
        Ok(0)
    }
    async fn block(
        &self,
        _actor_chat_id: i64,
        _target_chat_id: i64,
        _blocked: bool,
    ) -> Result<(), AdminError> {
        Ok(())
    }
}

fn update(username: Option<&str>, text: &str) -> IncomingUpdate {
    IncomingUpdate {
        update_id: 1,
        chat_id: 42,
        chat_type: ChatType::Private,
        user_id: Some(7),
        username: username.map(|s| s.to_string()),
        text: text.to_string(),
        reply_to_message_id: None,
    }
}

/// Роутер з моками. `source_reachable` керує доступністю джерела.
fn router(source_reachable: bool) -> Router<MockSession, MockAdmin> {
    let snapshot = telegram_gateway::SnapshotCell::new();
    let mut snap = Snapshot::default();
    snap.source_reachable = source_reachable;
    // Зріз минулої хвилини.
    snap.as_of = telegram_gateway::now_ms() - 60_000;
    snapshot.set(snap);

    let cfg = RouterConfig {
        default_language: "uk".to_string(),
        admins: vec!["admin".to_string()],
        // Ліміт команд: у тесті кілька запитів підряд, тож має бути
        // достатньо (0 — відкидає все).
        rate_limits: (60, 60, 600),
        ..Default::default()
    };
    Router::new(MockSession, MockAdmin::new(), snapshot, cfg)
}

#[test]
fn health_is_admin_only() {
    let r = router(true);
    // Не адмін: команда не розпізнається.
    let reply = futures_block_on(r.handle(&update(Some("user"), "/health")));
    let text = reply.map(|x| x.text).unwrap_or_default();
    assert!(
        !text.contains("Стан бота"),
        "не-адмін не повинен отримати /health: {text}"
    );

    // Адмін: отримує.
    let reply = futures_block_on(r.handle(&update(Some("admin"), "/health")));
    let text = reply.map(|x| x.text).unwrap_or_default();
    assert!(text.contains("Стан бота"), "адмін має отримати /health: {text}");
}

#[test]
fn health_reports_state() {
    let r = router(true);
    let reply = futures_block_on(r.handle(&update(Some("admin"), "/health")));
    let text = reply.map(|x| x.text).unwrap_or_default();

    assert!(text.contains("команди:"), "немає стану опитування: {text}");
    assert!(text.contains("джерело: доступне"), "джерело: {text}");
    assert!(text.contains("зріз:"), "немає свіжості зрізу: {text}");
    assert!(text.contains("черга:"), "немає стану черги: {text}");
    // Черга з лічильників статистики.
    assert!(text.contains('4'), "черга не показана: {text}");
}

#[test]
fn health_is_honest_when_source_is_down() {
    let r = router(false);
    let reply = futures_block_on(r.handle(&update(Some("admin"), "/health")));
    let text = reply.map(|x| x.text).unwrap_or_default();
    assert!(
        text.contains("НЕДОСТУПНЕ"),
        "недоступне джерело має бути сказано прямо: {text}"
    );
}

#[test]
fn age_formatting() {
    let now = 1_000_000_000;
    assert_eq!(telegram_gateway::fmt_age(now, 0), "— (ще не було)");
    assert_eq!(telegram_gateway::fmt_age(now, now), "щойно");
    assert_eq!(telegram_gateway::fmt_age(now, now - 60_000), "1 хв тому");
    assert_eq!(telegram_gateway::fmt_age(now, now - 30 * 60_000), "30 хв тому");
    assert_eq!(telegram_gateway::fmt_age(now, now - 3 * 3_600_000), "3 год тому");
    assert_eq!(telegram_gateway::fmt_age(now, now - 50 * 3_600_000), "2 дн тому");
}

/// Простий блокер: тести синхронні, але API асинхронний.
fn futures_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(f)
}
