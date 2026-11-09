//! Розсилка алертів і приватних відповідей.
//!
//! Три гарантії:
//!
//! * **Ідемпотентність.** Алерт реєструється в `alert_log` (UNIQUE `alert_key`)
//!   до того, як хтось його отримає. Повторний прогін того самого доказу не
//!   створює другого запису.
//! * **Одна доставка на користувача.** `deliveries` має PK `(chat_id, alert_key)`,
//!   тому повтор — це оновлення `attempts`, а не новий рядок.
//! * **Жодна відповідь не чекає на розсилку.** Пріоритети: відповідь (0) → дайджест
//!   (1) → розсилка (2). Відповідь йде напряму, минаючи чергу розсилки.
//!
//! Швидкість відповіді — не деталь: користувач надіслав питання й чекає на нього.
//! Розсилка на 3000 користувачів — фонова робота, вона не має права заважати.

use alert_engine::{Alert, AlertKind, FoldedSignal};
use privacy_filter::PrivacyFilter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use storage::{
    AlertRecord, Delivery, DeliveryStatus, DeliveryUpdate, NewDelivery, Priority, Store,
};
use thiserror::Error;

/// Повідомлення, готове до відправки.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outbound {
    pub text: String,
    pub reply_to: Option<i64>,
}

/// Помилка відправки — з класифікацією, бо від неї залежить ретрай.
#[derive(Debug, Error)]
pub enum SendError {
    /// Telegram каже «зачекай N секунд». Не дропаємо — відкладаємо.
    #[error("telegram rate limit: чекати {retry_after} с")]
    RateLimited { retry_after: u64 },
    /// Користувач заблокував бота або чату не існує — ретрай безглуздий.
    #[error("постійна помилка telegram: {0}")]
    Permanent(String),
    /// Тимчасова помилка — ретраїмо з backoff.
    #[error("тимчасова помилка telegram: {0}")]
    Transient(String),
}

/// Канал відправки.
pub trait Notifier {
    fn send(&self, chat_id: i64, msg: &Outbound) -> impl Future<Output = Result<(), SendError>>;
}

/// Налаштування розсилки.
#[derive(Clone, Debug)]
pub struct BroadcastConfig {
    pub global_per_sec: u32,
    pub per_chat_min_interval_ms: u64,
    pub max_pending_per_chat: u32,
    pub max_attempts: u32,
    pub backoff_base_ms: u64,
    pub backoff_cap_ms: u64,
    /// Інтервал опитування черги.
    pub poll_interval_ms: u64,
    /// Поріг «широкого» сигналу: частка активних користувачів (у відсотках),
    /// яка має отримати той самий сигнал, щоб він згорнувся у зведений дайджест.
    ///
    /// `0` — згортати все. Більше — згортаються лише справді спільні сигнали,
    /// а вузькі (персональні) лишаються детальними й негайними.
    pub digest_threshold_pct: u32,
    /// Вікно згортання: раз на скільки секунд збирається один зведений дайджест.
    pub digest_window_secs: u64,
    /// Межа буфера: скільки хвилин доставка може чекати в черзі.
    ///
    /// `0` — обмеження вимкнено (буфер безстроковий). Інакше доставка, яка
    /// провисела в черзі довше за цю межу, скидається і **не надсилається
    /// ніколи**: вікно агрегації застаріло, а «кит накопичував 2 години
    /// тому» — це не інформація, а шум.
    pub max_backlog_minutes: u32,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            // Ліміт Telegram — 30/с глобально; беремо 25.
            global_per_sec: 25,
            // Ліміт Telegram — 1/с на чат; беремо 1.05 с.
            per_chat_min_interval_ms: 1050,
            max_pending_per_chat: 20,
            max_attempts: 6,
            backoff_base_ms: 2_000,
            backoff_cap_ms: 300_000,
            poll_interval_ms: 500,
            digest_threshold_pct: 25,
            digest_window_secs: 300,
            max_backlog_minutes: 30,
        }
    }
}

