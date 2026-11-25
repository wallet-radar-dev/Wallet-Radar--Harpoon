//! Тести reaper'а: доставки, що застрягли у статусі `sending`.
//!
//! Воркер переводить доставку в `sending` і фіксує результат
//! (`finish_delivery`). Якщо процес упаде між цими кроками, рядок лишається
//! `sending` назавжди — алерт не піде ніколи. Reaper повертає такі рядки в
//! чергу, щоб воркер забрав їх знову.

use alert_engine::{Alert, AlertKey, AlertKind, AssetLabel, Direction, Metrics, Severity, Window};
use broadcast::{BroadcastConfig, Broadcaster, Notifier, Outbound};
use privacy_filter::PrivacyFilter;
use std::sync::{Arc, Mutex};
use storage::{Language, NewUser, Role, SqliteStore, Store, Topic};

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

fn broadcaster(
    store: SqliteStore,
    sent: Arc<Mutex<Vec<String>>>,
) -> Arc<Broadcaster<SqliteStore, RecordingNotifier>> {
    let cfg = BroadcastConfig {
        per_chat_min_interval_ms: 0,
        max_backlog_minutes: 30,
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
async fn застряглі_доставки_повертаються_в_чергу_і_доходять() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 1, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone());

    b.publish(&alert_at(
        AlertKind::WhaleAccumulation,
        "SOL",
        "e0",
        now_ms(),
    ))
    .await
    .expect("publish");

    // Імітуємо воркера, який упав після claim, але до finish.
    let now = now_ms();
    let claimed = store.claim_deliveries(now, 10).await.expect("claim");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        store.count_pending().await.expect("pending"),
        0,
        "доставка в роботі, не в черзі"
    );

    // Оренда ще не минула — нічого не повертаємо.
    let early = b.requeue_stuck(now + 1_000).await.expect("requeue");
    assert_eq!(early, 0, "оренда не минула — доставку не чіпаємо");

    // Оренда минула — доставку повернуто в чергу.
    let requeued = b
        .requeue_stuck(now + 600_000)
        .await
        .expect("requeue");
    assert_eq!(requeued, 1, "застряглу доставку повернуто");
    assert_eq!(b.stats().messages_requeued, 1);
    assert_eq!(
        store.count_pending().await.expect("pending"),
        1,
        "доставка знову в черзі"
    );

    // Воркер забере її і надішле.
    let pumped = b.pump_once(now + 600_001).await.expect("pump");
    assert_eq!(pumped, 1);
    assert_eq!(sent_count(&sent), 1, "доставка дійшла");
}

#[tokio::test]
async fn reaper_не_чіпає_доставки_під_час_нормальної_роботи() {
    let store = SqliteStore::open_in_memory().expect("БД");
    user_with_topic(&store, 2, Topic::Whale).await;

    let sent = Arc::new(Mutex::new(Vec::new()));
    let b = broadcaster(store.clone(), sent.clone());

    b.publish(&alert_at(
        AlertKind::WhaleAccumulation,
        "ETH",
        "e1",
        now_ms(),
    ))
    .await
    .expect("publish");

    // Нічого не застрягло — reaper мовчить.
    let requeued = b.requeue_stuck(now_ms()).await.expect("requeue");
    assert_eq!(requeued, 0);
    assert_eq!(b.stats().messages_requeued, 0);

    // І нормальна доставка проходить без втручання reaper'а.
    let pumped = b.pump_once(now_ms()).await.expect("pump");
    assert_eq!(pumped, 1);
    assert_eq!(sent_count(&sent), 1);
    assert_eq!(b.stats().messages_requeued, 0);
}
