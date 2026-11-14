//! Тести сховища на PostgreSQL.
//!
//! Тести потребують живої бази: URL береться зі змінної оточення
//! `POSTGRES_TEST_URL` (напр. `postgres://wr:pw@127.0.0.1:5433/wallet_radar_test`).
//! Якщо змінної немає — тести пропускаються, а не падають: у CI без бази
//! вони просто не виконуються.
//!
//! Кожен тест працює в окремій схемі (`test_<uuid>`), тож тести не заважають
//! один одному й не чіпають робочу базу.

use sqlx::postgres::PgPoolOptions;
use storage::{
    AlertRecord, AuditEntry, DeliveryStatus, DeliveryUpdate, Language, NewDelivery, NewUser,
    PostgresStore, Priority, Role, Settings, Store, Timeframe, Topic, UserState,
};

/// Прибирання схеми після тесту — найкраще зусилля.
struct SchemaGuard {
    url: String,
    schema: String,
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        let url = self.url.clone();
        let schema = self.schema.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Ok(admin) = PgPoolOptions::new().max_connections(1).connect(&url).await {
                    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
                        .execute(&admin)
                        .await
                        .ok();
                    admin.close().await;
                }
            });
        }
    }
}

/// Створює ізольоване сховище у тимчасовій схемі.
///
/// `None` — немає змінної `POSTGRES_TEST_URL`, тест пропускається.
/// Повертається разом із guard'ом: поки він живий, схема існує, а коли
/// тест завершиться — схема прибирається.
async fn test_store() -> Option<(PostgresStore, SchemaGuard)> {
    let url = std::env::var("POSTGRES_TEST_URL").ok()?;
    let schema = format!("test_{}", uuidish());

    // Схему створюємо окремим з'єднанням — без `search_path`.
    {
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("підключення до тестової бази Postgres");
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
            .execute(&admin)
            .await
            .expect("створення тестової схеми");
        admin.close().await;
    }

    let schema_for_conn = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let schema = schema_for_conn.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {schema}"))
                    .execute(conn)
                    .await?;
                Ok::<(), sqlx::Error>(())
            })
        })
        .connect(&url)
        .await
        .expect("підключення до тестової бази Postgres");

    let store = PostgresStore::from_pool(pool, b"test-secret");
    store.migrate().await.expect("застосування схеми");

    let guard = SchemaGuard {
        url: url.clone(),
        schema,
    };
    Some((store, guard))
}

/// Простий унікальний суфікс схеми (без залежності на uuid-крейт).
fn uuidish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
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
async fn схема_створюється_і_повторне_відкриття_безпечне() {
    let Some((store, _guard)) = test_store().await else {
        eprintln!("POSTGRES_TEST_URL не задано — тест пропущено");
        return;
    };

    let user = store.upsert_user(new_user(1, Some(11))).await.expect("upsert");
    assert_eq!(user.chat_id, 1);
    assert_eq!(user.state, UserState::Active);
    assert!(!user.muted && !user.blocked);

    // Повторне застосування схеми не падає.
    store.migrate().await.expect("повторна схема");

    let user = store.get_user(1).await.expect("get").expect("користувач є");
    assert_eq!(user.user_id, Some(11));
}

#[tokio::test]
async fn upsert_користувача_ідемпотентний() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    let mut u = new_user(100, Some(1000));
    u.username = Some("first".to_string());
    let first = store.upsert_user(u).await.expect("перший upsert");
    assert_eq!(first.username.as_deref(), Some("first"));
    assert_eq!(first.language, Language::Uk);

    let mut u2 = new_user(100, Some(1000));
    u2.language = Language::En;
    u2.username = Some("second".to_string());
    u2.role = Role::Admin;
    let second = store.upsert_user(u2).await.expect("другий upsert");
    assert_eq!(second.username.as_deref(), Some("second"));
    assert_eq!(second.language, Language::En);
    assert_eq!(second.role, Role::Admin);
    // Один запис, а не два.
    assert_eq!(store.list_users().await.expect("list").len(), 1);

    // Пошук за Telegram user id.
    let found = store
        .find_user_by_user_id(1000)
        .await
        .expect("find")
        .expect("знайдено");
    assert_eq!(found.chat_id, 100);
}

#[tokio::test]
async fn реєстрація_алерту_ідемпотентна() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    assert!(store.register_alert(&alert("a-1")).await.expect("перший"));
    assert!(!store.register_alert(&alert("a-1")).await.expect("другий"));
    assert!(store.alert_exists("a-1").await.expect("exists"));
    assert!(!store.alert_exists("a-2").await.expect("немає"));
}

