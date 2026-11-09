//! Тести буфера розсилки: `max_backlog_minutes`.
//!
//! Механіка (див. `docs/ARCHITECTURE.md` §8.3):
//!
//! 1. **Поки джерело недоступне — черга буферизується**: доставки не
//!    відправляються, а накопичуються.
//! 2. **Межа буфера**: доставка, що провисела в черзі довше за
//!    `max_backlog_minutes`, скидається і **не надсилається ніколи** —
//!    вікно агрегації застаріло, а «кит накопичував 2 години тому» — шум.
//! 3. `max_backlog_minutes = 0` — обмеження вимкнено (буфер безстроковий).
//!
//! Перевіряємо саме це — і те, що межа не ламає звичайну доставку.

use alert_engine::{Alert, AlertKey, AlertKind, AssetLabel, Direction, Metrics, Severity, Window};
use broadcast::{BroadcastConfig, Broadcaster, Notifier, Outbound};
use privacy_filter::PrivacyFilter;
use std::sync::atomic::Ordering;
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

/// Алерт із заданим часом створення (щоб імітувати вік).
fn alert_at(kind: AlertKind, symbol: &str, key: &str, created_at: i64) -> Alert {
    Alert {
        key: AlertKey::derive(kind, symbol, 0, &[key]),
        kind,
        severity: Severity::Medium,
        asset: AssetLabel::unknown(symbol),
        related: Vec::new(),
        direction: Some(Direction::Accumulation),
        metrics: Metrics {
            participants: 5,
            total_value_usd: 100_000.0,
            group_share_pct: None,
            concentration_top10_pct: None,
            group_size: None,
            window: Window::W1h,
        },
        archetype: None,
        degraded: false,
        created_at,
    }
}

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

/// Розсильник із заданою межею буфера.
fn broadcaster(
    store: SqliteStore,
    sent: Arc<Mutex<Vec<String>>>,
    max_backlog_minutes: u32,
) -> Arc<Broadcaster<SqliteStore, RecordingNotifier>> {
    let cfg = BroadcastConfig {
        per_chat_min_interval_ms: 0,
        max_backlog_minutes,
        ..BroadcastConfig::default()
    };
    Arc::new(Broadcaster::new(
        store,
        RecordingNotifier { sent },
        PrivacyFilter::new("приховано"),
        cfg,
    ))
}

fn sent_count(sent: &Arc<Mutex<Vec<String>>>) -> usize {
    sent.lock().unwrap_or_else(|e| e.into_inner()).len()
}

#[tokio::test]
async fn черга_буферизується_поки_джерело_недоступне() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 30);

    b.publish(&alert_at(
        AlertKind::WhaleAccumulation,
        "SOL",
        "e0",
        now_ms(),
    ))
    .await
    .expect("publish");

    // Джерело недоступне — нічого не надсилається, доставка чекає.
    b.source_gate().store(false, Ordering::Relaxed);
    let sent_now = b.pump_once(now_ms()).await.expect("pump");
    assert_eq!(sent_now, 0, "поки джерело недоступне, нічого не йде");
    assert_eq!(sent_count(&sent), 0);
    assert_eq!(
        b.backlog_pending().await.expect("pending"),
        1,
        "буфер не пустий"
    );

    // Джерело ожило — буфер виливається.
    b.source_gate().store(true, Ordering::Relaxed);
    let sent_after = b.pump_once(now_ms()).await.expect("pump");
    assert_eq!(sent_after, 1, "після відновлення буфер вилився");
    assert_eq!(sent_count(&sent), 1);
    assert_eq!(b.backlog_pending().await.expect("pending"), 0);
}

#[tokio::test]
async fn прострочені_доставки_скидаються_і_не_надсилаються() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 30);

    // Алерт 40 хвилин тому: стільки він лежав би в буфері.
    let old = now_ms() - 40 * 60_000;
    b.publish(&alert_at(AlertKind::WhaleAccumulation, "SOL", "e0", old))
        .await
        .expect("publish");

    // Межа — 30 хв, тож алерт застарів.
    let expired = b.expire_backlog(now_ms()).await.expect("expire");
    assert_eq!(expired, 1, "прострочену доставку має бути скинуто");

    // І після відновлення джерела вона **не** надсилається.
    b.source_gate().store(false, Ordering::Relaxed);
    b.source_gate().store(true, Ordering::Relaxed);
    let pumped = b.pump_once(now_ms()).await.expect("pump");
    assert_eq!(pumped, 0, "скинута доставка не відправляється");
    assert_eq!(sent_count(&sent), 0, "користувач не мав нічого отримати");
    assert_eq!(b.backlog_pending().await.expect("pending"), 0);
    assert_eq!(b.stats().messages_expired, 1);
    assert_eq!(b.stats().messages_sent, 0);
}

