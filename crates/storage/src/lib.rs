//! Сховище стану для Telegram-бота фінансових алертів.
//!
//! Крейт складається з двох шарів:
//!
//! * доменні типи ([`Role`], [`Language`], [`Timeframe`], [`Topic`] тощо) та абстракція
//!   persistence — трейт [`Store`];
//! * конкретна реалізація [`SqliteStore`] поверх SQLite (rusqlite).
//!
//! `rusqlite` — синхронна бібліотека, тому кожна операція з базою виконується у
//! `tokio::task::spawn_blocking`: async-екзекутор ніколи не блокується.
//!
//! Час у всіх полях — UTC, epoch-мілісекунди (`i64`).
//! SQL-запити завжди параметризовані; логи — через `tracing`.

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

mod sqlite;

pub use sqlite::SqliteStore;

/// Поточний час у форматі epoch-мс (UTC).
///
/// Використовується там, де API не приймає час параметром.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Роль користувача.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Звичайний читач алертів.
    Viewer,
    /// Адміністратор (доступ до адмін-команд).
    Admin,
}

impl Role {
    /// Код ролі для зберігання в БД.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Admin => "admin",
        }
    }

    /// Розбір коду ролі; невідомий код трактується як [`Role::Viewer`].
    pub fn from_str_code(s: &str) -> Role {
        match s.trim().to_ascii_lowercase().as_str() {
            "admin" => Role::Admin,
            _ => Role::Viewer,
        }
    }
}

/// Мова інтерфейсу.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Українська.
    Uk,
    /// Англійська.
    En,
}

impl Language {
    /// Код мови для зберігання в БД.
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::Uk => "uk",
            Language::En => "en",
        }
    }

    /// Розбір коду мови; невідомий код трактується як [`Language::Uk`].
    pub fn from_str_code(s: &str) -> Language {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" => Language::En,
            _ => Language::Uk,
        }
    }
}

/// Таймфрейм підписки.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timeframe {
    /// Останні 24 години.
    H24,
    /// Останні 7 днів.
    D7,
    /// Останні 30 днів.
    D30,
}

impl Timeframe {
    /// Код таймфрейму для зберігання в БД.
    pub fn as_str(&self) -> &'static str {
        match self {
            Timeframe::H24 => "24h",
            Timeframe::D7 => "7d",
            Timeframe::D30 => "30d",
        }
    }

    /// Розбір коду таймфрейму; невідомий код трактується як [`Timeframe::H24`].
    pub fn from_str_code(s: &str) -> Timeframe {
        match s.trim().to_ascii_lowercase().as_str() {
            "7d" | "7" | "week" => Timeframe::D7,
            "30d" | "30" | "month" => Timeframe::D30,
            _ => Timeframe::H24,
        }
    }
}

/// Тема (канал) алертів.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// Великі транзакції.
    Whale,
    /// Рух коштів всередині групи.
    Group,
    /// Рух між різними активами.
    CrossAsset,
    /// Ліквідації позицій.
    Liquidation,
    /// Денний дайджест.
    Digest,
}

impl Topic {
    /// Код теми для зберігання в БД.
    pub fn as_str(&self) -> &'static str {
        match self {
            Topic::Whale => "whale",
            Topic::Group => "group",
            Topic::CrossAsset => "cross_asset",
            Topic::Liquidation => "liquidation",
            Topic::Digest => "digest",
        }
    }

    /// Розбір коду теми; невідомий код трактується як [`Topic::Whale`].
    pub fn from_str_code(s: &str) -> Topic {
        match s.trim().to_ascii_lowercase().as_str() {
            "group" => Topic::Group,
            "cross_asset" | "crossasset" => Topic::CrossAsset,
            "liquidation" => Topic::Liquidation,
            "digest" => Topic::Digest,
            _ => Topic::Whale,
        }
    }
}

/// Стан користувача.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserState {
    /// Зареєстрований, але ще не активний.
    Pending,
    /// Активний користувач.
    Active,
    /// Заблокований.
    Blocked,
}

