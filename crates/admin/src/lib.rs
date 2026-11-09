//! Адмінські дії: розсилка, статистика, блокування, аудит.
//!
//! Адмін — це не «особливий користувач», а роль, визначена конфігом
//! (`[admin].usernames`). Роль перевіряється **до** виконання дії, і кожна дія
//! пише в аудит.
//!
//! Важливо: адмінський ручний текст проходить через той самий фільтр, що й
//! будь-який інший вихід. Адмін теж може випадково вставити адресу — і не повинен.

use broadcast::{Broadcaster, Notifier};
use privacy_filter::PrivacyFilter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::Arc;

use storage::{AuditEntry, Store};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdminError {
    #[error("помилка сховища: {0}")]
    Storage(#[from] storage::StoreError),
    #[error("доставка не вдалась: {0}")]
    Delivery(String),
}

/// Операційна статистика (без персональних даних і без статистики підписок).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AdminStats {
    pub recipients: i64,
    pub alerts_published: u64,
    pub alerts_duplicate: u64,
    pub messages_sent: u64,
    pub messages_failed: u64,
    pub messages_retried: u64,
    pub messages_skipped: u64,
}

/// Розсилка, якою користується адмін.
pub trait AdminBroadcaster {
    fn publish_manual(&self, text: &str) -> impl Future<Output = Result<usize, AdminError>>;
    fn stats(&self) -> AdminStats;
}

impl<S: Store, N: Notifier> AdminBroadcaster for Broadcaster<S, N> {
    /// Ручний алерт усім підключеним.
    ///
    /// Одержачі — усі активні чати (не заглушені, не заблоковані). Статистики
    /// підписок у відповідях немає й не буде: рахуємо тих, хто підключений.
    async fn publish_manual(&self, text: &str) -> Result<usize, AdminError> {
        let chats = self.store().all_active_chats().await?;
        if chats.is_empty() {
            return Ok(0);
        }

        // Ключ — хеш тексту й хвилини: повтор того самого ручного алерту в ту саму
        // хвилину не дублюється.
        let now = now_ms();
        let mut h = Sha256::new();
        h.update(text.as_bytes());
        h.update(b"|");
        h.update((now / 60_000).to_le_bytes());
        let key = format!("manual_{}", hex::encode(h.finalize()));

        let record = storage::AlertRecord {
            alert_key: key.clone(),
            kind: "manual".into(),
            asset: None,
            window_start: now,
            severity: "high".into(),
            degraded: false,
            payload: serde_json::json!({"manual": true}).to_string(),
            created_at: now,
        };
        if !self.store().register_alert(&record).await? {
            return Ok(0);
        }

        let mut queued = 0usize;
        for chat_id in chats {
            self.store()
                .enqueue_delivery(storage::NewDelivery {
                    chat_id,
                    alert_key: key.clone(),
                    priority: storage::Priority::Broadcast,
                    text: text.to_string(),
                    created_at: now,
                })
                .await?;
            queued += 1;
        }
        Ok(queued)
    }

    fn stats(&self) -> AdminStats {
        let s = Broadcaster::stats(self);
        AdminStats {
            recipients: 0,
            alerts_published: s.alerts_published,
            alerts_duplicate: s.alerts_duplicate,
            messages_sent: s.messages_sent,
            messages_failed: s.messages_failed,
            messages_retried: s.messages_retried,
            messages_skipped: s.messages_skipped,
        }
    }
}

/// Сервіс адмінки, яким користується роутер команд.
pub trait AdminService {
    fn is_admin(&self, username: Option<&str>) -> bool;
    fn stats(&self) -> impl Future<Output = Result<AdminStats, AdminError>>;
    fn broadcast_manual(
        &self,
        actor_chat_id: i64,
        actor_username: Option<&str>,
        text: &str,
    ) -> impl Future<Output = Result<usize, AdminError>>;
    fn block(
        &self,
        actor_chat_id: i64,
        target_chat_id: i64,
        blocked: bool,
    ) -> impl Future<Output = Result<(), AdminError>>;
}