/// Лімітер відправки: глобальний token bucket + мініманий інтервал на чат.
pub struct RateLimiter {
    cfg: BroadcastConfig,
    state: Mutex<LimiterState>,
}

struct LimiterState {
    tokens: f64,
    last_refill: Instant,
    last_chat_send: HashMap<i64, Instant>,
}

impl RateLimiter {
    pub fn new(cfg: BroadcastConfig) -> RateLimiter {
        RateLimiter {
            state: Mutex::new(LimiterState {
                tokens: cfg.global_per_sec as f64,
                last_refill: Instant::now(),
                last_chat_send: HashMap::new(),
            }),
            cfg,
        }
    }

    /// Зачекати, поки можна надсилати в цей чат.
    pub async fn acquire(&self, chat_id: i64) {
        loop {
            let wait = {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();

                // Глобальний бакет.
                let elapsed = now.duration_since(st.last_refill).as_secs_f64();
                let per_sec = self.cfg.global_per_sec as f64;
                st.tokens = (st.tokens + elapsed * per_sec).min(per_sec);
                st.last_refill = now;

                let chat_wait = st
                    .last_chat_send
                    .get(&chat_id)
                    .map(|t| {
                        // Скільки ще чекати до наступного дозволеного надсилання.
                        // Важливо: саме `min - elapsed`, а не `elapsed - min` —
                        // інакше при `min = 0` очікування ніколи не стає нульовим
                        // (вічный цикл), а при великому `elapsed` пауза зникала б
                        // й ліміт «1 повідомлення на чат» не працював би.
                        let min = Duration::from_millis(self.cfg.per_chat_min_interval_ms);
                        let elapsed = now.saturating_duration_since(*t);
                        min.saturating_sub(elapsed)
                    })
                    .unwrap_or_default();

                if st.tokens >= 1.0 && chat_wait.is_zero() {
                    st.tokens -= 1.0;
                    st.last_chat_send.insert(chat_id, now);
                    // Не даємо мапі рости безмежно.
                    if st.last_chat_send.len() > 10_000 {
                        let cutoff = now - Duration::from_secs(3600);
                        st.last_chat_send.retain(|_, t| *t > cutoff);
                    }
                    None
                } else {
                    let global_wait = if st.tokens >= 1.0 {
                        Duration::ZERO
                    } else {
                        Duration::from_secs_f64((1.0 - st.tokens) / per_sec.max(1.0))
                    };
                    Some(std::cmp::max(global_wait, chat_wait))
                }
            };
            match wait {
                None => return,
                Some(d) => {
                    if d.is_zero() {
                        tokio::task::yield_now().await;
                    } else {
                        tokio::time::sleep(d).await;
                    }
                }
            }
        }
    }
}

/// Підсумок публікації.
#[derive(Clone, Debug, PartialEq)]
pub enum PublishOutcome {
    /// Алерт уже був зареєстрований — нічого не дублюємо.
    AlreadyKnown,
    /// Зареєстровано й поставлено в чергу.
    Queued { recipients: usize },
    /// Зареєстровано, але сигнал згорнуто у зведений дайджест: підписники
    /// отримають одне повідомлення за вікно замість окремих на кожен алерт.
    Folded { recipients: usize },
}

/// Помилки розсилки.
#[derive(Debug, Error)]
pub enum BroadcastError {
    #[error("помилка сховища: {0}")]
    Storage(#[from] storage::StoreError),
}

/// Лічильники для `/stats`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BroadcastStats {
    pub alerts_published: u64,
    pub alerts_duplicate: u64,
    pub alerts_folded: u64,
    pub digests_sent: u64,
    pub messages_sent: u64,
    pub messages_failed: u64,
    pub messages_retried: u64,
    pub messages_skipped: u64,
    /// Скільки доставок скинуто за межею буфера (алерт застарів).
    pub messages_expired: u64,
}

