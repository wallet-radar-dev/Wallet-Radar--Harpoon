//! Стан опитування Telegram: переходи, опис і показ у `/stats`.
//!
//! Стан потрібен через конфлікт поллерів: поки на токені працює інший
//! полер, сам `/stats` не надійде — тож адмін має бачити причину в
//! статистиці (а про сам конфлікт — у окремому повідомленні).

use admin::Admin;
use broadcast::{BroadcastConfig, Broadcaster, Notifier, Outbound};
use privacy_filter::PrivacyFilter;
use std::sync::{Arc, Mutex};
use storage::{Language, NewUser, Role, SqliteStore, Store};
use telegram_gateway::{
    ChatType, IncomingUpdate, PollHealth, PollState, Router, RouterConfig, SnapshotCell,
};
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

type TestRouter = Router<
    UserSession<SqliteStore>,
    Admin<SqliteStore, Arc<Broadcaster<SqliteStore, RecordingNotifier>>>,
>;

/// Роутер з адміном і (необовʼязковим) станом опитування.
fn router(store: SqliteStore, health: Option<Arc<PollHealth>>) -> TestRouter {
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
            poll_health: health,
        },
    )
}

#[tokio::test]
async fn стан_поллера_перемикається_і_повідомляє_про_зміну_один_раз() {
    let health = PollHealth::new();
    assert_eq!(health.state(), PollState::Unknown);

    // Перша зміна — так; повтор того самого стану — ні.
    assert!(health.set(PollState::Conflict, 1_000));
    assert!(!health.set(PollState::Conflict, 2_000));
    assert_eq!(health.state(), PollState::Conflict);
    assert_eq!(health.since_ms(), 1_000);

    assert!(health.set(PollState::Healthy, 3_000));
    assert_eq!(health.state(), PollState::Healthy);
    assert_eq!(health.since_ms(), 3_000);

    // Успішне опитування фіксує час останньої успішної спроби.
    // Стан уже «Healthy», тож зміни немає — `mark_ok` повертає false.
    assert!(!health.mark_ok(4_000));
    assert_eq!(health.last_ok_ms(), 4_000);
}

#[tokio::test]
async fn опис_стану_каже_про_конфлікт_поллерів() {
    let health = PollHealth::new();
    health.set(PollState::Conflict, 1_000);
    let text = health.describe();
    assert!(text.contains("409"), "має згадати код конфлікту: {text}");
    assert!(
        text.contains("поллер"),
        "має згадати іншого поллера: {text}"
    );
}

#[tokio::test]
async fn stats_показує_стан_опитування() {
    let store = SqliteStore::open_in_memory().expect("БД у памʼяті");
    store
        .upsert_user(NewUser {
            chat_id: 42_000,
            user_id: Some(42_000),
            username: Some("tester".into()),
            language: Language::Uk,
            timezone: "UTC".into(),
            role: Role::Admin,
        })
        .await
        .expect("користувач створено");

    let health = Arc::new(PollHealth::new());
    health.set(PollState::Conflict, 1_000);

    let router = router(store, Some(health));
    let reply = router
        .handle(&update(1, 42_000, "/stats"))
        .await
        .expect("є відповідь");
    let text = reply.text;

    assert!(text.contains("команди:"), "{text}");
    assert!(text.contains("409"), "має бути видно конфлікт: {text}");
    // І звичайна статистика на місці.
    assert!(text.contains("буфер:"), "{text}");
}

#[tokio::test]
async fn stats_без_стану_поллера_не_ламається() {
    let store = SqliteStore::open_in_memory().expect("БД у памʼяті");
    store
        .upsert_user(NewUser {
            chat_id: 42_001,
            user_id: Some(42_001),
            username: Some("tester".into()),
            language: Language::Uk,
            timezone: "UTC".into(),
            role: Role::Admin,
        })
        .await
        .expect("користувач створено");

    let router = router(store, None);
    let reply = router
        .handle(&update(1, 42_001, "/stats"))
        .await
        .expect("є відповідь");
    let text = reply.text;

    assert!(text.contains("підключено:"), "{text}");
    assert!(!text.contains("команди:"), "без стану — без рядка: {text}");
}
