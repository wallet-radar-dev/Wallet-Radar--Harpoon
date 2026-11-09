//! Тести сесії користувача.

use storage::{Language, NewUser, Role, SqliteStore, Store, Timeframe, Topic};
use user_session::{Session, SessionError, UserSession};

fn session() -> UserSession<SqliteStore> {
    UserSession::new(SqliteStore::open_in_memory().expect("відкриття БД"))
}

fn new_user(chat_id: i64, user_id: Option<i64>) -> NewUser {
    NewUser {
        chat_id,
        user_id,
        username: Some("tester".to_string()),
        language: Language::Uk,
        timezone: "UTC".to_string(),
        role: Role::Viewer,
    }
}

#[tokio::test]
async fn register_and_read_user() {
    let s = session();
    let user = s.register(new_user(10, Some(100))).await.expect("register");
    assert_eq!(user.chat_id, 10);
    assert_eq!(user.language, Language::Uk);

    let again = s.user(10).await.expect("user");
    assert_eq!(again.user_id, Some(100));

    // Невідомий чат — помилка, а не падіння.
    assert!(matches!(
        s.user(999).await,
        Err(SessionError::NotFound(999))
    ));
}

#[tokio::test]
async fn settings_reflect_updates() {
    let s = session();
    s.register(new_user(11, Some(111))).await.expect("register");

    let st = s.settings(11).await.expect("settings");
    assert_eq!(st.language, Language::Uk);
    assert_eq!(st.timeframe, Timeframe::H24);
    assert!(!st.muted);

    s.set_language(11, Language::En).await.expect("lang");
    s.set_timeframe(11, Timeframe::D30).await.expect("tf");
    s.set_muted(11, true).await.expect("muted");

    let st = s.settings(11).await.expect("settings");
    assert_eq!(st.language, Language::En);
    assert_eq!(st.timeframe, Timeframe::D30);
    assert!(st.muted);
}

#[tokio::test]
async fn subscribe_and_unsubscribe_are_isolated_per_chat() {
    let s = session();
    s.register(new_user(20, Some(200))).await.expect("register");
    s.register(new_user(21, Some(201))).await.expect("register");

    s.subscribe(20, Topic::Whale).await.expect("sub");
    s.subscribe(21, Topic::Liquidation).await.expect("sub");

    let subs20 = s.store().subscriptions(20).await.expect("subs");
    let subs21 = s.store().subscriptions(21).await.expect("subs");
    assert_eq!(subs20.len(), 1);
    assert_eq!(subs20[0].topic, Topic::Whale);
    assert_eq!(subs21.len(), 1);
    assert_eq!(subs21[0].topic, Topic::Liquidation);

    s.unsubscribe(20, Topic::Whale).await.expect("unsub");
    assert!(s.store().subscriptions(20).await.expect("subs").is_empty());
    assert_eq!(s.store().subscriptions(21).await.expect("subs").len(), 1);
}

#[tokio::test]
async fn subscribe_creates_missing_user_placeholder() {
    let s = session();
    // Користувача ще немає — підписка має створити запис (FK вимагає існування users).
    s.subscribe(30, Topic::Digest).await.expect("sub");
    let u = s.user(30).await.expect("user");
    assert_eq!(u.chat_id, 30);
    assert_eq!(u.role, Role::Viewer);
}

#[tokio::test]
async fn purge_removes_user_data() {
    let s = session();
    s.register(new_user(40, Some(400))).await.expect("register");
    s.subscribe(40, Topic::Group).await.expect("sub");
    let chat_ref = s.store().chat_ref(40);
    s.store()
        .audit(storage::AuditEntry {
            chat_ref: chat_ref.clone(),
            actor: "user".to_string(),
            action: "subscribe".to_string(),
            detail: None,
            at: 1_000,
        })
        .await
        .expect("audit");

    let purged = s.purge(400).await.expect("purge");
    assert_eq!(purged.users, 1);
    assert!(purged.audits > 0);

    assert!(matches!(s.user(40).await, Err(SessionError::NotFound(40))));
    assert_eq!(s.store().audit_count(&chat_ref).await.expect("audit"), 0);
}
