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
    pub backend: String,
    pub sqlite_path: String,
    pub busy_timeout_ms: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: "sqlite".into(),
            sqlite_path: "data/bot.db".into(),
            busy_timeout_ms: 5000,
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
