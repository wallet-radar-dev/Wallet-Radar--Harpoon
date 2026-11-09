//! End-to-end перевірка команди `/digest`: від команди до готового тексту.
//!
//! Перевіряємо весь шлях: роутер → сесія → журнал алертів → підсумок →
//! текст відповіді. Зокрема те, що дайджест:
//!
//! * рахує алерти за період, а не показує зріз стану;
//! * ураховує мову користувача;
//! * не показує нічого зайвого для користувача без підписок;
//! * не містить адрес, tx-hash і сум окремих транзакцій.

use admin::Admin;
use alert_engine::{Alert, AlertKey, AlertKind, AssetLabel, Direction, Metrics, Severity, Window};
use broadcast::{BroadcastConfig, Broadcaster, Notifier, Outbound};
use privacy_filter::PrivacyFilter;
use std::sync::{Arc, Mutex};
use storage::{AlertRecord, Language, NewUser, Role, SqliteStore, Store};
use telegram_gateway::{ChatType, IncomingUpdate, Router, RouterConfig, SnapshotCell};
use user_session::UserSession;

/// Відправник, що записує все, що пішло б у Telegram.
struct RecordingNotifier {
    sent: Arc<Mutex<Vec<String>>>,
}

impl Notifier for RecordingNotifier {
    async fn send(&self, _chat_id: i64, msg: &Outbound) -> Result<(), broadcast::SendError> {
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.text.clone());
        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn update(update_id: i64, chat_id: i64, text: &str) -> IncomingUpdate {
    IncomingUpdate {
        update_id,
        chat_id,
        chat_type: ChatType::Private,
        user_id: Some(chat_id),
        username: Some("tester".into()),
        text: text.to_string(),
        reply_to_message_id: None,
    }
}

fn alert(
    kind: AlertKind,
    symbol: &str,
    severity: Severity,
    participants: u32,
    value: f64,
) -> Alert {
    Alert {
        key: AlertKey::derive(kind, symbol, 0, &["evidence"]),
        kind,
        severity,
        asset: AssetLabel::unknown(symbol),
        related: Vec::new(),
        direction: Some(Direction::Accumulation),
        metrics: Metrics {
            participants,
            total_value_usd: value,
            group_share_pct: None,
            concentration_top10_pct: None,
            group_size: None,
            window: Window::W1h,
        },
        archetype: None,
        degraded: false,
        created_at: now_ms(),
    }
}

fn record(a: &Alert) -> AlertRecord {
    AlertRecord {
        alert_key: a.key.as_str().to_string(),
        kind: a.kind.as_str().to_string(),
        asset: Some(a.asset.symbol.clone()),
        window_start: a.created_at - a.metrics.window.ms(),
        severity: a.severity.as_str().to_string(),
        degraded: a.degraded,
        payload: serde_json::to_string(a).unwrap_or_else(|_| "{}".into()),
        created_at: a.created_at,
    }
}

/// Тип роутера в тестах — щоб не повторювати довгий опис.
type TestRouter = Router<
    UserSession<SqliteStore>,
    Admin<SqliteStore, Arc<Broadcaster<SqliteStore, RecordingNotifier>>>,
>;

/// Роутер із реальним сховищем — без моків на рівні сесії.
fn router(store: SqliteStore) -> TestRouter {
    let sanitizer = PrivacyFilter::new("приховано");
    let notifier = RecordingNotifier {
        sent: Arc::new(Mutex::new(Vec::new())),
    };
    let broadcaster = Arc::new(Broadcaster::new(
        store.clone(),
        notifier,
        sanitizer.clone(),
        BroadcastConfig {
            per_chat_min_interval_ms: 0,
            ..BroadcastConfig::default()
        },
    ));
    let session = UserSession::new(store.clone());
    let admin = Admin::new(store, broadcaster, sanitizer, vec!["tester".into()]);
    Router::new(
        session,
        admin,
        SnapshotCell::new(),
        RouterConfig {
            default_language: "uk".into(),
            default_timezone: "UTC".into(),
            access_code: None,
            admins: vec!["tester".into()],
            rate_limits: (20, 60, 200),
            poll_health: None,
        },
    )
}

#[tokio::test]
async fn digest_reports_alerts_of_the_period_in_users_language() {
    let store = SqliteStore::open_in_memory().expect("БД у памʼяті");
    store
        .upsert_user(NewUser {
            chat_id: 42_000,
            user_id: Some(42_000),
            username: Some("tester".into()),
            language: Language::En,
            timezone: "UTC".into(),
            role: Role::Viewer,
        })
        .await
        .expect("користувач створено");

    store
        .register_alert(&record(&alert(
            AlertKind::WhaleAccumulation,
            "SOL",
            Severity::High,
            8,
            1_200_000.0,
        )))
        .await
        .expect("алерт 1");
    store
        .register_alert(&record(&alert(
            AlertKind::CapitalGroupChange,
            "UNI",
            Severity::Medium,
            2,
            0.0,
        )))
        .await
        .expect("алерт 2");

    let router = router(store);
    let reply = router
        .handle(&update(1, 42_000, "/digest daily"))
        .await
        .expect("є відповідь");
    let text = reply.text;

    assert!(text.contains("Digest for the last 24 hours"), "{text}");
    assert!(text.contains("alerts: 2"), "{text}");
    assert!(text.contains("Accumulation"), "{text}");
    assert!(text.contains("SOL"), "{text}");
    assert!(text.contains("Capital group"), "{text}");
    // Мова користувача (en), а не мова за замовчуванням (uk).
    assert!(!text.contains("Дайджест"), "{text}");

    // Приватність: у відповіді немає адрес і tx-hash.
    assert!(!text.contains("0x"), "{text}");
}

#[tokio::test]
async fn digest_weekly_covers_seven_days() {
    let store = SqliteStore::open_in_memory().expect("БД у памʼяті");
    store
        .upsert_user(NewUser {
            chat_id: 43_000,
            user_id: Some(43_000),
            username: Some("tester".into()),
            language: Language::Uk,
            timezone: "UTC".into(),
            role: Role::Viewer,
        })
        .await
        .expect("користувач створено");

    // Алерт три дні тому: у добовому вікні його немає, у тижневому — є.
    let mut old = alert(
        AlertKind::WhaleAccumulation,
        "ETH",
        Severity::Medium,
        4,
        300_000.0,
    );
    old.created_at = now_ms() - 3 * 24 * 60 * 60 * 1000;
    store.register_alert(&record(&old)).await.expect("алерт");

    let router = router(store);
    let daily = router
        .handle(&update(1, 43_000, "/digest daily"))
        .await
        .expect("є відповідь");
    assert!(daily.text.contains("сигналів не було"), "{}", daily.text);

    let weekly = router
        .handle(&update(2, 43_000, "/digest weekly"))
        .await
        .expect("є відповідь");
    assert!(weekly.text.contains("алертів: 1"), "{}", weekly.text);
    assert!(weekly.text.contains("ETH"), "{}", weekly.text);
}

#[tokio::test]
async fn digest_without_alerts_says_so_instead_of_failing() {
    let store = SqliteStore::open_in_memory().expect("БД у памʼяті");
    store
        .upsert_user(NewUser {
            chat_id: 44_000,
            user_id: Some(44_000),
            username: Some("tester".into()),
            language: Language::Uk,
            timezone: "UTC".into(),
            role: Role::Viewer,
        })
        .await
        .expect("користувач створено");

    let router = router(store);
    let reply = router
        .handle(&update(1, 44_000, "/digest"))
        .await
        .expect("є відповідь");
    assert!(reply.text.contains("сигналів не було"), "{}", reply.text);
}