impl UserState {
    /// Код стану для зберігання в БД.
    pub fn as_str(&self) -> &'static str {
        match self {
            UserState::Pending => "pending",
            UserState::Active => "active",
            UserState::Blocked => "blocked",
        }
    }

    /// Розбір коду стану; невідомий код трактується як [`UserState::Active`].
    pub fn from_str_code(s: &str) -> UserState {
        match s.trim().to_ascii_lowercase().as_str() {
            "pending" => UserState::Pending,
            "blocked" => UserState::Blocked,
            _ => UserState::Active,
        }
    }
}

/// Пріоритет доставки (впливає на порядок обробки черги).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Відповідь на конкретний запит користувача — найвищий пріоритет.
    Reply,
    /// Періодичний дайджест.
    Digest,
    /// Масова розсилка.
    Broadcast,
}

impl Priority {
    /// Числовий код пріоритету для зберігання в БД (менше — раніше).
    pub fn as_i64(&self) -> i64 {
        match self {
            Priority::Reply => 0,
            Priority::Digest => 1,
            Priority::Broadcast => 2,
        }
    }

    /// Розбір числового коду пріоритету; невідомий код трактується як [`Priority::Broadcast`].
    pub fn from_i64(v: i64) -> Priority {
        match v {
            0 => Priority::Reply,
            1 => Priority::Digest,
            _ => Priority::Broadcast,
        }
    }

    /// Код пріоритету для зберігання в БД.
    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Reply => "reply",
            Priority::Digest => "digest",
            Priority::Broadcast => "broadcast",
        }
    }

    /// Розбір текстового коду пріоритету.
    pub fn from_str_code(s: &str) -> Priority {
        match s.trim().to_ascii_lowercase().as_str() {
            "reply" => Priority::Reply,
            "digest" => Priority::Digest,
            _ => Priority::Broadcast,
        }
    }
}

/// Статус доставки алерту конкретному чату.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    /// У черзі, очікує на відправку.
    Pending,
    /// Забрано воркером, обробляється.
    Sending,
    /// Успішно відправлено.
    Sent,
    /// Не вдалося відправити (буде повтор).
    Failed,
    /// Пропущено (наприклад, користувач muted).
    Skipped,
}

impl DeliveryStatus {
    /// Код статусу для зберігання в БД.
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryStatus::Pending => "pending",
            DeliveryStatus::Sending => "sending",
            DeliveryStatus::Sent => "sent",
            DeliveryStatus::Failed => "failed",
            DeliveryStatus::Skipped => "skipped",
        }
    }

    /// Розбір коду статусу; невідомий код трактується як [`DeliveryStatus::Pending`].
    pub fn from_str_code(s: &str) -> DeliveryStatus {
        match s.trim().to_ascii_lowercase().as_str() {
            "sending" => DeliveryStatus::Sending,
            "sent" => DeliveryStatus::Sent,
            "failed" => DeliveryStatus::Failed,
            "skipped" => DeliveryStatus::Skipped,
            _ => DeliveryStatus::Pending,
        }
    }
}

/// Дані для створення/оновлення користувача.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewUser {
    /// Telegram chat id.
    pub chat_id: i64,
    /// Telegram user id (якщо відомий).
    pub user_id: Option<i64>,
    /// Нік користувача.
    pub username: Option<String>,
    /// Мова інтерфейсу.
    pub language: Language,
    /// Часовий пояс (IANA, наприклад `Europe/Kyiv`).
    pub timezone: String,
    /// Роль користувача.
    pub role: Role,
}

/// Користувач, який зберігається у сховищі.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// Telegram chat id.
    pub chat_id: i64,
    /// Telegram user id.
    pub user_id: Option<i64>,
    /// Нік користувача.
    pub username: Option<String>,
    /// Роль користувача.
    pub role: Role,
    /// Мова інтерфейсу.
    pub language: Language,
    /// Часовий пояс.
    pub timezone: String,
    /// Обраний таймфрейм.
    pub timeframe: Timeframe,
    /// Чи muted користувач.
    pub muted: bool,
    /// Чи заблокований користувач.
    pub blocked: bool,
    /// Стан користувача.
    pub state: UserState,
    /// Час створення (epoch-мс).
    pub created_at: i64,
    /// Час останнього оновлення (epoch-мс).
    pub updated_at: i64,
    /// Час останньої активності (epoch-мс).
    pub last_seen_at: Option<i64>,
}