impl<S: Store, B: AdminBroadcaster> AdminService for Admin<S, B> {
    fn is_admin(&self, username: Option<&str>) -> bool {
        Admin::is_admin(self, username)
    }

    async fn stats(&self) -> Result<AdminStats, AdminError> {
        Admin::stats(self).await
    }

    async fn broadcast_manual(
        &self,
        actor_chat_id: i64,
        actor_username: Option<&str>,
        text: &str,
    ) -> Result<usize, AdminError> {
        Admin::broadcast_manual(self, actor_chat_id, actor_username, text).await
    }

    async fn block(
        &self,
        actor_chat_id: i64,
        target_chat_id: i64,
        blocked: bool,
    ) -> Result<(), AdminError> {
        Admin::block(self, actor_chat_id, target_chat_id, blocked).await
    }
}

/// Обгортка `Arc` теж вміє розсилати — так зручніше жити в архітектурі,
/// де один розсильник живить у кількох місцях.
impl<B: AdminBroadcaster> AdminBroadcaster for Arc<B> {
    async fn publish_manual(&self, text: &str) -> Result<usize, AdminError> {
        (**self).publish_manual(text).await
    }

    fn stats(&self) -> AdminStats {
        (**self).stats()
    }
}

/// Адмін-сервіс.
pub struct Admin<S: Store, B: AdminBroadcaster> {
    store: S,
    broadcaster: B,
    sanitizer: PrivacyFilter,
    admins: Vec<String>,
}

impl<S: Store, B: AdminBroadcaster> Admin<S, B> {
    pub fn new(
        store: S,
        broadcaster: B,
        sanitizer: PrivacyFilter,
        admins: Vec<String>,
    ) -> Admin<S, B> {
        Admin {
            store,
            broadcaster,
            sanitizer,
            admins: admins
                .into_iter()
                .map(|a| a.trim().trim_start_matches('@').to_lowercase())
                .filter(|a| !a.is_empty())
                .collect(),
        }
    }

    /// Чи цей юзер — адмін.
    pub fn is_admin(&self, username: Option<&str>) -> bool {
        if self.admins.is_empty() {
            return false;
        }
        match username {
            Some(u) => self
                .admins
                .iter()
                .any(|a| a == &u.trim().trim_start_matches('@').to_lowercase()),
            None => false,
        }
    }

    /// Статистика.
    pub async fn stats(&self) -> Result<AdminStats, AdminError> {
        let recipients = self.store.count_recipients().await?;
        let mut s = self.broadcaster.stats();
        s.recipients = recipients;
        Ok(s)
    }

    /// Ручна розсилка. Текст спершу фільтрується.
    pub async fn broadcast_manual(
        &self,
        actor_chat_id: i64,
        actor_username: Option<&str>,
        text: &str,
    ) -> Result<usize, AdminError> {
        let report = self.sanitizer.scrub(text);
        let changed = report.count();
        let n = self.broadcaster.publish_manual(&report.text).await?;
        self.store
            .audit(AuditEntry {
                chat_ref: self.store.chat_ref(actor_chat_id),
                actor: "admin".into(),
                action: "broadcast".into(),
                detail: Some(format!(
                    "chars={} recipients={n} redacted={changed}",
                    text.len()
                )),
                at: now_ms(),
            })
            .await?;
        let _ = actor_username;
        Ok(n)
    }

    /// Блокування користувача.
    pub async fn block(
        &self,
        actor_chat_id: i64,
        target_chat_id: i64,
        blocked: bool,
    ) -> Result<(), AdminError> {
        self.store.set_blocked(target_chat_id, blocked).await?;
        self.store
            .audit(AuditEntry {
                chat_ref: self.store.chat_ref(actor_chat_id),
                actor: "admin".into(),
                action: if blocked { "block" } else { "unblock" }.into(),
                detail: Some(format!("target={target_chat_id}")),
                at: now_ms(),
            })
            .await?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
