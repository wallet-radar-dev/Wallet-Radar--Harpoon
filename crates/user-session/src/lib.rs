//! Сесія користувача — тонкий шар над сховищем стану.
//!
//! Крейт не містить власної логіки зберігання: він лише перетворює виклики
//! сховища ([`storage::Store`]) на прикладні операції сесії та перекладає
//! помилки у [`SessionError`].
//!
//! Усі операції асинхронні і не блокують async-екзекутор.

use std::future::Future;

use storage::{Language, NewUser, Purged, Role, Settings, Store, Timeframe, Topic, User};

/// Помилки сесії.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Помилка нижнього рівня (сховище).
    #[error("помилка сховища: {0}")]
    Store(#[from] storage::StoreError),
    /// Користувача (чату) не знайдено.
    #[error("користувача не знайдено: chat_id={0}")]
    NotFound(i64),
}

/// Сесія одного користувача.
pub trait Session {
    /// Повертає профіль користувача.
    fn user(&self, chat_id: i64) -> impl Future<Output = Result<User, SessionError>>;
    /// Повертає налаштування користувача.
    fn settings(&self, chat_id: i64) -> impl Future<Output = Result<Settings, SessionError>>;
    /// Реєструє або оновлює користувача.
    fn register(&self, u: NewUser) -> impl Future<Output = Result<User, SessionError>>;
    /// Додає підписку на тему.
    fn subscribe(
        &self,
        chat_id: i64,
        topic: Topic,
    ) -> impl Future<Output = Result<(), SessionError>>;
    /// Прибирає підписку на тему.
    fn unsubscribe(
        &self,
        chat_id: i64,
        topic: Topic,
    ) -> impl Future<Output = Result<(), SessionError>>;
    /// Вмикає або вимикає mute.
    fn set_muted(
        &self,
        chat_id: i64,
        muted: bool,
    ) -> impl Future<Output = Result<(), SessionError>>;
    /// Змінює мову інтерфейсу.
    fn set_language(
        &self,
        chat_id: i64,
        lang: Language,
    ) -> impl Future<Output = Result<(), SessionError>>;
    /// Змінює обраний таймфрейм.
    fn set_timeframe(
        &self,
        chat_id: i64,
        tf: Timeframe,
    ) -> impl Future<Output = Result<(), SessionError>>;
    /// Видаляє всі дані користувача (GDPR).
    fn purge(&self, user_id: i64) -> impl Future<Output = Result<Purged, SessionError>>;
}

/// Обгортка сховища, що реалізує [`Session`].
#[derive(Clone)]
pub struct UserSession<S: Store> {
    store: S,
}

/// Синонім для [`UserSession`] — зручніше читається в коді, що працює з
/// конкретним сховищем.
pub type StoreSession<S> = UserSession<S>;

impl<S: Store> UserSession<S> {
    /// Створює сесію поверх сховища.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Доступ до нижнього сховища.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Гарантує, що запис користувача існує (потрібно для FK-цілісності).
    async fn ensure_user(&self, chat_id: i64) -> Result<User, SessionError> {
        if let Some(u) = self.store.get_user(chat_id).await? {
            return Ok(u);
        }
        let user = self
            .store
            .upsert_user(NewUser {
                chat_id,
                user_id: None,
                username: None,
                language: Language::Uk,
                timezone: "UTC".to_string(),
                role: Role::Viewer,
            })
            .await?;
        Ok(user)
    }
}

impl<S: Store> Session for UserSession<S> {
    async fn user(&self, chat_id: i64) -> Result<User, SessionError> {
        self.store
            .get_user(chat_id)
            .await?
            .ok_or(SessionError::NotFound(chat_id))
    }

    async fn settings(&self, chat_id: i64) -> Result<Settings, SessionError> {
        let u = self.user(chat_id).await?;
        Ok(Settings {
            language: u.language,
            timezone: u.timezone,
            timeframe: u.timeframe,
            muted: u.muted,
        })
    }

    async fn register(&self, u: NewUser) -> Result<User, SessionError> {
        Ok(self.store.upsert_user(u).await?)
    }

    async fn subscribe(&self, chat_id: i64, topic: Topic) -> Result<(), SessionError> {
        // Підписка має FK на users, тому запис користувача створюємо за потреби.
        self.ensure_user(chat_id).await?;
        self.store
            .set_subscription(&storage::Subscription {
                chat_id,
                topic,
                min_severity: "medium".to_string(),
            })
            .await?;
        tracing::debug!(
            chat_id,
            topic = topic.as_str(),
            "користувач підписаний на тему"
        );
        Ok(())
    }

    async fn unsubscribe(&self, chat_id: i64, topic: Topic) -> Result<(), SessionError> {
        self.store.remove_subscription(chat_id, topic).await?;
        tracing::debug!(chat_id, topic = topic.as_str(), "підписку знято");
        Ok(())
    }

    async fn set_muted(&self, chat_id: i64, muted: bool) -> Result<(), SessionError> {
        let mut s = self.settings(chat_id).await?;
        s.muted = muted;
        self.store.set_settings(chat_id, &s).await?;
        Ok(())
    }

    async fn set_language(&self, chat_id: i64, lang: Language) -> Result<(), SessionError> {
        let mut s = self.settings(chat_id).await?;
        s.language = lang;
        self.store.set_settings(chat_id, &s).await?;
        Ok(())
    }

    async fn set_timeframe(&self, chat_id: i64, tf: Timeframe) -> Result<(), SessionError> {
        let mut s = self.settings(chat_id).await?;
        s.timeframe = tf;
        self.store.set_settings(chat_id, &s).await?;
        Ok(())
    }

    async fn purge(&self, user_id: i64) -> Result<Purged, SessionError> {
        Ok(self.store.purge_user(user_id).await?)
    }
}