/// Налаштування користувача.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Мова інтерфейсу.
    pub language: Language,
    /// Часовий пояс.
    pub timezone: String,
    /// Обраний таймфрейм.
    pub timeframe: Timeframe,
    /// Чи muted користувач.
    pub muted: bool,
}

/// Підписка користувача на тему.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    /// Telegram chat id власника підписки.
    pub chat_id: i64,
    /// Тема підписки.
    pub topic: Topic,
    /// Мінімальна серйозність алерту для доставки.
    pub min_severity: String,
}

/// Запит на постановку доставки в чергу.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDelivery {
    /// Telegram chat id одержувача.
    pub chat_id: i64,
    /// Ключ алерту (унікальний для чату).
    pub alert_key: String,
    /// Пріоритет доставки.
    pub priority: Priority,
    /// Текст повідомлення.
    pub text: String,
    /// Час постановки (epoch-мс).
    pub created_at: i64,
}

/// Запис черги доставок.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// Telegram chat id одержувача.
    pub chat_id: i64,
    /// Ключ алерту.
    pub alert_key: String,
    /// Пріоритет доставки.
    pub priority: Priority,
    /// Текст повідомлення.
    pub text: String,
    /// Поточний статус.
    pub status: DeliveryStatus,
    /// Кількість спроб відправки.
    pub attempts: u32,
    /// Час наступної спроби (epoch-мс).
    pub next_attempt_at: i64,
    /// Час створення (epoch-мс).
    pub created_at: i64,
}

/// Оновлення стану доставки.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryUpdate {
    /// Telegram chat id одержувача.
    pub chat_id: i64,
    /// Ключ алерту.
    pub alert_key: String,
    /// Новий статус.
    pub status: DeliveryStatus,
    /// Кількість спроб відправки.
    pub attempts: u32,
    /// Час наступної спроби (epoch-мс).
    pub next_attempt_at: i64,
    /// Текст останньої помилки.
    pub error: Option<String>,
}

/// Запис алерту в журналі (для ідемпотентності).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertRecord {
    /// Стійкий ключ алерту.
    pub alert_key: String,
    /// Вид алерту (`whale`, `liquidation` тощо).
    pub kind: String,
    /// Актив, якщо стосується.
    pub asset: Option<String>,
    /// Початок вікна агрегації (epoch-мс).
    pub window_start: i64,
    /// Серйозність алерту.
    pub severity: String,
    /// Чи алерт отримано в деградованому режимі.
    pub degraded: bool,
    /// Сирий payload джерела.
    pub payload: String,
    /// Час створення (epoch-мс).
    pub created_at: i64,
}

/// Сигнал, згорнутий у зведений дайджест, але ще не відправлений.
///
/// Буфер згортання живе у БД, а не в памʼяті процесу: перезапуск не має
/// втрачати згорнуті алерти.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldPending {
    /// Ключ алерту (унікальний — повтор не дублюється).
    pub alert_key: String,
    /// Вид алерту.
    pub kind: String,
    /// Вікно агрегації алерту (`15m`, `1h` тощо) — групувальна ознака.
    pub win: String,
    /// Символ актива.
    pub asset: Option<String>,
    /// Повний алерт (JSON) — щоб зібрати зведення без повторного читання джерела.
    pub payload: String,
    /// Час створення (epoch-мс).
    pub created_at: i64,
}

/// Запис журналу аудиту.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Посилання на чат (для GDPR-очищення).
    pub chat_ref: String,
    /// Хто виконав дію.
    pub actor: String,
    /// Що було зроблено.
    pub action: String,
    /// Додаткові деталі.
    pub detail: Option<String>,
    /// Час події (epoch-мс).
    pub at: i64,
}

/// Підсумок видалення даних користувача (GDPR).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Purged {
    /// Кількість видалених записів користувачів.
    pub users: u32,
    /// Кількість видалених доставок.
    pub deliveries: u32,
    /// Кількість видалених записів аудиту.
    pub audits: u32,
}

