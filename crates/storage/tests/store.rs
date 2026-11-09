//! Тести сховища стану.

use storage::{
    AlertRecord, AuditEntry, DeliveryStatus, DeliveryUpdate, Language, NewDelivery, NewUser,
    Priority, Role, Settings, SqliteStore, Store, Timeframe, Topic, User, UserState,
};

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

fn alert(key: &str) -> AlertRecord {
    AlertRecord {
        alert_key: key.to_string(),
        kind: "whale".to_string(),
        asset: Some("BTC".to_string()),
        window_start: 1_700_000_000_000,
        severity: "high".to_string(),
        degraded: false,
        payload: "{}".to_string(),
        created_at: 1_700_000_000_000,
    }
}

#[tokio::test]
async fn schema_is_created_and_reopening_is_safe() {
    let path = std::env::temp_dir().join(format!("wallet-radar-schema-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);

    // Перший виклик створює файл і схему.
    let store = SqliteStore::open(&path).expect("відкриття БД");
    let user = store
        .upsert_user(new_user(1, Some(11)))
        .await
        .expect("upsert");
    assert_eq!(user.chat_id, 1);

    // Другий виклик на тому ж файлі не падає (CREATE TABLE IF NOT EXISTS).
    drop(store);
    let store = SqliteStore::open(&path).expect("повторне відкриття БД");
    let user = store
        .get_user(1)
        .await
        .expect("get_user")
        .expect("користувач існує");
    assert_eq!(user.user_id, Some(11));

    // Видалення тестового файлу.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[tokio::test]
async fn upsert_user_is_idempotent_and_updates_fields() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    let mut u = new_user(100, Some(1000));
    u.language = Language::Uk;
    u.username = Some("first".to_string());
    let first = store.upsert_user(u).await.expect("перший upsert");
    assert_eq!(first.username.as_deref(), Some("first"));
    assert_eq!(first.language, Language::Uk);
    assert_eq!(first.state, UserState::Active);

    let mut u2 = new_user(100, Some(1000));
    u2.language = Language::En;
    u2.username = Some("second".to_string());
    u2.role = Role::Admin;
    let second = store.upsert_user(u2).await.expect("другий upsert");

    // Один запис, дані оновлені.
    assert_eq!(second.username.as_deref(), Some("second"));
    assert_eq!(second.language, Language::En);
    assert_eq!(second.role, Role::Admin);
    assert_eq!(second.created_at, first.created_at);
    assert_eq!(store.count_recipients().await.expect("count"), 1);
}

#[tokio::test]
async fn register_alert_is_idempotent() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    assert!(store.register_alert(&alert("key-1")).await.expect("перший"));
    assert!(!store.register_alert(&alert("key-1")).await.expect("другий"));
    assert!(store.alert_exists("key-1").await.expect("exists"));
    assert!(!store.alert_exists("key-2").await.expect("exists"));

    assert!(store
        .register_alert(&alert("key-2"))
        .await
        .expect("інший ключ"));
}

#[tokio::test]
async fn enqueue_delivery_is_idempotent_per_chat_and_alert() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    let d = NewDelivery {
        chat_id: 500,
        alert_key: "a-1".to_string(),
        priority: Priority::Broadcast,
        text: "текст".to_string(),
        created_at: 1_000,
    };
    store.enqueue_delivery(d.clone()).await.expect("перше");
    store.enqueue_delivery(d.clone()).await.expect("друге");

    let claimed = store.claim_deliveries(10_000, 10).await.expect("claim");
    assert_eq!(claimed.len(), 1, "дубль не створюється");
    assert_eq!(claimed[0].chat_id, 500);
    assert_eq!(claimed[0].alert_key, "a-1");
    assert_eq!(claimed[0].attempts, 0);
}