#[tokio::test]
async fn claim_доставок_впорядковує_за_пріоритетом_і_не_повторюється() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

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

    // Повторний claim не повертає вже забірі справ.
    let again = store.claim_deliveries(5_000, 10).await.expect("повтор");
    assert!(again.is_empty());

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
async fn два_claim_паралельно_не_забирають_одні_й_ті_ж_доставки() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    for i in 0..20 {
        store
            .enqueue_delivery(NewDelivery {
                chat_id: 100 + i,
                alert_key: format!("k-{i}"),
                priority: Priority::Broadcast,
                text: "t".to_string(),
                created_at: 1_000,
            })
            .await
            .expect("enqueue");
    }

    // Два воркери забирають чергу одночасно — `SKIP LOCKED` не дає їм
    // забрати ті самі рядки.
    let (a, b) = tokio::join!(
        store.claim_deliveries(5_000, 10),
        store.claim_deliveries(5_000, 10)
    );
    let a = a.expect("claim a");
    let b = b.expect("claim b");
    let mut keys_a: Vec<String> = a.iter().map(|d| d.alert_key.clone()).collect();
    let mut keys_b: Vec<String> = b.iter().map(|d| d.alert_key.clone()).collect();
    keys_a.sort();
    keys_b.sort();
    assert_eq!(keys_a.len() + keys_b.len(), 20, "усі рядки розібрано");
    for k in &keys_a {
        assert!(!keys_b.contains(k), "рядок {k} не забрано двома воркерами");
    }
}

#[tokio::test]
async fn seen_event_і_seen_update_повідомляють_про_перший_раз() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    assert!(store.seen_event("evt-1", 1_000).await.expect("перший"));
    assert!(!store.seen_event("evt-1", 1_001).await.expect("другий"));
    assert!(store.seen_event("evt-2", 1_002).await.expect("інший"));
    assert!(store.seen_update(42).await.expect("перший update"));
    assert!(!store.seen_update(42).await.expect("другий update"));
}

#[tokio::test]
async fn purge_користувача_видаляє_підписки_доставки_й_аудит() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    store.upsert_user(new_user(700, Some(7000))).await.expect("upsert");
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

    assert!(store.get_user(700).await.expect("get").is_none());
    assert!(store.subscriptions(700).await.expect("subs").is_empty());
    assert!(store.user_assets(700).await.expect("assets").is_empty());
    assert_eq!(store.audit_count(&chat_ref).await.expect("audit_count"), 0);
}

#[tokio::test]
async fn заблоковані_і_приглушені_не_отримують_алерти() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    for chat_id in [10_i64, 11, 12] {
        store
            .upsert_user(new_user(chat_id, Some(chat_id)))
            .await
            .expect("upsert");
        store
            .set_subscription(&storage::Subscription {
                chat_id,
                topic: Topic::Whale,
                min_severity: "medium".to_string(),
            })
            .await
            .expect("subscription");
    }
    store.set_blocked(11, true).await.expect("block");
    store
        .set_settings(
            12,
            &Settings {
                language: Language::Uk,
                timezone: "UTC".to_string(),
                timeframe: Timeframe::H24,
                muted: true,
            },
        )
        .await
        .expect("settings");

    let recipients = store.recipients_for(Topic::Whale).await.expect("recipients");
    assert_eq!(recipients, vec![10]);
    assert_eq!(store.count_recipients().await.expect("count"), 1);
    assert_eq!(store.all_active_chats().await.expect("active"), vec![10]);
}

#[tokio::test]
async fn налаштування_курсор_снапшот_і_аудит_зберігаються() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    store.upsert_user(new_user(200, Some(2000))).await.expect("upsert");
    store
        .set_settings(
            200,
            &Settings {
                language: Language::En,
                timezone: "Europe/Kyiv".to_string(),
                timeframe: Timeframe::D7,
                muted: false,
            },
        )
        .await
        .expect("settings");
    let user = store.get_user(200).await.expect("get").expect("user");
    assert_eq!(user.language, Language::En);
    assert_eq!(user.timezone, "Europe/Kyiv");
    assert_eq!(user.timeframe, Timeframe::D7);

    store.set_last_seen(200, 1_234).await.expect("last_seen");
    let user = store.get_user(200).await.expect("get").expect("user");
    assert_eq!(user.last_seen_at, Some(1_234));

    assert!(store.cursor("telegram_offset").await.expect("cursor").is_none());
    store
        .set_cursor("telegram_offset", "42")
        .await
        .expect("set_cursor");
    assert_eq!(
        store.cursor("telegram_offset").await.expect("cursor"),
        Some("42".to_string())
    );

    store.put_snapshot(1_700, "{\"a\":1}").await.expect("snapshot");
    let (as_of, payload) = store.last_snapshot().await.expect("snapshot").expect("є");
    assert_eq!(as_of, 1_700);
    assert_eq!(payload, "{\"a\":1}");
}