/// Помилки сховища.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Помилка роботи з SQLite.
    #[error("помилка SQLite: {0}")]
    Sql(#[from] rusqlite::Error),
    /// Блокуюча задача не завершилась (наприклад, runtime зупинено).
    #[error("помилка виконання блокуючої задачі: {0}")]
    Join(#[from] tokio::task::JoinError),
    /// Замок з'єднання з базою отруєно.
    #[error("замок з'єднання з базою отруєно")]
    LockPoisoned,
    /// Значення у базі не відповідає очікуваному формату.
    #[error("некоректні дані у сховищі: {0}")]
    InvalidData(String),
    /// Інша помилка, що не має окремого джерела (наприклад, збій доставки).
    #[error("{0}")]
    Other(String),
    /// Запис не знайдено.
    #[error("запис не знайдено: chat_id={0}")]
    NotFound(i64),
}

/// Абстракція сховища стану.
///
/// Реалізація зобов'язана:
/// * не блокувати async-екзекутор (усі звернення до БД — через `spawn_blocking`);
/// * зберігати час у UTC epoch-мс;
/// * виконувати лише параметризовані SQL-запити.
pub trait Store {
    /// Псевдонім чату для аудиту й логів: HMAC від chat_id (без відтворюваного
    /// chat_id у самому значенні).
    fn chat_ref(&self, chat_id: i64) -> String;
    /// Створює або оновлює користувача.
    fn upsert_user(&self, u: NewUser) -> impl Future<Output = Result<User, StoreError>>;
    /// Повертає користувача за chat id.
    fn get_user(&self, chat_id: i64) -> impl Future<Output = Result<Option<User>, StoreError>>;
    /// Повертає користувача за Telegram user id.
    fn find_user_by_user_id(
        &self,
        user_id: i64,
    ) -> impl Future<Output = Result<Option<User>, StoreError>>;
    /// Оновлює налаштування користувача.
    fn set_settings(
        &self,
        chat_id: i64,
        s: &Settings,
    ) -> impl Future<Output = Result<(), StoreError>>;
    /// Фіксує часову активність користувача.
    fn set_last_seen(&self, chat_id: i64, at: i64) -> impl Future<Output = Result<(), StoreError>>;
    /// Заблокувати/розблокувати користувача (admin-команда /block, /unblock).
    fn set_blocked(
        &self,
        chat_id: i64,
        blocked: bool,
    ) -> impl Future<Output = Result<(), StoreError>>;
    /// Повертає підписки користувача.
    fn subscriptions(
        &self,
        chat_id: i64,
    ) -> impl Future<Output = Result<Vec<Subscription>, StoreError>>;
    /// Створює або оновлює підписку.
    fn set_subscription(&self, s: &Subscription) -> impl Future<Output = Result<(), StoreError>>;
    /// Видаляє підписку.
    fn remove_subscription(
        &self,
        chat_id: i64,
        topic: Topic,
    ) -> impl Future<Output = Result<(), StoreError>>;
    /// Повертає символи активів, за якими стежить користувач.
    fn user_assets(&self, chat_id: i64) -> impl Future<Output = Result<Vec<String>, StoreError>>;
    /// Додає або прибирає символ активу.
    fn set_user_asset(
        &self,
        chat_id: i64,
        symbol: &str,
        on: bool,
    ) -> impl Future<Output = Result<(), StoreError>>;

    /// Ідемпотентно: `Ok(false)` — такий alert_key уже існує.
    fn register_alert(&self, a: &AlertRecord) -> impl Future<Output = Result<bool, StoreError>>;
    /// Чи існує alert_key.
    fn alert_exists(&self, alert_key: &str) -> impl Future<Output = Result<bool, StoreError>>;
    /// Ставить доставку в чергу (повтор не створює дубля).
    fn enqueue_delivery(&self, d: NewDelivery) -> impl Future<Output = Result<(), StoreError>>;
    /// claim: атомарно переводить pending→sending і повертає до N записів
    /// (для одного воркера — ORDER BY priority, next_attempt_at).
    fn claim_deliveries(
        &self,
        now: i64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Delivery>, StoreError>>;
    /// Фіксує результат спроби доставки.
    fn finish_delivery(&self, d: DeliveryUpdate) -> impl Future<Output = Result<(), StoreError>>;
    /// Скинути доставки, що провисели в буфері довше за межу.
    ///
    /// Торкається лише `pending`: надіслані, пропущені й невдалі — вже
    /// завершені, їх не чіпаємо. Повертає кількість скинутих рядків.
    fn expire_pending_before(
        &self,
        cutoff_ms: i64,
        reason: &str,
    ) -> impl Future<Output = Result<usize, StoreError>>;
    /// Скільки доставок чекає в черзі (буфер).
    fn count_pending(&self) -> impl Future<Output = Result<i64, StoreError>>;
    /// Активні (не muted, не blocked) чати, які мають підписку на тему, — для розсилки.
    fn recipients_for(&self, topic: Topic) -> impl Future<Output = Result<Vec<i64>, StoreError>>;
    /// Кількість активних одержувачів.
    fn count_recipients(&self) -> impl Future<Output = Result<i64, StoreError>>;
    /// Усі активні чати (не muted, не blocked, state='active') — для ручної розсилки.
    fn all_active_chats(&self) -> impl Future<Output = Result<Vec<i64>, StoreError>>;
    /// Усі користувачі — для синхронізації реєстру на сервер Wallet Radar.
    fn list_users(&self) -> impl Future<Output = Result<Vec<User>, StoreError>>;

    /// Алерти, створені не раніше `since_ms`, — новіші спершу.
    ///
    /// Джерело для дайджестів за період: у журналі лежать **агрегати**
    /// (вид, символ актива, важливість, вікно), а не сирі події, тому
    /// добірка звідти не може витікати адресами.
    fn alerts_since(
        &self,
        since_ms: i64,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, StoreError>>;

    /// Додати сигнал до буфера згортання (повтор із тим самим ключем — ні).
    fn fold_pending_insert(
        &self,
        p: &FoldPending,
    ) -> impl Future<Output = Result<bool, StoreError>>;
    /// Увесь буфер згортання.
    fn fold_pending_list(&self) -> impl Future<Output = Result<Vec<FoldPending>, StoreError>>;
    /// Прибрати з буфера зазначені сигнали (після формування зведення).
    fn fold_pending_remove(
        &self,
        alert_keys: &[String],
    ) -> impl Future<Output = Result<usize, StoreError>>;

    /// `Ok(true)` — це подія щойно побачена вперше (записано); `Ok(false)` — дублікат.
    fn seen_event(&self, event_id: &str, ts: i64)
        -> impl Future<Output = Result<bool, StoreError>>;
    /// `Ok(true)` — оновлення щойно побачене вперше; `Ok(false)` — дублікат.
    fn seen_update(&self, update_id: i64) -> impl Future<Output = Result<bool, StoreError>>;
    /// Читає значення з key-value сховища.
    fn cursor(&self, name: &str) -> impl Future<Output = Result<Option<String>, StoreError>>;
    /// Записує значення в key-value сховище.
    fn set_cursor(&self, name: &str, v: &str) -> impl Future<Output = Result<(), StoreError>>;

    /// Зберігає снапшот стану джерела.
    fn put_snapshot(
        &self,
        as_of: i64,
        payload: &str,
    ) -> impl Future<Output = Result<(), StoreError>>;
    /// Повертає останній збережений снапшот.
    fn last_snapshot(&self) -> impl Future<Output = Result<Option<(i64, String)>, StoreError>>;

    /// Додає запис до журналу аудиту.
    fn audit(&self, a: AuditEntry) -> impl Future<Output = Result<(), StoreError>>;
    /// Кількість записів аудиту для chat_ref.
    fn audit_count(&self, chat_ref: &str) -> impl Future<Output = Result<i64, StoreError>>;
    /// GDPR: видаляє юзера (каскадом підписки/активи/доставки) + його аудит.
    /// Повертає, що видалено.
    fn purge_user(&self, user_id: i64) -> impl Future<Output = Result<Purged, StoreError>>;
}
