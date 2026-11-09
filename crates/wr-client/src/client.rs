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
    ///
    /// **`source_reachable`** — чи джерело взагаді відповідає. Секції
    /// деградують у порожні, тож сам по собі `snapshot()` завжди `Ok`: без
    /// ціє ознаки оркестратор не відрізнив би «джерело впало» від «сервер
    /// відповів 404 на секцію, якої тут немає».
    pub async fn snapshot(&self) -> Result<Snapshot, WrError> {
        let (signals, _) = self.section("/api/signals", parse::parse_signals).await;
        let (whale_assets, ok_assets) = self
            .section("/api/whale-assets", parse::parse_whale_assets)
            .await;
        let (groups, ok_groups) = self
            .section("/api/capital-groups", parse::parse_capital_groups)
            .await;
        let (cross_asset, ok_cross) = self
            .section("/api/cross-asset", parse::parse_cross_asset)
            .await;
        let (archetypes, _) = self
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
            // Опорні секції: саме за ними судимо, чи джерело живе. Відсутність
            // маршруту (404) — це відповідь сервера, тож джерело доступне;
            // недоступне воно лише тоді, коли сервер не відповів узагалі.
            source_reachable: ok_assets || ok_groups || ok_cross,
        })
    }

    /// Секція зрізу: значення + чи сервер на неї відповів.
    ///
    /// Другий елемент — «сервер відповів» (хоч би 404): це те, за чим ми
    /// судимо про доступність джерела, а не про наявність маршруту.
    async fn section<T>(&self, path: &str, f: fn(&Value) -> T) -> (T, bool)
    where
        T: Default,
    {
        match self.get_json(path).await {
            Ok(v) => {
                let denied = parse::denied_keys_present(&v);
                if !denied.is_empty() {
                    tracing::debug!(path, ?denied, "відповідь містила поля, які не копіюємо");
                }
                (f(&v), true)
            }
            Err(e) => {
                let answered = !is_transport_failure(&e);
                tracing::warn!(path, error = %e, "секція джерела недоступна — порожньо");
                (T::default(), answered)
            }
        }
    }

    /// WS-з'єднання до стрічки подій.
    pub async fn connect_ws(&self, ws_path: &str) -> Result<crate::ws::WsStream, WrError> {
        crate::ws::WsStream::connect(&self.base_url, ws_path, self.basic_auth_value().as_deref())
            .await
    }
}

/// Помилка — це «сервер не відповів» (а не «сервер відповів не так»).
///
/// Транспортний збій (з'єднання відхилено, таймаут, DNS) означає, що джерело
/// недоступне. HTTP-відповідь (навіть 404 чи 500) — джерело відповідає.
fn is_transport_failure(e: &WrError) -> bool {
    matches!(e, WrError::Transport(_) | WrError::Connect(_))
}

/// Поточний час у мс.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Мінімальний HTTP-відповідач: на будь-який запит — 404 (маршруту
    /// такого немає, але сервер **відповів**).
    async fn spawn_404_server() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let resp =
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(resp).await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn зріз_каже_що_джерело_доступне_навіть_якщо_секції_404() {
        // Відсутність маршруту — це відповідь сервера, а не збій джерела:
        // саме так виглядає розгорнутий Wallet Radar без /api/signals.
        let port = spawn_404_server().await;
        let client = WrClient::new(format!("http://127.0.0.1:{port}"), "admin", "pass");
        let snap = client.snapshot().await.expect("зріз");
        assert!(
            snap.source_reachable,
            "сервер відповів — джерело доступне, буфер не вмикається"
        );
    }

    #[tokio::test]
    async fn зріз_каже_що_джерело_недоступне_коли_server_не_відповідає() {
        // Порт без слухача: з'єднання відхиляється — це транспортна помилка,
        // тобто джерело впало.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        let client = WrClient::new(format!("http://127.0.0.1:{port}"), "admin", "pass");
        let snap = client.snapshot().await.expect("зріз");
        assert!(
            !snap.source_reachable,
            "сервер не відповів — джерело недоступне, черга має буферизуватися"
        );
    }
}
