//! Сесія користувача — тонкий шар над сховищем стану.
//!
//! Крейт не містить власної логіки зберігання: він лише перетворює виклики
//! сховища ([`storage::Store`]) на прикладні операції сесії та перекладає
//! помилки у [`SessionError`].
//!
//! Усі операції асинхронні і не блокують async-екзекутор.

use std::collections::HashMap;
use std::future::Future;

use alert_engine::{AlertKind, Severity};
use storage::{
    AlertRecord, Language, NewUser, Purged, Role, Settings, Store, Timeframe, Topic, User,
};

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
    /// Алерти за період, релевантні цьому користувачу.
    ///
    /// Фільтри — ті самі, що й для доставки в реальному часі: підписки на
    /// теми, мінімальна важливість у підписці та перелік відстежуваних
    /// активів. Якщо підписок немає, показуємо всі алерти: дайджест має
    /// бути корисним і тим, хто ще нічого не підписав.
    fn alerts_since(
        &self,
        chat_id: i64,
        since_ms: i64,
    ) -> impl Future<Output = Result<Vec<AlertRecord>, SessionError>>;
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

    async fn alerts_since(
        &self,
        chat_id: i64,
        since_ms: i64,
    ) -> Result<Vec<storage::AlertRecord>, SessionError> {
        let all = self.store.alerts_since(since_ms).await?;
        let subs = self.store.subscriptions(chat_id).await?;
        let assets = self.store.user_assets(chat_id).await?;

        // Порожні підписки означають «показуй усе»: дайджест має бути
        // корисним і тим, хто ще нічого не підписав. Але фільтр за видом
        // (відкидаємо ручні розсилки) діє завжди — тому раннього виходу тут
        // немає, лише окремі умови на кожен фільтр.
        //
        // Тема → мінімальна важливість.
        let mut wanted: HashMap<Topic, Severity> = HashMap::new();
        for s in &subs {
            wanted.insert(s.topic, Severity::parse(&s.min_severity));
        }

        let mut out = Vec::new();
        for a in all {
            // Ручні розсилки адміністратора — не ринкові сигнали.
            let kind = match AlertKind::parse(&a.kind) {
                Some(k) => k,
                None => continue,
            };
            let topic = Topic::from_str_code(kind.topic());

            // Фільтр за темою: якщо користувач підписався на «китів»,
            // ліквідації в дайджест не потрапляють.
            if !wanted.is_empty() {
                match wanted.get(&topic) {
                    Some(min) => {
                        if Severity::parse(&a.severity) < *min {
                            continue;
                        }
                    }
                    None => continue,
                }
            }

            // Фільтр за відстежуваними активами (якщо користувач обрав їх).
            if !assets.is_empty() {
                let symbol = a.asset.clone().unwrap_or_default().to_uppercase();
                if !assets.iter().any(|s| s.to_uppercase() == symbol) {
                    continue;
                }
            }

            out.push(a);
        }
        Ok(out)
    }
}
