//! Тести сесії користувача.

use storage::{AlertRecord, Language, NewUser, Role, SqliteStore, Store, Timeframe, Topic};
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

/// Запис у журналі `alert_log` для перевірки фільтрів дайджесту.
fn alert(key: &str, kind: &str, asset: &str, severity: &str, created_at: i64) -> AlertRecord {
    AlertRecord {
        alert_key: key.to_string(),
        kind: kind.to_string(),
        asset: Some(asset.to_string()),
        window_start: created_at - 3_600_000,
        severity: severity.to_string(),
        degraded: false,
        payload: "{}".to_string(),
        created_at,
    }
}

#[tokio::test]
async fn alerts_since_filters_by_topic_and_severity() {
    let s = session();
    s.register(new_user(1, Some(1))).await.expect("register");
    // Підписка лише на «китів» із мінімальною важливістю medium.
    s.subscribe(1, Topic::Whale).await.expect("subscribe");

    s.store()
        .register_alert(&alert("a1", "whale_accumulation", "SOL", "medium", 5_000))
        .await
        .expect("whale medium");
    s.store()
        .register_alert(&alert("a2", "whale_accumulation", "ETH", "info", 5_000))
        .await
        .expect("whale info");
    s.store()
        .register_alert(&alert("a3", "liquidation_spike", "BTC", "high", 5_000))
        .await
        .expect("liquidation high");

    let rows = s.alerts_since(1, 0).await.expect("alerts_since");
    assert_eq!(rows.len(), 1, "лише китів із важливістю не нижче medium");
    assert_eq!(rows[0].alert_key, "a1");
}

#[tokio::test]
async fn alerts_since_filters_by_tracked_assets() {
    let s = session();
    s.register(new_user(2, Some(2))).await.expect("register");
    s.subscribe(2, Topic::Whale).await.expect("subscribe");
    s.store()
        .set_user_asset(2, "SOL", true)
        .await
        .expect("asset");

    s.store()
        .register_alert(&alert("b1", "whale_accumulation", "SOL", "medium", 5_000))
        .await
        .expect("sol");
    s.store()
        .register_alert(&alert("b2", "whale_accumulation", "ETH", "medium", 5_000))
        .await
        .expect("eth");

    let rows = s.alerts_since(2, 0).await.expect("alerts_since");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].alert_key, "b1");
}

#[tokio::test]
async fn alerts_since_without_subscriptions_shows_everything() {
    let s = session();
    s.register(new_user(3, Some(3))).await.expect("register");

    s.store()
        .register_alert(&alert("c1", "whale_accumulation", "SOL", "info", 5_000))
        .await
        .expect("whale");
    s.store()
        .register_alert(&alert("c2", "liquidation_spike", "BTC", "high", 5_000))
        .await
        .expect("liq");

    let rows = s.alerts_since(3, 0).await.expect("alerts_since");
    assert_eq!(rows.len(), 2, "без підписок показуємо все");
}

#[tokio::test]
async fn alerts_since_skips_manual_broadcasts_and_respects_window() {
    let s = session();
    s.register(new_user(4, Some(4))).await.expect("register");

    s.store()
        .register_alert(&alert("d1", "manual", "", "high", 9_000))
        .await
        .expect("manual");
    s.store()
        .register_alert(&alert("d2", "whale_accumulation", "SOL", "medium", 5_000))
        .await
        .expect("whale");

    // Вікно від 6_000: ручна розсилка й алерт поза вікном — геть.
    let rows = s.alerts_since(4, 6_000).await.expect("alerts_since");
    assert!(rows.is_empty());

    // Ширше вікно — лише ринковий алерт, ручна розсилка не проходить.
    let rows = s.alerts_since(4, 0).await.expect("alerts_since");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].alert_key, "d2");
}