#[tokio::test]
async fn свіжі_доставки_не_скидаються() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 30);

    b.publish(&alert_at(
        AlertKind::WhaleAccumulation,
        "SOL",
        "e0",
        now_ms(),
    ))
    .await
    .expect("publish");

    let expired = b.expire_backlog(now_ms()).await.expect("expire");
    assert_eq!(expired, 0, "свіжий алерт скидати не можна");

    let pumped = b.pump_once(now_ms()).await.expect("pump");
    assert_eq!(pumped, 1);
    assert_eq!(sent_count(&sent), 1);
    assert_eq!(b.stats().messages_expired, 0);
}

#[tokio::test]
async fn межа_0_вимкнена_буфер_безстроковий() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 0);

    // Навіть дуже старий алерт має дійти: межа вимкнена.
    let old = now_ms() - 6 * 60 * 60_000;
    b.publish(&alert_at(AlertKind::WhaleAccumulation, "SOL", "e0", old))
        .await
        .expect("publish");

    let expired = b.expire_backlog(now_ms()).await.expect("expire");
    assert_eq!(expired, 0, "межа 0 — нічого не скидаємо");

    let pumped = b.pump_once(now_ms()).await.expect("pump");
    assert_eq!(pumped, 1, "буфер безстроковий — доставка доходить");
    assert_eq!(sent_count(&sent), 1);
}

#[tokio::test]
async fn скидання_не_чіпає_вже_надіслані() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 30);

    let old = now_ms() - 40 * 60_000;
    b.publish(&alert_at(AlertKind::WhaleAccumulation, "SOL", "e0", old))
        .await
        .expect("publish");
    b.pump_once(now_ms()).await.expect("pump");
    assert_eq!(sent_count(&sent), 1);

    // Наступна перевірка межі не повинна переписати завершену доставку.
    let expired = b.expire_backlog(now_ms()).await.expect("expire");
    assert_eq!(expired, 0, "надіслана доставка — не в буфері");
    assert_eq!(b.stats().messages_sent, 1);
    assert_eq!(b.stats().messages_expired, 0);
}

#[tokio::test]
async fn буфер_переживає_перезапуск_процесу() {
    // Буфер живе в БД: рестарт (через конфлікт поллерів, рестарт systemd)
    // не має втратити ані накопичені доставки, ані правило межі.
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 30);

    let old = now_ms() - 40 * 60_000;
    b.publish(&alert_at(AlertKind::WhaleAccumulation, "SOL", "e0", old))
        .await
        .expect("publish");

    // «Перезапуск»: новий розсильник на тій самій БД.
    let sent2 = Arc::new(Mutex::new(Vec::new()));
    let b2 = broadcaster(store.clone(), sent2.clone(), 30);
    let expired = b2.expire_backlog(now_ms()).await.expect("expire");
    assert_eq!(expired, 1, "правило межі діє й після перезапуску");
    b2.pump_once(now_ms()).await.expect("pump");
    assert_eq!(sent_count(&sent2), 0, "прострочене не надсилається");
}

#[tokio::test]
async fn зведення_не_зливаються_поки_джерело_недоступне() {
    // Буфер згортання теж має пережити збій джерела: поки воно недоступне,
    // зведення не зливаємо — інакше вікно згортання «з’їдалося» б у буфері.
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;
    user_with_topic(&store, 2, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));

    // Згортання увімкнено: два одержувачі — поріг 25% від двох активних.
    let cfg = BroadcastConfig {
        per_chat_min_interval_ms: 0,
        digest_threshold_pct: 25,
        digest_window_secs: 300,
        max_backlog_minutes: 30,
        ..BroadcastConfig::default()
    };
    let b = Arc::new(Broadcaster::new(
        store.clone(),
        RecordingNotifier { sent: sent.clone() },
        PrivacyFilter::new("приховано"),
        cfg,
    ));

    b.publish(&alert_at(
        AlertKind::WhaleAccumulation,
        "SOL",
        "e0",
        now_ms(),
    ))
    .await
    .expect("publish");
    assert_eq!(store.fold_pending_list().await.expect("fold").len(), 1);

    b.source_gate().store(false, Ordering::Relaxed);
    let queued = b.flush_digests(now_ms() + 301_000).await.expect("flush");
    assert_eq!(queued, 0, "поки джерело недоступне, зведення не зливаємо");
    assert_eq!(
        store.fold_pending_list().await.expect("fold").len(),
        1,
        "буфер згортання не спорожнівся"
    );

    // Джерело ожило — зведення зливається.
    b.source_gate().store(true, Ordering::Relaxed);
    let queued = b.flush_digests(now_ms() + 301_000).await.expect("flush");
    assert_eq!(queued, 2, "по одному зведенню на одержувача");
}

#[tokio::test]
async fn буфер_не_зʼїдає_пріоритетів_і_лишається_в_черзі() {
    // Доставка, що чекає в буфері, лишається в черзі з пріоритетом
    // звичайної розсилки — скидання її не плутає з дайджестами.
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone(), 30);

    b.publish(&alert_at(
        AlertKind::WhaleAccumulation,
        "SOL",
        "e0",
        now_ms(),
    ))
    .await
    .expect("publish");

    let rows = store.claim_deliveries(now_ms(), 10).await.expect("claim");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].priority, Priority::Broadcast);
    assert_eq!(rows[0].status.as_str(), "sending");
}
