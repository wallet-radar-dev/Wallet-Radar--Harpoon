//! Синхронізація реєстру підключених із сервером Wallet Radar.
//!
//! Навіщо це, якщо новий бот — окремий продукт. Історично реєстр підключених
//! живе на сервері Wallet Radar (`dataset/telegram-users.json`), і **сервер
//! розсилає свої власні алерти саме цим списком**. Якщо новий бот не
//! синхронізує свій список, вийде розкол: серверні алерти йдуть старим
//! переліком, а нові підписники нового бота їх не отримують.
//!
//! Тому новий бот пише в сервер реєстр — тим самим вузьким токеном
//! `X-Telegram-Sync-Token`, що й старий `botd`. Це **запис одного файлу на
//! локальному сервісі**, а не читання даних: усі *дані* бот бере через
//! read-only API.
//!
//! Сервер **замінює файл цілком**, тому надсилаємо повний список, а не дельту.

use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use storage::Store;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("помилка сховища: {0}")]
    Storage(#[from] storage::StoreError),
    #[error("помилка синхронізації реєстру: {0}")]
    Http(String),
}

/// Синхронізатор реєстру.
pub struct RegistrySync<S: Store> {
    store: S,
    base_url: String,
    sync_token: String,
    http: reqwest::Client,
}

impl<S: Store> RegistrySync<S> {
    pub fn new(store: S, base_url: &str, sync_token: &str) -> RegistrySync<S> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        RegistrySync {
            store,
            base_url: base_url.trim_end_matches('/').to_string(),
            sync_token: sync_token.to_string(),
            http,
        }
    }

    /// Побудувати тіло реєстру зі стану бота.
    pub async fn build_payload(&self) -> Result<Value, SyncError> {
        let users = self.store.list_users().await?;
        let mut map = serde_json::Map::new();
        for u in users {
            // До сервера йдуть усі, хто підключився: mute — це «не надсилати
            // алерти», а не «видалити з реєстру».
            map.insert(
                u.chat_id.to_string(),
                json!({
                    "chat": u.chat_id.to_string(),
                    "username": u.username.clone().unwrap_or_default(),
                    "role": u.role.as_str(),
                    "muted": u.muted,
                    "joined": u.created_at / 1000,
                    "last": u.last_seen_at.unwrap_or(u.updated_at) / 1000,
                }),
            );
        }
        Ok(json!({ "users": Value::Object(map) }))
    }

    /// Надіслати реєстр на сервер.
    pub async fn sync_once(&self) -> Result<usize, SyncError> {
        let payload = self.build_payload().await?;
        let count = payload["users"].as_object().map(|m| m.len()).unwrap_or(0);
        if self.sync_token.is_empty() {
            return Err(SyncError::Http(
                "токен синхронізації не задано — реєстр на сервері не оновлюється".into(),
            ));
        }
        // Порожній реєстр на сервері означає «серверні алерти не отримує ніхто».
        // Синхронізація — це запис, який **замінює** файл цілком, тому
        // випадковий запуск бота з порожньою БД (тест, міграція, нова інстанція)
        // стер би список реальних підписників. Тому порожній список ніколи
        // не надсилаємо, а лише фіксуємо в лозці.
        if count == 0 {
            return Err(SyncError::Http(
                "реєстр порожній — на сервер не надсилаємо (захист від стирання підписників)"
                    .into(),
            ));
        }
        let url = format!("{}/api/telegram-users", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("X-Telegram-Sync-Token", &self.sync_token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| SyncError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SyncError::Http(format!(
                "сервер відповів {}",
                resp.status().as_u16()
            )));
        }
        Ok(count)
    }

    /// Фоновий цикл синхронізації.
    pub async fn run(
        self: Arc<Self>,
        interval: Duration,
        stop: Arc<std::sync::atomic::AtomicBool>,
    ) {
        use std::sync::atomic::Ordering;
        let mut last_error: Option<String> = None;
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match self.sync_once().await {
                Ok(n) => {
                    tracing::debug!(users = n, "реєстр синхронізовано із сервером");
                    last_error = None;
                }
                Err(e) => {
                    // Помилку логуємо лише коли вона змінилась: інакше раз на
                    // інтервал отримаємо десятки однакових рядків у журналі.
                    let msg = e.to_string();
                    if last_error.as_deref() != Some(msg.as_str()) {
                        tracing::warn!(error = %msg, "реєстр не синхронізовано");
                        last_error = Some(msg);
                    }
                }
            }
            tokio::time::sleep(interval).await;
        }
    }
}
