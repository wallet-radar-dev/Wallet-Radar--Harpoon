//! Конфігурація бота.
//!
//! Секретів у конфіги немає: у TOML — лише **ім'я змінної** (`token_env`), а
//! значення читається з оточення. Тому конфіг можна комітити в git, а
//! `.env` — ні.

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("не вдалося прочитати конфіг {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("не вдалося розібрати конфіг {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("змінна оточення {name} не задана")]
    MissingEnv { name: String },
}

/// Повна конфігурація.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub bot: BotConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub wr: WrConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub rate_limits: RateLimitsConfig,
    #[serde(default)]
    pub broadcast: BroadcastConfig,
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub features: FeaturesConfig,
    #[serde(default)]
    pub alerts: AlertsConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BotConfig {
    pub name: String,
    /// Ім'я змінної оточення з токеном Telegram.
    pub token_env: String,
    pub default_language: String,
    pub default_timezone: String,
    pub poll_timeout_secs: u64,
    pub heartbeat_timeout_secs: u64,
    pub log_level: String,
    /// Ім'я змінної з секретом для HMAC-підписів chat_ref у логах.
    pub ref_secret_env: String,
    /// Що робити, коли Telegram відповів 409 (на токені вже працює
    /// інший полер).
    ///
    /// * `keep_running` — зупинити лише **опитування команд**; решта бота
    ///   (збір даних, згортання, доставка алертів) продовжує працювати.
    ///   Опитування автоматично повторюється, тож коли конфлікт зникне,
    ///   команди знову оброблятимуться.
    /// * `exit` — завершити процес: нічого не дублюємо, але й нічого не
    ///   надсилаємо.
    /// * `preempt` — перехопити токен: після 409 повторити опитування за
    ///   кілька секунд, щоб перебити чужий запит і забирати оновлення собі.
    pub poll_conflict_policy: PollConflictPolicy,
    /// Пауза між повторами опитування після 409 (політика `keep_running`).
    pub poll_conflict_retry_secs: u64,
    /// Пауза перед повтором опитування в політиці `preempt`.
    ///
    /// Мала: чим швидше ми переб'ємо чужий запит, тим більше часу наш
    /// запит висить і ми забираємо оновлення.
    pub poll_conflict_preempt_secs: u64,
}

/// Політика поведінки при конфлікті поллерів на токені.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PollConflictPolicy {
    /// Зупинити опитування, решта бота працює.
    #[default]
    KeepRunning,
    /// Завершити процес.
    Exit,
    /// Перехопити токен: не чекати довго, а одразу повторити опитування.
    ///
    /// Якщо на токені працює інший полер, Telegram віддає `409` **старшому**
    /// з двох запитів (перевірено експериментом), а новіший — перемагає.
    /// Тому достатньо повторити запит небагато згодом після `409`: наш
    /// запит переб'є чужий, і ми забиратимемо оновлення собі.
    Preempt,
}

impl PollConflictPolicy {
    /// Чи ця політика перехоплює токен (а не просто чекає).
    pub fn preempts(&self) -> bool {
        matches!(self, PollConflictPolicy::Preempt)
    }
}