/// Згорнуті сигнали, що очікують зведення.
///
/// Буфер живе у БД (`fold_pending`), а не в памʼяті: перезапуск процесу
/// не має втрачати згорнуті алерти.
pub struct Broadcaster<S: Store, N: Notifier> {
    store: S,
    notifier: N,
    sanitizer: PrivacyFilter,
    limiter: Arc<RateLimiter>,
    cfg: BroadcastConfig,
    stats: BroadcastStatsAtomic,
    /// «Джерело доступне». Коли вимкнено — черга **буферизується**: нічого не
    /// надсилається, поки джерело не оживе (див. `docs/ARCHITECTURE.md` §8.3).
    source_gate: Arc<AtomicBool>,
}

/// Атомарні лічильники (щоб `Arc<Broadcaster>` ділився ними з воркером).
#[derive(Default)]
pub struct BroadcastStatsAtomic {
    pub alerts_published: AtomicU64,
    pub alerts_duplicate: AtomicU64,
    pub alerts_folded: AtomicU64,
    pub digests_sent: AtomicU64,
    pub messages_sent: AtomicU64,
    pub messages_failed: AtomicU64,
    pub messages_retried: AtomicU64,
    pub messages_skipped: AtomicU64,
    pub messages_expired: AtomicU64,
}

impl BroadcastStatsAtomic {
    pub fn snapshot(&self) -> BroadcastStats {
        BroadcastStats {
            alerts_published: self.alerts_published.load(Ordering::Relaxed),
            alerts_duplicate: self.alerts_duplicate.load(Ordering::Relaxed),
            alerts_folded: self.alerts_folded.load(Ordering::Relaxed),
            digests_sent: self.digests_sent.load(Ordering::Relaxed),
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            messages_failed: self.messages_failed.load(Ordering::Relaxed),
            messages_retried: self.messages_retried.load(Ordering::Relaxed),
            messages_skipped: self.messages_skipped.load(Ordering::Relaxed),
            messages_expired: self.messages_expired.load(Ordering::Relaxed),
        }
    }
}

