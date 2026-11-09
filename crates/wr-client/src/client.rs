//! HTTP-клєнт до Wallet Radar.
//!
//! Read-only: ми ніколи не пишемо в сервіс. Автентифікація — HTTP Basic тим самим
//! читачем, що й веб-інтерфейс (у конфізі окремо).

use crate::parse;
use crate::types::*;
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WrError {
    #[error("Wallet Radar відповів {status} на {url}")]
    Http { status: u16, url: String },
    #[error("помилка транспорту до Wallet Radar: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("некоректний JSON від Wallet Radar: {0}")]
    Json(#[from] serde_json::Error),
    #[error("не вдалося підключитися: {0}")]
    Connect(String),
}

/// Клієнт зовнішнього сервісу.
#[derive(Clone)]
pub struct WrClient {
    base_url: String,
    username: String,
    password: String,
    http: reqwest::Client,
}

impl WrClient {
    pub fn new(
        base_url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self::with_timeout(base_url, username, password, 20)
    }

    /// Клієнт із явним таймаутом запиту.
    pub fn with_timeout(
        base_url: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
        timeout_secs: u64,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "reqwest-клієнт не збудувався — використаю дефолтний");
                reqwest::Client::new()
            });
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            username: username.into(),
            password: password.into(),
            http,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Значення заголовка Authorization для WS-з'єднання (там немає нормального
    /// Basic-механізму у запиті — та сама семантика, але вручну).
    pub fn basic_auth_value(&self) -> Option<String> {
        if self.username.is_empty() && self.password.is_empty() {
            return None;
        }
        let raw = format!("{}:{}", self.username, self.password);
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        Some(format!("Basic {encoded}"))
    }

    fn headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = self.basic_auth_value() {
            if let Ok(value) = HeaderValue::from_str(&v) {
                h.insert(AUTHORIZATION, value);
            }
        }
        h
    }

    async fn get_json(&self, path: &str) -> Result<Value, WrError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .get(&url)
            .headers(self.headers())
            .send()
            .await
            .map_err(WrError::Transport)?;
        let status = resp.status();
        if !status.is_success() {
            return Err(WrError::Http {
                status: status.as_u16(),
                url: path.to_string(),
            });
        }
        let body = resp.text().await.map_err(WrError::Transport)?;
        serde_json::from_str(&body).map_err(WrError::Json)
    }

    /// `/api/events` — потік подій гаманців. Джерело не має серверного курсора,
    /// тому клієнт сам тримає watermark (див. `alert-engine`).
    pub async fn events(&self, limit: u32) -> Result<Vec<RawEvent>, WrError> {
        let v = self.get_json(&format!("/api/events?limit={limit}")).await?;
        Ok(parse::parse_events(&v))
    }

    /// Зріз агрегатів. Одна недоступна секція не ламає весь зріз: її повертаємо
    /// порожньою й логуємо — краще показати менше, ніж не показати нічого.
    pub async fn snapshot(&self) -> Result<Snapshot, WrError> {
        let signals = self.section("/api/signals", parse::parse_signals).await;
        let whale_assets = self
            .section("/api/whale-assets", parse::parse_whale_assets)
            .await;
        let groups = self
            .section("/api/capital-groups", parse::parse_capital_groups)
            .await;
        let cross_asset = self
            .section("/api/cross-asset", parse::parse_cross_asset)
            .await;
        let archetypes = self
            .section("/api/meta-subnets", parse::parse_archetypes)
            .await;

        let liquidations = match self.get_json("/api/liquidations").await {
            Ok(v) => parse::parse_liquidations(&v),
            Err(e) => {
                tracing::warn!(error = %e, "секція ліквідацій недоступна");
                None
            }
        };

        let track_record = match self.get_json("/api/track-record").await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::debug!(error = %e, "track-record недоступний");
                None
            }
        };

        Ok(Snapshot {
            as_of: now_ms(),
            signals,
            whale_assets,
            groups,
            cross_asset,
            liquidations,
            archetypes,
            track_record,
        })
    }

    async fn section<T>(&self, path: &str, f: fn(&Value) -> T) -> T
    where
        T: Default,
    {
        match self.get_json(path).await {
            Ok(v) => {
                let denied = parse::denied_keys_present(&v);
                if !denied.is_empty() {
                    tracing::debug!(path, ?denied, "відповідь містила поля, які не копіюємо");
                }
                f(&v)
            }
            Err(e) => {
                tracing::warn!(path, error = %e, "секція джерела недоступна — порожньо");
                T::default()
            }
        }
    }

    /// WS-з'єднання до стрічки подій.
    pub async fn connect_ws(&self, ws_path: &str) -> Result<crate::ws::WsStream, WrError> {
        crate::ws::WsStream::connect(&self.base_url, ws_path, self.basic_auth_value().as_deref())
            .await
    }
}

/// Поточний час у мс.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