/// Пауза перед наступним опитуванням після конфлікту поллерів.
///
/// `preempt` — коротка пауза: ми мусимо перебити чужий запит, поки він ще
/// висить. Інакше — довга пауза, щоб не смикати API зайво.
pub fn conflict_retry_delay(
    policy: PollConflictPolicy,
    preempt_secs: u64,
    retry_secs: u64,
) -> std::time::Duration {
    let secs = if policy.preempts() {
        preempt_secs
    } else {
        retry_secs
    };
    std::time::Duration::from_secs(secs.max(1))
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            name: "wallet-radar-alerts".into(),
            token_env: "TELEGRAM_BOT_TOKEN".into(),
            default_language: "uk".into(),
            default_timezone: "Europe/Kyiv".into(),
            poll_timeout_secs: 25,
            heartbeat_timeout_secs: 90,
            log_level: "info".into(),
            ref_secret_env: "BOT_REF_SECRET".into(),
            poll_conflict_policy: PollConflictPolicy::default(),
            poll_conflict_retry_secs: 60,
            poll_conflict_preempt_secs: 2,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TelegramConfig {
    /// Перевизначення адреси Telegram API — потрібне для тестів і моків.
    pub api_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WrConfig {
    pub base_url: String,
    pub username: String,
    /// Ім'я змінної оточення з паролем.
    pub password_env: String,
    /// Ім'я змінної оточення з токеном синхронізації реєстру підключених.
    /// Порожньо або не задано — реєстр на сервер не пишеться.
    pub sync_token_env: String,
    /// Чи синхронізувати реєстр підключених із сервером Wallet Radar.
    ///
    /// Тестові й локальні запуски мають це вимкнути: сервер **замінює**
    /// файл реєстру цілком, тому тестовий інстанс із порожньою БД стер би
    /// список реальних підписників.
    pub registry_sync_enabled: bool,
    /// Як часто синхронізувати реєстр.
    pub registry_sync_interval_secs: u64,
    /// Додаткові сервери Wallet Radar, куди теж пишеться реєстр.
    ///
    /// У мережі буває кілька незалежних екземплярів Wallet Radar, і в кожному
    /// реєстр — це список, за яким сервер розсилає свої власні алерти. Якщо
    /// екземпляр має ввімкнений Telegram, але його реєстр не оновлюється,
    /// підписки розходять: нові підписники не отримують серверних алертів.
    ///
    /// Тому можна перелічити адреси додаткових серверів — реєстр піде в
    /// кожен із них тим самим вузьким токеном `sync_token_env`.
    pub registry_sync_extra_urls: Vec<String>,
    pub poll_interval_secs: u64,
    pub event_poll_interval_secs: u64,
    pub request_timeout_secs: u64,
    pub ws_path: String,
}

impl Default for WrConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:3000".into(),
            username: "admin".into(),
            password_env: "WR_PASSWORD".into(),
            sync_token_env: "TELEGRAM_SYNC_TOKEN".into(),
            registry_sync_enabled: true,
            registry_sync_interval_secs: 60,
            registry_sync_extra_urls: Vec::new(),
            poll_interval_secs: 60,
            event_poll_interval_secs: 15,
            request_timeout_secs: 20,
            ws_path: "/ws".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Бекенд сховища: `sqlite` або `postgres`.
    pub backend: String,
    /// Шлях до файлу SQLite (використовується, коли `backend = "sqlite"`).
    pub sqlite_path: String,
    /// Таймаут очікування SQLite-замку (мс).
    pub busy_timeout_ms: u64,
    /// Ім'я змінної оточення з URL бази Postgres
    /// (`postgres://user:pass@host:port/db`).
    pub postgres_url_env: String,
    /// Розмір пулу з'єднань до Postgres.
    pub postgres_max_connections: u32,
    /// Таймаут встановлення з'єднання з Postgres (секунди).
    pub postgres_connect_timeout_secs: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: "sqlite".into(),
            sqlite_path: "data/bot.db".into(),
            busy_timeout_ms: 5000,
            postgres_url_env: "POSTGRES_URL".into(),
            postgres_max_connections: 5,
            postgres_connect_timeout_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub mask: String,
    pub redact_ens: bool,
    pub redact_explorer_urls: bool,
    pub redact_base58: bool,
    pub extra_blocked_domains: Vec<String>,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            mask: "приховано".into(),
            redact_ens: true,
            redact_explorer_urls: true,
            redact_base58: true,
            extra_blocked_domains: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RateLimitsConfig {
    pub per_user_per_min: u32,
    pub admin_per_min: u32,
    pub global_inbound_per_min: u32,
    pub telegram_global_per_sec: u32,
    pub telegram_per_chat_min_interval_ms: u64,
    pub max_pending_per_chat: u32,
}

impl Default for RateLimitsConfig {
    fn default() -> Self {
        Self {
            per_user_per_min: 20,
            admin_per_min: 60,
            global_inbound_per_min: 200,
            telegram_global_per_sec: 25,
            telegram_per_chat_min_interval_ms: 1050,
            max_pending_per_chat: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BroadcastConfig {
    pub digest_threshold_pct: u32,
    pub digest_window_secs: u64,
    pub max_backlog_minutes: u32,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            digest_threshold_pct: 25,
            digest_window_secs: 300,
            max_backlog_minutes: 30,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AdminConfig {
    pub usernames: Vec<String>,
    /// Ім'я змінної оточення з кодом підключення (порожньо — /start без коду).
    pub access_code_env: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct FeaturesConfig {
    /// Асистент на основі /api/ask. За замовчуванням вимкнено: LLM може
    /// процитувати сирі дані, а ми не хочемо ризикувати приватністю.
    pub assistant: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AlertsConfig {
    /// Правила алертів (пороги).
    pub rules: Vec<AlertRuleConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AlertRuleConfig {
    pub kind: String,
    pub min_participants: u32,
    pub min_total_value_usd: f64,
    #[serde(default)]
    pub windows: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default = "default_severity")]
    pub severity: String,
}

fn default_severity() -> String {
    "medium".into()
}

impl Config {
    /// Прочитати конфіг із файлу.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        toml::from_str(&raw).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    /// Значення змінної оточення.
    pub fn env(&self, name: &str) -> Result<String, ConfigError> {
        std::env::var(name).map_err(|_| ConfigError::MissingEnv {
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preempt_перехоплює_токен_швидко_а_решта_чекає_довго() {
        // `preempt` — коротка пауза: наш запит має перебити чужий, поки той
        // ще висить.
        assert_eq!(
            conflict_retry_delay(PollConflictPolicy::Preempt, 2, 60),
            std::time::Duration::from_secs(2)
        );
        // Решта політики — довга пауза, щоб не смикати API.
        assert_eq!(
            conflict_retry_delay(PollConflictPolicy::KeepRunning, 2, 60),
            std::time::Duration::from_secs(60)
        );
        assert_eq!(
            conflict_retry_delay(PollConflictPolicy::Exit, 2, 60),
            std::time::Duration::from_secs(60)
        );
    }

    #[test]
    fn пауза_не_буває_нульовою() {
        // Нуль секунд — це «опитувати в циклі без пауз»: Telegram пошле
        // відмову на такий темп, тож мінімум — одна секунда.
        assert_eq!(
            conflict_retry_delay(PollConflictPolicy::Preempt, 0, 0),
            std::time::Duration::from_secs(1)
        );
    }
}