#[tokio::test]
async fn claim_deliveries_orders_by_priority_and_marks_sending() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    for (chat_id, key, priority) in [
        (10_i64, "b-1", Priority::Broadcast),
        (11_i64, "r-1", Priority::Reply),
        (12_i64, "d-1", Priority::Digest),
    ] {
        store
            .enqueue_delivery(NewDelivery {
                chat_id,
                alert_key: key.to_string(),
                priority,
                text: "t".to_string(),
                created_at: 1_000,
            })
            .await
            .expect("enqueue");
    }

    let claimed = store.claim_deliveries(5_000, 10).await.expect("claim");
    assert_eq!(claimed.len(), 3);
    assert_eq!(claimed[0].priority, Priority::Reply);
    assert_eq!(claimed[1].priority, Priority::Digest);
    assert_eq!(claimed[2].priority, Priority::Broadcast);
    assert!(
        claimed.iter().all(|d| d.status == DeliveryStatus::Sending),
        "записи переводяться у sending"
    );

    // Другий claim не повертає вже забірі справ.
    let again = store
        .claim_deliveries(5_000, 10)
        .await
        .expect("повторний claim");
    assert!(again.is_empty());

    // Фіналізація доставки фіксує статус.
    store
        .finish_delivery(DeliveryUpdate {
            chat_id: 11,
            alert_key: "r-1".to_string(),
            status: DeliveryStatus::Sent,
            attempts: 1,
            next_attempt_at: 9_000,
            error: None,
        })
        .await
        .expect("finish");
}

#[tokio::test]
async fn seen_event_and_seen_update_report_first_sighting() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    assert!(store
        .seen_event("evt-1", 1_000)
        .await
        .expect("перший event"));
    assert!(!store
        .seen_event("evt-1", 1_001)
        .await
        .expect("другий event"));
    assert!(store.seen_event("evt-2", 1_002).await.expect("інший event"));

    assert!(store.seen_update(42).await.expect("перший update"));
    assert!(!store.seen_update(42).await.expect("другий update"));
    assert!(store.seen_update(43).await.expect("інший update"));
}

#[tokio::test]
async fn purge_user_removes_user_subscriptions_deliveries_and_audit() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    let user = store
        .upsert_user(new_user(700, Some(7000)))
        .await
        .expect("upsert");
    store
        .set_subscription(&storage::Subscription {
            chat_id: 700,
            topic: Topic::Whale,
            min_severity: "high".to_string(),
        })
        .await
        .expect("subscription");
    store.set_user_asset(700, "btc", true).await.expect("asset");
    store
        .enqueue_delivery(NewDelivery {
            chat_id: 700,
            alert_key: "x-1".to_string(),
            priority: Priority::Reply,
            text: "t".to_string(),
            created_at: 1_000,
        })
        .await
        .expect("enqueue");
    let chat_ref = store.chat_ref(700);
    store
        .audit(AuditEntry {
            chat_ref: chat_ref.clone(),
            actor: "user".to_string(),
            action: "subscribe".to_string(),
            detail: None,
            at: 1_000,
        })
        .await
        .expect("audit");
    assert_eq!(store.audit_count(&chat_ref).await.expect("audit_count"), 1);

    let purged = store.purge_user(7000).await.expect("purge");
    assert_eq!(purged.users, 1);
    assert_eq!(purged.deliveries, 1);
    assert_eq!(purged.audits, 1);
    assert!(purged.users > 0 && purged.deliveries > 0 && purged.audits > 0);

    assert!(store.get_user(700).await.expect("get").is_none());
    assert!(store
        .subscriptions(700)
        .await
        .expect("subscriptions")
        .is_empty());
    assert!(store.user_assets(700).await.expect("assets").is_empty());
    assert!(store
        .claim_deliveries(10_000, 10)
        .await
        .expect("claim")
        .is_empty());
    assert_eq!(store.audit_count("chat:700").await.expect("audit_count"), 0);

    // Профіль за user_id теж зник.
    assert!(store
        .find_user_by_user_id(7000)
        .await
        .expect("find")
        .is_none());
    let _ = user;
}

#[tokio::test]
async fn users_are_isolated_from_each_other() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    store
        .upsert_user(new_user(1, Some(11)))
        .await
        .expect("upsert");
    store
        .upsert_user(new_user(2, Some(22)))
        .await
        .expect("upsert");

    store
        .set_subscription(&storage::Subscription {
            chat_id: 1,
            topic: Topic::Whale,
            min_severity: "medium".to_string(),
        })
        .await
        .expect("sub 1");
    store
        .set_subscription(&storage::Subscription {
            chat_id: 2,
            topic: Topic::Liquidation,
            min_severity: "medium".to_string(),
        })
        .await
        .expect("sub 2");

    let subs1 = store.subscriptions(1).await.expect("subs 1");
    let subs2 = store.subscriptions(2).await.expect("subs 2");
    assert_eq!(subs1.len(), 1);
    assert_eq!(subs1[0].topic, Topic::Whale);
    assert_eq!(subs2.len(), 1);
    assert_eq!(subs2[0].topic, Topic::Liquidation);

    assert_eq!(
        store.recipients_for(Topic::Whale).await.expect("rec 1"),
        vec![1]
    );
    assert_eq!(
        store
            .recipients_for(Topic::Liquidation)
            .await
            .expect("rec 2"),
        vec![2]
    );
    assert!(store
        .recipients_for(Topic::Digest)
        .await
        .expect("rec 3")
        .is_empty());

    // Прибирання підписки одного не зачіпає іншого.
    store
        .remove_subscription(1, Topic::Whale)
        .await
        .expect("rm");
    assert!(store.subscriptions(1).await.expect("subs 1").is_empty());
    assert_eq!(store.subscriptions(2).await.expect("subs 2").len(), 1);
}

