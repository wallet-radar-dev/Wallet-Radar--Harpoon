//! Тести згортання «широких» сигналів у зведений дайджест.
//!
//! Механіка: якщо сигнал долає до великої частки активних користувачів
//! (`digest_threshold_pct`), він не надсилається окремим повідомленням
//! кожному, а згортається — за вікно (`digest_window_secs`) підписники
//! отримують одне зведене повідомлення.
//!
//! Перевіряємо:
//!
//! 1. **Широкий сигнал згортається** — доставки не створюються одразу,
//!    а після злиття вікна — одна на одержувача.
//! 2. **Вузький сигнал лишається детальним** і йде негайно.
//! 3. **Сигнал одному одержувачу не згортається** ніколи.
//! 4. **Поріг 0** згортає все, що має хоча б двох одержувачів.
//! 5. **Зведення не містить адрес** — навіть якщо payload підмінити.

use alert_engine::{Alert, AlertKey, AlertKind, AssetLabel, Direction, Metrics, Severity, Window};
use broadcast::{BroadcastConfig, Broadcaster, Notifier, Outbound};
use privacy_filter::PrivacyFilter;
use std::sync::{Arc, Mutex};
use storage::{Language, NewUser, Priority, Role, SqliteStore, Store, Topic};

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

fn alert(kind: AlertKind, symbol: &str, key: &str, value: f64) -> Alert {
    Alert {
        key: AlertKey::derive(kind, symbol, 0, &[key]),
        kind,
        severity: Severity::Medium,
        asset: AssetLabel::unknown(symbol),
        related: Vec::new(),
        direction: Some(Direction::Accumulation),
        metrics: Metrics {
            participants: 5,
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

/// Користувач із підпискою на тему.
async fn user_with_topic(store: &SqliteStore, chat_id: i64, topic: Topic) {
    store
        .upsert_user(NewUser {
            chat_id,
            user_id: Some(chat_id),
            username: Some(format!("u{chat_id}")),
            language: Language::Uk,
            timezone: "UTC".to_string(),
            role: Role::Viewer,
        })
        .await
        .expect("користувач");
    store
        .set_subscription(&storage::Subscription {
            chat_id,
            topic,
            min_severity: "info".to_string(),
        })
        .await
        .expect("підписка");
}

/// Користувач без підписокки на жодну тему.
async fn plain_user(store: &SqliteStore, chat_id: i64) {
    store
        .upsert_user(NewUser {
            chat_id,
            user_id: Some(chat_id),
            username: Some(format!("u{chat_id}")),
            language: Language::Uk,
            timezone: "UTC".to_string(),
            role: Role::Viewer,
        })
        .await
        .expect("користувач");
}

fn broadcaster(
    store: SqliteStore,
    sent: Arc<Mutex<Vec<String>>>,
    threshold_pct: u32,
    window_secs: u64,
) -> Arc<Broadcaster<SqliteStore, RecordingNotifier>> {
    let cfg = BroadcastConfig {
        per_chat_min_interval_ms: 0,
        digest_threshold_pct: threshold_pct,
        digest_window_secs: window_secs,
        ..BroadcastConfig::default()
    };
    Arc::new(Broadcaster::new(
        store,
        RecordingNotifier { sent },
        PrivacyFilter::new("приховано"),
        cfg,
    ))
}

#[tokio::test]
async fn широкий_сигнал_згортається_і_йде_одним_повідомленням_за_вікно() {
    let store = SqliteStore::open_in_memory().expect("БД");
    // Три користувачі, усі підписані на «китів».
    user_with_topic(&store, 1, Topic::Whale).await;
    user_with_topic(&store, 2, Topic::Whale).await;
    user_with_topic(&store, 3, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 25, 300);

    // Три алерти по SOL — вони мають згорнутися в одне зведення.
    for i in 0..3 {
        let outcome = b
            .publish(&alert(
                AlertKind::WhaleAccumulation,
                "SOL",
                &format!("e{i}"),
                100_000.0,
            ))
            .await
            .expect("publish");
        assert!(
            matches!(outcome, broadcast::PublishOutcome::Folded { recipients: 3 }),
            "сигнал мав згорнутися: {outcome:?}"
        );
    }

    // До вікна нічого не надіслано.
    assert!(sent.lock().unwrap_or_else(|e| e.into_inner()).is_empty());

    // Злиття після вікна — і в черга, і в надсилач.
    let queued = b.flush_digests(now_ms() + 301_000).await.expect("flush");
    assert_eq!(queued, 3, "по одному зведенню на одержувача");
    b.pump_once(now_ms() + 301_000).await.expect("pump");

    let messages = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(messages.len(), 3, "по одному повідомленню на одержувача");
    for m in &messages {
        assert!(
            m.contains("3 сигнали"),
            "у зведенні має бути кількість: {m}"
        );
        assert!(m.contains("SOL"), "{m}");
        assert!(!m.contains("0x"), "у зведенні не має бути адрес: {m}");
    }
}

#[tokio::test]
async fn різні_активи_одного_виду_згортаються_в_одне_повідомлення() {
    // Це той самий сценарій, що дав 22 окремі алерти на старті:
    // двадцять різних активів одного виду мають дати ОДНЕ зведення,
    // а не двадцять повідомлень.
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::CrossAsset).await;
    user_with_topic(&store, 2, Topic::CrossAsset).await;
    user_with_topic(&store, 3, Topic::CrossAsset).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 25, 300);

    let symbols = ["OKB", "ARB", "BICO", "RLUSD", "LIT", "APE", "MANA", "CVX"];
    for (i, s) in symbols.iter().enumerate() {
        let outcome = b
            .publish(&alert(
                AlertKind::CrossAssetAnomaly,
                s,
                &format!("e{i}"),
                100_000.0,
            ))
            .await
            .expect("publish");
        assert!(
            matches!(outcome, broadcast::PublishOutcome::Folded { recipients: 3 }),
            "сигнал мав згорнутися: {outcome:?}"
        );
    }

    let queued = b.flush_digests(now_ms() + 301_000).await.expect("flush");
    assert_eq!(
        queued, 3,
        "по одному зведенню на одержувача, а не по одному на актив"
    );
    b.pump_once(now_ms() + 301_000).await.expect("pump");

    let messages = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(
        messages.len(),
        3,
        "три повідомлення замість двадцяти чотирьох"
    );
    for m in &messages {
        assert!(m.contains("8 сигналів"), "{m}");
        assert!(m.contains("активи:"), "має бути перелік активів: {m}");
        assert!(m.contains("OKB") && m.contains("ARB"), "{m}");
        assert!(!m.contains("0x"), "{m}");
    }
}

#[tokio::test]
async fn вузький_сигнал_лишається_детальним_і_йде_негайно() {
    let store = SqliteStore::open_in_memory().expect("БД");
    // Три користувачі, але підписка на «китів» лише в одного.
    user_with_topic(&store, 1, Topic::Whale).await;
    plain_user(&store, 2).await;
    plain_user(&store, 3).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 25, 300);

    let outcome = b
        .publish(&alert(AlertKind::WhaleAccumulation, "SOL", "e0", 100_000.0))
        .await
        .expect("publish");
    assert!(
        matches!(outcome, broadcast::PublishOutcome::Queued { recipients: 1 }),
        "вузький сигнал мав піти одразу: {outcome:?}"
    );

    b.pump_once(now_ms()).await.expect("pump");
    let messages = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(messages.len(), 1);
    // Детальний алерт, а не зведення.
    assert!(!messages[0].contains("Зведено"), "{}", messages[0]);
}