#[tokio::test]
async fn alerts_since_повертає_лише_алерти_у_вікні_новіші_першими() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    let mut a1 = alert("s-1");
    a1.created_at = 1_000;
    let mut a2 = alert("s-2");
    a2.created_at = 2_000;
    let mut a3 = alert("s-3");
    a3.created_at = 3_000;
    for a in [&a1, &a2, &a3] {
        store.register_alert(a).await.expect("register");
    }

    let all = store.alerts_since(0).await.expect("alerts");
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].alert_key, "s-3");
    assert_eq!(all[2].alert_key, "s-1");

    let window = store.alerts_since(2_000).await.expect("alerts");
    assert_eq!(window.len(), 2);
    assert_eq!(window[0].alert_key, "s-3");
}

#[tokio::test]
async fn expire_pending_before_чіпає_лише_застряглі_доставки() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    store
        .enqueue_delivery(NewDelivery {
            chat_id: 1,
            alert_key: "old".to_string(),
            priority: Priority::Broadcast,
            text: "t".to_string(),
            created_at: 1_000,
        })
        .await
        .expect("enqueue");
    store
        .enqueue_delivery(NewDelivery {
            chat_id: 2,
            alert_key: "fresh".to_string(),
            priority: Priority::Broadcast,
            text: "t".to_string(),
            created_at: 9_000,
        })
        .await
        .expect("enqueue");

    let n = store
        .expire_pending_before(5_000, "застрягло")
        .await
        .expect("expire");
    assert_eq!(n, 1);
    assert_eq!(store.count_pending().await.expect("count"), 1);
}

#[tokio::test]
async fn буфер_згортання_не_дублює_і_видаляється_за_ключами() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    let p = storage::FoldPending {
        alert_key: "f-1".to_string(),
        kind: "whale".to_string(),
        win: "15m".to_string(),
        asset: Some("BTC".to_string()),
        payload: "{}".to_string(),
        created_at: 1_000,
    };
    assert!(store.fold_pending_insert(&p).await.expect("insert"));
    assert!(!store.fold_pending_insert(&p).await.expect("повтор"));

    let list = store.fold_pending_list().await.expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].win, "15m");

    let removed = store
        .fold_pending_remove(&["f-1".to_string(), "нет".to_string()])
        .await
        .expect("remove");
    assert_eq!(removed, 1);
    assert!(store.fold_pending_list().await.expect("list").is_empty());
}

#[tokio::test]
async fn expire_stuck_sending_before_повертає_застряглі_доставки_в_чергу() {
    let Some((store, _guard)) = test_store().await else {
        return;
    };

    let now = storage::now_ms();
    store
        .enqueue_delivery(NewDelivery {
            chat_id: 500,
            alert_key: "stuck".to_string(),
            priority: Priority::Reply,
            text: "t".to_string(),
            created_at: now,
        })
        .await
        .expect("enqueue");

    // Воркер забрав доставку і... упав, не зафіксувавши результат.
    let claimed = store.claim_deliveries(now, 10).await.expect("claim");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].status, DeliveryStatus::Sending);

    // Оренда ще не минула — reaper не чіпає доставку.
    let early = store
        .expire_stuck_sending_before(now + 1_000, "воркер не зафіксував")
        .await
        .expect("reaper");
    assert_eq!(early, 0);
    assert_eq!(store.count_pending().await.expect("pending"), 0);

    // Оренда минула — доставку повернуто в чергу.
    let requeued = store
        .expire_stuck_sending_before(now + 600_000, "воркер не зафіксував")
        .await
        .expect("reaper");
    assert_eq!(requeued, 1);
    assert_eq!(store.count_pending().await.expect("pending"), 1);

    // Воркер забере її знову.
    let again = store
        .claim_deliveries(now + 600_001, 10)
        .await
        .expect("повторний claim");
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].alert_key, "stuck");
    assert_eq!(again[0].attempts, 1, "втрачена спроба врахована");
}