#[tokio::test]
async fn set_blocked_toggles_flag_and_excludes_from_active_chats() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");

    for chat_id in [1_i64, 2, 3] {
        store
            .upsert_user(new_user(chat_id, Some(chat_id * 10)))
            .await
            .expect("upsert");
    }
    assert_eq!(
        store.all_active_chats().await.expect("active"),
        vec![1, 2, 3]
    );

    // Блокування змінює поле і виключає чат з активних.
    store.set_blocked(2, true).await.expect("block");
    let u = store.get_user(2).await.expect("get").expect("user");
    assert!(u.blocked);
    assert_eq!(store.all_active_chats().await.expect("active"), vec![1, 3]);
    assert_eq!(store.count_recipients().await.expect("count"), 2);

    // Розблокування повертає чат.
    store.set_blocked(2, false).await.expect("unblock");
    let u = store.get_user(2).await.expect("get").expect("user");
    assert!(!u.blocked);
    assert_eq!(
        store.all_active_chats().await.expect("active"),
        vec![1, 2, 3]
    );

    // Muted також виключає чат.
    store
        .set_settings(
            3,
            &Settings {
                language: Language::Uk,
                timezone: "UTC".to_string(),
                timeframe: Timeframe::H24,
                muted: true,
            },
        )
        .await
        .expect("settings");
    assert_eq!(store.all_active_chats().await.expect("active"), vec![1, 2]);
}

#[tokio::test]
async fn settings_cursor_snapshot_and_audit_are_persisted() {
    let store = SqliteStore::open_in_memory().expect("відкриття БД");
    store
        .upsert_user(new_user(900, Some(9000)))
        .await
        .expect("upsert");

    store
        .set_settings(
            900,
            &Settings {
                language: Language::En,
                timezone: "Europe/Kyiv".to_string(),
                timeframe: Timeframe::D7,
                muted: true,
            },
        )
        .await
        .expect("settings");
    let u: User = store.get_user(900).await.expect("get").expect("user");
    assert_eq!(u.language, Language::En);
    assert_eq!(u.timezone, "Europe/Kyiv");
    assert_eq!(u.timeframe, Timeframe::D7);
    assert!(u.muted);

    // Muted-користувач не потрапляє до одержувачів.
    store
        .set_subscription(&storage::Subscription {
            chat_id: 900,
            topic: Topic::Whale,
            min_severity: "medium".to_string(),
        })
        .await
        .expect("sub");
    assert!(store
        .recipients_for(Topic::Whale)
        .await
        .expect("rec")
        .is_empty());

    store.set_last_seen(900, 12_345).await.expect("last_seen");
    let u = store.get_user(900).await.expect("get").expect("user");
    assert_eq!(u.last_seen_at, Some(12_345));

    assert!(store.cursor("w1").await.expect("cursor").is_none());
    store.set_cursor("w1", "v1").await.expect("set cursor");
    assert_eq!(
        store.cursor("w1").await.expect("cursor"),
        Some("v1".to_string())
    );

    assert!(store.last_snapshot().await.expect("snapshot").is_none());
    store.put_snapshot(1_000, "{\"a\":1}").await.expect("put");
    store.put_snapshot(2_000, "{\"a\":2}").await.expect("put");
    assert_eq!(
        store.last_snapshot().await.expect("snapshot"),
        Some((2_000, "{\"a\":2}".to_string()))
    );

    store
        .audit(AuditEntry {
            chat_ref: "chat:900".to_string(),
            actor: "admin".to_string(),
            action: "block".to_string(),
            detail: Some("тест".to_string()),
            at: 1_000,
        })
        .await
        .expect("audit");
    assert_eq!(store.audit_count("chat:900").await.expect("count"), 1);
}