#[tokio::test]
async fn сигнал_одному_одержувачу_не_згортається_навіть_за_порога_0() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;
    plain_user(&store, 2).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    // Поріг 0 = згортати все.
    let b = broadcaster(store.clone(), sent.clone(), 0, 300);

    let outcome = b
        .publish(&alert(AlertKind::WhaleAccumulation, "SOL", "e0", 100_000.0))
        .await
        .expect("publish");
    assert!(
        matches!(outcome, broadcast::PublishOutcome::Queued { recipients: 1 }),
        "персональний сигнал не згортається: {outcome:?}"
    );
}

#[tokio::test]
async fn поріг_0_згортає_все_що_має_двох_одержувачів() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;
    user_with_topic(&store, 2, Topic::Whale).await;
    plain_user(&store, 3).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 0, 300);

    let outcome = b
        .publish(&alert(AlertKind::WhaleAccumulation, "SOL", "e0", 100_000.0))
        .await
        .expect("publish");
    assert!(
        matches!(outcome, broadcast::PublishOutcome::Folded { recipients: 2 }),
        "поріг 0 має згортати все: {outcome:?}"
    );
}

#[tokio::test]
async fn зведення_не_містить_адрес_навіть_зі_підмішаним_payload() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;
    user_with_topic(&store, 2, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 25, 300);

    // Публікуємо алерт, у payload якого підмішано адресу.
    let mut a = alert(AlertKind::WhaleAccumulation, "SOL", "e0", 100_000.0);
    a.archetype = Some("0x611f7bf868a6212f871e89f7e44684045ddfb09d".to_string());
    b.publish(&a).await.expect("publish");

    b.flush_digests(now_ms() + 301_000).await.expect("flush");
    b.pump_once(now_ms() + 301_000).await.expect("pump");

    let messages = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(messages.len(), 2);
    for m in &messages {
        assert!(!m.contains("0x"), "у зведенні є hex-рядок:\n{m}");
        assert!(!m.contains("611f7bf8"), "у зведенні є адреса:\n{m}");
    }
}

#[tokio::test]
async fn зведення_має_пріоритет_дайджесту_і_йде_перед_звичайними_алертами() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;
    user_with_topic(&store, 2, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 25, 300);

    b.publish(&alert(AlertKind::WhaleAccumulation, "SOL", "e0", 100_000.0))
        .await
        .expect("publish");
    b.flush_digests(now_ms() + 301_000).await.expect("flush");

    // Доставка зведення має пріоритет Digest, а не Broadcast.
    let rows = store
        .claim_deliveries(now_ms() + 301_000, 10)
        .await
        .expect("claim");
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert_eq!(r.priority, Priority::Digest);
    }
}

#[tokio::test]
async fn буфер_згортання_переживає_перезапуск_процесу() {
    // Буфер згортання живе в БД, а не в памʼяті: перезапуск (через
    // конфлікт поллерів, рестарт systemd, оновлення) не має втратити
    // згорнуті алерти — вони просто зникли б із ротації.
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;
    user_with_topic(&store, 2, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 25, 300);

    b.publish(&alert(AlertKind::WhaleAccumulation, "SOL", "e0", 100_000.0))
        .await
        .expect("publish");

    // «Перезапуск»: новий розсильник на тій самій БД.
    let sent2 = Arc::new(Mutex::new(Vec::new()));
    let b2 = broadcaster(store.clone(), sent2.clone(), 25, 300);

    let queued = b2.flush_digests(now_ms() + 301_000).await.expect("flush");
    assert_eq!(queued, 2, "згорнутий алерт пережив перезапуск");
    b2.pump_once(now_ms() + 301_000).await.expect("pump");

    let messages = sent2.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(messages.len(), 2);
    for m in &messages {
        assert!(m.contains("SOL"), "{m}");
    }
}