impl<S: Store, N: Notifier> Broadcaster<S, N> {
    pub fn new(store: S, notifier: N, sanitizer: PrivacyFilter, cfg: BroadcastConfig) -> Self {
        let limiter = Arc::new(RateLimiter::new(cfg.clone()));
        Self {
            store,
            notifier,
            sanitizer,
            limiter,
            cfg,
            stats: BroadcastStatsAtomic::default(),
            source_gate: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Ручка стану джерела: `false` — джерело недоступне, черга буферизується.
    ///
    /// Клонується `Arc`, тож оркестратор (цикл зрізів) керує тим самим
    /// станом, що й воркер розсилки.
    pub fn source_gate(&self) -> Arc<AtomicBool> {
        self.source_gate.clone()
    }

    /// Чи джерело зараз доступне (черга не буферизується).
    pub fn source_available(&self) -> bool {
        self.source_gate.load(Ordering::Relaxed)
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn stats(&self) -> BroadcastStats {
        self.stats.snapshot()
    }

    /// Зареєструвати алерт і поставити в чергу всім підходящим підписникам.
    pub async fn publish(&self, alert: &Alert) -> Result<PublishOutcome, BroadcastError> {
        let record = AlertRecord {
            alert_key: alert.key.as_str().to_string(),
            kind: alert.kind.as_str().to_string(),
            asset: Some(alert.asset.symbol.clone()),
            window_start: alert.created_at - alert.metrics.window.ms(),
            severity: alert.severity.as_str().to_string(),
            degraded: alert.degraded,
            payload: serde_json::to_string(alert).unwrap_or_else(|_| "{}".into()),
            created_at: alert.created_at,
        };

        let fresh = self.store.register_alert(&record).await?;
        if !fresh {
            self.stats.alerts_duplicate.fetch_add(1, Ordering::Relaxed);
            return Ok(PublishOutcome::AlreadyKnown);
        }
        self.stats.alerts_published.fetch_add(1, Ordering::Relaxed);

        let topic = storage::Topic::from_str_code(alert.kind.topic());
        let recipients = self.store.recipients_for(topic).await?;
        if recipients.is_empty() {
            return Ok(PublishOutcome::Queued { recipients: 0 });
        }

        // «Широкий» сигнал — той, що долає до великої частки активних
        // користувачів. Такі сигнали згортаються: однаковий алерт для
        // тисяч людей — це спам, а не інформація.
        if self.is_broad(recipients.len()).await {
            self.fold(alert).await?;
            self.stats.alerts_folded.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                kind = alert.kind.as_str(),
                asset = %alert.asset.symbol,
                recipients = recipients.len(),
                "сигнал згорнуто у зведений дайджест"
            );
            return Ok(PublishOutcome::Folded {
                recipients: recipients.len(),
            });
        }

        let mut queued = 0usize;

        for chat_id in recipients {
            // Мова користувача визначає мову тексту.
            let lang = match self.store.get_user(chat_id).await? {
                Some(u) => u.language.as_str().to_string(),
                None => "uk".to_string(),
            };
            let text = alert.render(&lang);
            self.store
                .enqueue_delivery(NewDelivery {
                    chat_id,
                    alert_key: alert.key.as_str().to_string(),
                    priority: Priority::Broadcast,
                    text,
                    created_at: alert.created_at,
                })
                .await?;
            queued += 1;
        }
        Ok(PublishOutcome::Queued { recipients: queued })
    }

    /// Чи є сигнал «широким» — тобто таким, що його варто згорнути.
    ///
    /// Поріг — частка **активних** користувачів, яка має отримати сигнал.
    /// `0` у конфігурации означає «згортати все». Порівнюємо хрестом,
    /// без округлення: `recipients * 100 >= threshold * total`.
    async fn is_broad(&self, recipients: usize) -> bool {
        // Сигнал до одного одержувача ніколи не згортається: згортання має
        // сенс, коли один і той самий сигнал іде багатьом, а для однієї
        // людини це лише затримка й втрата деталей.
        if recipients < 2 {
            return false;
        }
        let threshold = self.cfg.digest_threshold_pct;
        if threshold == 0 {
            return true;
        }
        // Активні користувачі — знаменник. Якщо їх немає, то й одержувачів
        // немає: згортати нічого.
        let total = match self.store.count_recipients().await {
            Ok(n) if n > 0 => n,
            _ => return false,
        };
        (recipients as i64) * 100 >= (threshold as i64) * total
    }

    /// Додати сигнал до буфера зведень (у БД).
    ///
    /// Буфер у БД, а не в памʼяті: перезапуск процесу (або рестарт через
    /// конфлікт поллерів) не має втрачати згорнуті алерти — вони просто
    /// зникли б із ротації.
    async fn fold(&self, alert: &Alert) -> Result<(), BroadcastError> {
        let payload = serde_json::to_string(alert).unwrap_or_else(|_| "{}".into());
        self.store
            .fold_pending_insert(&storage::FoldPending {
                alert_key: alert.key.as_str().to_string(),
                kind: alert.kind.as_str().to_string(),
                win: alert.metrics.window.as_str().to_string(),
                asset: Some(alert.asset.symbol.clone()),
                payload,
                created_at: alert.created_at,
            })
            .await?;
        Ok(())
    }

    /// Злити накопичені сигнали в зведені дайджести — раз на вікно.
    ///
    /// Повертає кількість поставлених у чергу доставок. Якщо вікно ще не
    /// минуло — нічого не робимо.
    pub async fn flush_digests(&self, now: i64) -> Result<usize, BroadcastError> {
        // Джерело недоступне — не зливаємо: буфер згортання має пережити
        // збій, а вікно вимірюється від найстарішого накопиченого сигналу.
        if !self.source_available() {
            return Ok(0);
        }
        let window_ms = (self.cfg.digest_window_secs.max(1) as i64) * 1000;
        let pending = self.store.fold_pending_list().await?;
        if pending.is_empty() {
            return Ok(0);
        }
        // Вікно відраховується від найстаршого згорнутого сигналу.
        let since = pending.iter().map(|p| p.created_at).min().unwrap_or(now);
        if now - since < window_ms {
            return Ok(0);
        }

        // Групуємо за видом і вікном агрегації. Групування за активом не
        // працювало б: двадцять різних активів дали б двадцять повідомлень
        // замість одного зведення.
        let mut groups: HashMap<(AlertKind, String), Vec<storage::FoldPending>> = HashMap::new();
        for p in pending {
            let kind = match AlertKind::parse(&p.kind) {
                Some(k) => k,
                None => continue,
            };
            groups.entry((kind, p.win.clone())).or_default().push(p);
        }

        let mut queued = 0usize;
        let mut flushed: Vec<String> = Vec::new();
        for ((kind, win), items) in groups {
            if items.is_empty() {
                continue;
            }
            let topic = storage::Topic::from_str_code(kind.topic());
            let recipients = self.store.recipients_for(topic).await?;
            if recipients.is_empty() {
                continue;
            }
            // Сильніший алерт групи: користувачеві цікавить максимум.
            let strongest = items
                .iter()
                .filter_map(|p| serde_json::from_str::<Alert>(&p.payload).ok())
                .max_by(|a, b| {
                    a.severity.cmp(&b.severity).then(
                        a.metrics
                            .total_value_usd
                            .partial_cmp(&b.metrics.total_value_usd)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
                });
            let strongest = match strongest {
                Some(a) => a,
                None => continue,
            };
            // Активи групи — унікальні, до п'яти: показуємо, які саме.
            let mut assets: Vec<String> = Vec::new();
            for p in &items {
                let s = p.asset.clone().unwrap_or_default();
                if !s.is_empty() && !assets.contains(&s) {
                    assets.push(s);
                }
            }
            assets.truncate(5);
            let folded = FoldedSignal {
                count: items.len(),
                assets,
                strongest,
            };
            // Ключ доставки — з вікном у складі: щоб два різні вікна не
            // злилися в одну доставку.
            let alert_key = format!("digest_{}_{}_{}", kind.as_str(), win, since);

            for chat_id in recipients {
                let lang = match self.store.get_user(chat_id).await? {
                    Some(u) => u.language.as_str().to_string(),
                    None => "uk".to_string(),
                };
                let text = folded.render(&lang);
                self.store
                    .enqueue_delivery(NewDelivery {
                        chat_id,
                        alert_key: alert_key.clone(),
                        priority: Priority::Digest,
                        text,
                        created_at: now,
                    })
                    .await?;
                queued += 1;
            }
            for p in &items {
                flushed.push(p.alert_key.clone());
            }
            self.stats.digests_sent.fetch_add(1, Ordering::Relaxed);
        }

        // Прибираємо з буфера те, що вже пішло у зведенні. Якщо зняття
        // не вдалося — наступного разу зведення повториться (безпечно:
        // доставка ідемпотентна за ключем).
        if !flushed.is_empty() {
            self.store.fold_pending_remove(&flushed).await?;
        }
        Ok(queued)
    }

    /// Приватна відповідь — назовні, поза чергою розсилки.
    pub async fn reply(&self, chat_id: i64, text: &str) -> Result<(), BroadcastError> {
        let report = self.sanitizer.scrub(text);
        if !report.is_clean() {
            tracing::warn!(
                redactions = report.count(),
                "відповідь відфільтровано перед відправкою"
            );
        }
        self.limiter.acquire(chat_id).await;
        let msg = Outbound {
            text: report.text,
            reply_to: None,
        };
        match self.notifier.send(chat_id, &msg).await {
            Ok(()) => {
                self.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                self.stats.messages_failed.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(error = %e, chat = chat_id, "приватна відповідь не доставлена");
                Err(BroadcastError::Storage(storage::StoreError::Other(
                    format!("доставка не вдалась: {e}"),
                )))
            }
        }
    }

    /// Відповідь із привʼязкою до питання.
    pub async fn reply_to(
        &self,
        chat_id: i64,
        text: &str,
        reply_to: i64,
    ) -> Result<(), BroadcastError> {
        let report = self.sanitizer.scrub(text);
        self.limiter.acquire(chat_id).await;
        let msg = Outbound {
            text: report.text,
            reply_to: Some(reply_to),
        };
        match self.notifier.send(chat_id, &msg).await {
            Ok(()) => {
                self.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                self.stats.messages_failed.fetch_add(1, Ordering::Relaxed);
                Err(BroadcastError::Storage(storage::StoreError::Other(
                    format!("доставка не вдалась: {e}"),
                )))
            }
        }
    }

    /// Один крок черги: забрати готове до відправки, надіслати, записати результат.
    pub async fn pump_once(&self, now: i64) -> Result<usize, BroadcastError> {
        // Джерело недоступне — черга **буферизується**: нічого не
        // відбираємо й нічого не надсилаємо. Коли джерело оживе, буфер
        // вилиється сам (з перевіркою межі віку).
        if !self.source_available() {
            return Ok(0);
        }
        let batch = self.store.claim_deliveries(now, 50).await?;
        let mut done = 0usize;
        for delivery in batch {
            self.deliver(&delivery, now).await?;
            done += 1;
        }
        Ok(done)
    }

    /// Скинути доставки, що провисели в черзі (буфері) довше за межу.
    ///
    /// Межа — `max_backlog_minutes`. `0` — обмеження вимкнено: буфер
    /// безстроковий. Скинуті доставки отримують статус `skipped` і
    /// **не надсилаються ніколи**: вікно агрегації застаріло.
    ///
    /// Повертає кількість скинутих рядків.
    pub async fn expire_backlog(&self, now: i64) -> Result<usize, BroadcastError> {
        let minutes = self.cfg.max_backlog_minutes;
        if minutes == 0 {
            return Ok(0);
        }
        let cutoff = now - (minutes as i64) * 60_000;
        let expired = self
            .store
            .expire_pending_before(cutoff, "буфер прострочено: алерт застарів")
            .await?;
        if expired > 0 {
            self.stats
                .messages_expired
                .fetch_add(expired as u64, Ordering::Relaxed);
            tracing::warn!(
                expired,
                max_backlog_minutes = minutes,
                "буфер скинуто: алерти застаріли за межею"
            );
        }
        Ok(expired)
    }

    /// Скільки доставок чекає в черзі (буфер).
    pub async fn backlog_pending(&self) -> Result<i64, BroadcastError> {
        Ok(self.store.count_pending().await?)
    }

    async fn deliver(&self, delivery: &Delivery, now: i64) -> Result<(), BroadcastError> {
        // Фільтр — обов'язковий навіть для черги: текст алерту готував двигун,
        // але сюди може потрапити все, що завгодно.
        let report = self.sanitizer.scrub(&delivery.text);
        if !report.is_clean() {
            tracing::warn!(redactions = report.count(), "текст у черзі відфільтровано");
        }
        let msg = Outbound {
            text: report.text,
            reply_to: None,
        };

        let attempts = delivery.attempts + 1;
        let outcome = self.notifier.send(delivery.chat_id, &msg).await;

        match outcome {
            Ok(()) => {
                self.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
                self.store
                    .finish_delivery(DeliveryUpdate {
                        chat_id: delivery.chat_id,
                        alert_key: delivery.alert_key.clone(),
                        status: DeliveryStatus::Sent,
                        attempts,
                        next_attempt_at: now,
                        error: None,
                    })
                    .await?;
            }
            Err(SendError::RateLimited { retry_after }) => {
                self.stats.messages_retried.fetch_add(1, Ordering::Relaxed);
                let next = now + (retry_after as i64) * 1000;
                self.store
                    .finish_delivery(DeliveryUpdate {
                        chat_id: delivery.chat_id,
                        alert_key: delivery.alert_key.clone(),
                        status: DeliveryStatus::Pending,
                        attempts,
                        next_attempt_at: next,
                        error: Some(format!("rate limited: {retry_after}s")),
                    })
                    .await?;
            }
            Err(SendError::Permanent(err)) => {
                self.stats.messages_skipped.fetch_add(1, Ordering::Relaxed);
                self.store
                    .finish_delivery(DeliveryUpdate {
                        chat_id: delivery.chat_id,
                        alert_key: delivery.alert_key.clone(),
                        status: DeliveryStatus::Skipped,
                        attempts,
                        next_attempt_at: now,
                        error: Some(err),
                    })
                    .await?;
            }
            Err(SendError::Transient(err)) => {
                self.stats.messages_failed.fetch_add(1, Ordering::Relaxed);
                let backoff = self.backoff_ms(attempts);
                if attempts >= self.cfg.max_attempts {
                    self.stats.messages_skipped.fetch_add(1, Ordering::Relaxed);
                    self.store
                        .finish_delivery(DeliveryUpdate {
                            chat_id: delivery.chat_id,
                            alert_key: delivery.alert_key.clone(),
                            status: DeliveryStatus::Failed,
                            attempts,
                            next_attempt_at: now,
                            error: Some(err),
                        })
                        .await?;
                } else {
                    self.stats.messages_retried.fetch_add(1, Ordering::Relaxed);
                    self.store
                        .finish_delivery(DeliveryUpdate {
                            chat_id: delivery.chat_id,
                            alert_key: delivery.alert_key.clone(),
                            status: DeliveryStatus::Pending,
                            attempts,
                            next_attempt_at: now + backoff as i64,
                            error: Some(err),
                        })
                        .await?;
                }
            }
        }
        Ok(())
    }

    fn backoff_ms(&self, attempts: u32) -> u64 {
        let shift = attempts.clamp(1, 10) - 1;
        let base = self.cfg.backoff_base_ms.saturating_mul(1u64 << shift);
        base.min(self.cfg.backoff_cap_ms)
    }

    /// Воркер черги: крутиться, поки не скажуть зупинитися.
    pub async fn run_worker(self: Arc<Self>, stop: Arc<AtomicBool>) {
        while !stop.load(Ordering::Relaxed) {
            let now = alert_engine_now_ms();
            // Межа буфера перевіряється **перед** відбором доставок, на кожному
            // кроці: інакше прострочена доставка встигла б вийти з черги й
            // піти користувачеві на один цикл раніше, ніж її скинули.
            match self.expire_backlog(now).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(expired = n, "черга: скинуто застарілі доставки"),
                Err(e) => {
                    tracing::warn!(error = %e, "черга: не вдалося перевірити межу буфера")
                }
            }
            // Злиття зведених дайджестів — до відправки черги, щоб
            // зведення пішли разом з іншими повідомленнями.
            match self.flush_digests(now).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(count = n, "черга: зведено в дайджест"),
                Err(e) => tracing::warn!(error = %e, "черга: злиття дайджестів не вдалося"),
            }
            match self.pump_once(now).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(count = n, "черга: відправлено"),
                Err(e) => tracing::warn!(error = %e, "черга: крок не вдався"),
            }
            tokio::time::sleep(Duration::from_millis(self.cfg.poll_interval_ms)).await;
        }
        tracing::info!("воркер розсилки зупинено");
    }
}

fn alert_engine_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Вид алерту → тема підписки (для зручності вгорі).
pub fn topic_of(kind: AlertKind) -> &'static str {
    kind.topic()
}
