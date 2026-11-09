//! Шлюз Telegram: поллінг оновлень, роутинг команд, ізоляція відповідей.
//!
//! Три правила, які тут закладені:
//!
//! 1. **Один полер на токен.** `409 Conflict` («terminated by other getUpdates
//!    request») означає, що на токені вже працює інший процес. Це фатальна
//!    помилка: мовчаливий retry-цикл лише сипав би помилки в лог.
//! 2. **Тільки приватні чати.** У групі або каналі бот не відповідає взагалі:
//!    відповідь у групі — це публікація приватних висновків, а сам факт
//!    присутності бота в групі розкриває, хто з групи підписаний на альерти.
//! 3. **Ідемпотентність оновлень.** Telegram повторно надсилає update, якщо не
//!    отримав підтвердження. `update_id` фіксується в сховищі, і команда
//!    виконується один раз.

pub mod digest;

use admin::AdminService;
use broadcast::{Notifier, Outbound, SendError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use storage::{Language, NewUser, Role};
use teloxide::errors::{ApiError, RequestError};
use teloxide::prelude::*;
use teloxide::sugar::request::RequestReplyExt;
use teloxide::types::{ChatKind, MessageId, PublicChatKind, Recipient, UpdateKind};
use user_session::Session;
use wr_client::Snapshot;

/// Поточний час у мс.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Тип чату.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatType {
    Private,
    Group,
    Supergroup,
    Channel,
}

/// Оновлення, зведене до потрібного нам вигляду.
#[derive(Clone, Debug)]
pub struct IncomingUpdate {
    pub update_id: i64,
    pub chat_id: i64,
    pub chat_type: ChatType,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub text: String,
    pub reply_to_message_id: Option<i64>,
}

/// Відповідь, яку треба надіслати.
#[derive(Clone, Debug)]
pub struct Reply {
    pub chat_id: i64,
    pub text: String,
    pub reply_to: Option<i64>,
}

/// Помилка поллера.
#[derive(Debug, thiserror::Error)]
pub enum PollerError {
    /// На токені вже працює інший полер — це не виправляється retry.
    #[error("конфлікт полерів Telegram: інший процес уже читає цей токен")]
    Conflict,
    #[error("тимчасова помилка Telegram: {0}")]
    Transient(String),
    #[error("некоректний токен: {0}")]
    InvalidToken(String),
}

/// Джерело оновлень.
pub trait UpdateSource {
    fn poll(&self, offset: i64) -> impl Future<Output = Result<Vec<IncomingUpdate>, PollerError>>;
}

/// Поллер на основі teloxide.
pub struct TeloxidePoller {
    bot: Bot,
    timeout_secs: u32,
}

impl TeloxidePoller {
    pub fn new(bot: Bot, timeout_secs: u32) -> TeloxidePoller {
        TeloxidePoller { bot, timeout_secs }
    }

    pub fn bot(&self) -> &Bot {
        &self.bot
    }
}

impl UpdateSource for TeloxidePoller {
    async fn poll(&self, offset: i64) -> Result<Vec<IncomingUpdate>, PollerError> {
        let req = self
            .bot
            .get_updates()
            .offset(offset as i32)
            .timeout(self.timeout_secs);
        let updates = req.send().await.map_err(classify_poll_error)?;
        Ok(updates.into_iter().filter_map(convert_update).collect())
    }
}

fn classify_poll_error(e: RequestError) -> PollerError {
    match e {
        RequestError::Api(ApiError::TerminatedByOtherGetUpdates) => PollerError::Conflict,
        RequestError::Api(ApiError::InvalidToken) => {
            PollerError::InvalidToken("Telegram не визнає токен".into())
        }
        RequestError::RetryAfter(secs) => PollerError::Transient(format!("flood: {secs} с")),
        other => PollerError::Transient(other.to_string()),
    }
}

fn convert_update(u: Update) -> Option<IncomingUpdate> {
    let msg = match u.kind {
        UpdateKind::Message(m) => m,
        _ => return None,
    };
    let chat_type = match &msg.chat.kind {
        ChatKind::Private(_) => ChatType::Private,
        ChatKind::Public(p) => match p.kind {
            PublicChatKind::Group => ChatType::Group,
            PublicChatKind::Supergroup(_) => ChatType::Supergroup,
            PublicChatKind::Channel(_) => ChatType::Channel,
        },
    };
    let text = msg.text().unwrap_or("").to_string();
    Some(IncomingUpdate {
        update_id: u.id.0 as i64,
        chat_id: msg.chat.id.0,
        chat_type,
        user_id: msg.from.as_ref().map(|f| f.id.0 as i64),
        username: msg.from.as_ref().and_then(|f| f.username.clone()),
        text,
        reply_to_message_id: msg.reply_to_message().map(|m| m.id.0 as i64),
    })
}

/// Відправник через teloxide.
pub struct TeloxideNotifier {
    bot: Bot,
}

impl TeloxideNotifier {
    pub fn new(bot: Bot) -> TeloxideNotifier {
        TeloxideNotifier { bot }
    }
}

impl Notifier for TeloxideNotifier {
    async fn send(&self, chat_id: i64, msg: &Outbound) -> Result<(), SendError> {
        let chat = Recipient::Id(ChatId(chat_id));
        let mut req = self.bot.send_message(chat, msg.text.clone());
        if let Some(reply_to) = msg.reply_to {
            req = req.reply_to(MessageId(reply_to as i32));
        }
        req.send().await.map(|_| ()).map_err(classify_send_error)
    }
}

fn classify_send_error(e: RequestError) -> SendError {
    match e {
        RequestError::RetryAfter(secs) => SendError::RateLimited {
            retry_after: secs.seconds() as u64,
        },
        RequestError::Api(ApiError::BotBlocked) => {
            SendError::Permanent("користувач заблокував бота".into())
        }
        RequestError::Api(ApiError::CantInitiateConversation) => {
            SendError::Permanent("користувач не починав розмову з ботом".into())
        }
        RequestError::Api(ApiError::InvalidToken) => {
            SendError::Permanent("некоректний токен".into())
        }
        RequestError::Api(ApiError::Unknown(d)) if d.contains("chat not found") => {
            SendError::Permanent("чат не знайдено".into())
        }
        other => SendError::Transient(other.to_string()),
    }
}

/// Спільна клітинка для поточного зрізу джерела.
#[derive(Default)]
pub struct SnapshotCell {
    inner: RwLock<Option<Snapshot>>,
}

impl SnapshotCell {
    pub fn new() -> Arc<SnapshotCell> {
        Arc::new(SnapshotCell {
            inner: RwLock::new(None),
        })
    }

    pub fn set(&self, snapshot: Snapshot) {
        if let Ok(mut guard) = self.inner.write() {
            *guard = Some(snapshot);
        }
    }

    pub fn get(&self) -> Option<Snapshot> {
        self.inner.read().ok().and_then(|g| g.clone())
    }
}

/// Лімітер вхідних команд: окремо на користувача, окремо глобально.
///
/// Фіксоване вікно в хвилину: достатньо для захисту від спаму й просто —
/// важлива не ідеальна точність, а те, що бот не дає перетворитися в
/// інструмент розсилки.
#[derive(Debug)]
pub struct InboundLimiter {
    per_user: std::sync::Mutex<HashMap<i64, (u32, i64)>>,
    global: std::sync::Mutex<(u32, i64)>,
    per_user_per_min: u32,
    admin_per_min: u32,
    global_per_min: u32,
}

impl InboundLimiter {
    pub fn new(per_user_per_min: u32, admin_per_min: u32, global_per_min: u32) -> InboundLimiter {
        InboundLimiter {
            per_user: std::sync::Mutex::new(HashMap::new()),
            global: std::sync::Mutex::new((0, 0)),
            per_user_per_min,
            admin_per_min,
            global_per_min,
        }
    }

    /// Чи можна виконати команду. `now` — epoch-мс.
    pub fn allow(&self, chat_id: i64, is_admin: bool, now: i64) -> bool {
        let minute = now / 60_000;
        let limit = if is_admin {
            self.admin_per_min
        } else {
            self.per_user_per_min
        };
        {
            let mut g = self.global.lock().unwrap_or_else(|e| e.into_inner());
            if g.1 != minute {
                *g = (0, minute);
            }
            if g.0 >= self.global_per_min {
                return false;
            }
            g.0 += 1;
        }
        let mut map = self.per_user.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.entry(chat_id).or_insert((0, minute));
        if entry.1 != minute {
            *entry = (0, minute);
        }
        if entry.0 >= limit {
            return false;
        }
        entry.0 += 1;
        true
    }
}

/// Роутер команд.
pub struct Router<S: Session, A: AdminService> {
    session: S,
    admin: A,
    snapshot: Arc<SnapshotCell>,
    default_language: String,
    access_code: Option<String>,
    admins: Vec<String>,
    limiter: Arc<InboundLimiter>,
    default_timezone: String,
}

/// Параметри роутера.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RouterConfig {
    pub default_language: String,
    pub access_code: Option<String>,
    pub admins: Vec<String>,
    /// Ліміти вхідних команд: (на користувача, для адміна, глобально).
    pub rate_limits: (u32, u32, u32),
    /// Часовий пояс за замовчуванням для нових користувачів.
    pub default_timezone: String,
}

impl<S: Session, A: AdminService> Router<S, A> {
    pub fn new(session: S, admin: A, snapshot: Arc<SnapshotCell>, cfg: RouterConfig) -> Self {
        Router {
            session,
            admin,
            snapshot,
            default_language: if cfg.default_language.is_empty() {
                "uk".to_string()
            } else {
                cfg.default_language.clone()
            },
            access_code: cfg.access_code.filter(|c| !c.is_empty()),
            admins: cfg
                .admins
                .iter()
                .map(|a| a.trim().trim_start_matches('@').to_lowercase())
                .filter(|a| !a.is_empty())
                .collect(),
            limiter: Arc::new(InboundLimiter::new(
                cfg.rate_limits.0,
                cfg.rate_limits.1,
                cfg.rate_limits.2,
            )),
            default_timezone: if cfg.default_timezone.is_empty() {
                "UTC".to_string()
            } else {
                cfg.default_timezone.clone()
            },
        }
    }

    fn is_admin(&self, username: Option<&str>) -> bool {
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

    /// Обробити оновлення. `None` — відповіді не потрібно.
    pub async fn handle(&self, upd: &IncomingUpdate) -> Option<Reply> {
        // Групи й канали — поза зоною бота.
        if upd.chat_type != ChatType::Private {
            tracing::debug!(
                chat_id = upd.chat_id,
                "update не з приватного чату — ігнорую"
            );
            return None;
        }
        if upd.text.trim().is_empty() {
            return None;
        }

        // Ліміт команд: і на користувача, і на бота загалом.
        let is_admin = self.is_admin(upd.username.as_deref());
        if !self.limiter.allow(upd.chat_id, is_admin, now_ms()) {
            tracing::debug!(chat_id = upd.chat_id, "команду пропущено: ліміт частоти");
            return Some(self.reply(upd, "Забагато запитів. Зачекайте трохи.".to_string()));
        }

        let text = upd.text.trim();
        let (head, rest) = match text.split_once(char::is_whitespace) {
            Some((h, r)) => (h.trim().to_lowercase(), r.trim().to_string()),
            None => (text.to_lowercase(), String::new()),
        };

        if !head.starts_with('/') {
            // Не команда — підказка, що вміємо.
            return Some(self.reply(upd, self.hint(&self.default_language)));
        }

        match head.trim_start_matches('/') {
            "start" => Some(self.reply(upd, self.cmd_start(upd, &rest).await)),
            "help" => Some(self.reply(upd, self.help(&self.default_language))),
            "settings" => Some(self.reply(upd, self.cmd_settings(upd).await)),
            "mute" => Some(self.reply(upd, self.cmd_mute(upd, true).await)),
            "unmute" => Some(self.reply(upd, self.cmd_mute(upd, false).await)),
            "language" | "lang" => Some(self.reply(upd, self.cmd_language(upd, &rest).await)),
            "assets" => Some(self.reply(upd, self.cmd_assets(&self.default_language))),
            "groups" => Some(self.reply(upd, self.cmd_groups(&self.default_language))),
            "signal" => Some(self.reply(upd, self.cmd_signal(&self.default_language, &rest))),
            "digest" => Some(self.reply(upd, self.cmd_digest(upd, &rest).await)),
            "delete" => Some(self.reply(upd, self.cmd_delete(upd).await)),
            "stats" if self.is_admin(upd.username.as_deref()) => {
                Some(self.reply(upd, self.cmd_stats().await))
            }
            "users" if self.is_admin(upd.username.as_deref()) => {
                Some(self.reply(upd, self.cmd_users().await))
            }
            "broadcast" if self.is_admin(upd.username.as_deref()) => {
                Some(self.reply(upd, self.cmd_broadcast(upd, &rest).await))
            }
            "block" if self.is_admin(upd.username.as_deref()) => {
                Some(self.reply(upd, self.cmd_block(upd, &rest, true).await))
            }
            "unblock" if self.is_admin(upd.username.as_deref()) => {
                Some(self.reply(upd, self.cmd_block(upd, &rest, false).await))
            }
            _ => Some(self.reply(
                upd,
                format!(
                    "Невідома команда: {head}\n{}",
                    self.hint(&self.default_language)
                ),
            )),
        }
    }

    fn reply(&self, upd: &IncomingUpdate, text: String) -> Reply {
        Reply {
            chat_id: upd.chat_id,
            text,
            reply_to: upd.reply_to_message_id,
        }
    }

    async fn cmd_start(&self, upd: &IncomingUpdate, rest: &str) -> String {
        // Код підключення: якщо заданий — без нього не підключитися.
        if let Some(code) = &self.access_code {
            let ok = !rest.is_empty() && rest == code;
            if !ok {
                return if rest.is_empty() {
                    "Для підключення потрібен код: /start <код>".to_string()
                } else {
                    "Код невірний.".to_string()
                };
            }
        }

        let user = self
            .session
            .register(NewUser {
                chat_id: upd.chat_id,
                user_id: upd.user_id,
                username: upd.username.clone(),
                language: Language::from_str_code(&self.default_language),
                timezone: self.default_timezone.clone(),
                role: Role::Viewer,
            })
            .await;

        match user {
            Ok(_) => format!(
                "Привіт 👋 Я стежу за ринковими сигналами китів.\n\n{}\n{}",
                self.hint(&self.default_language),
                "Налаштування: /settings · тиша: /mute · мова: /language uk|en"
            ),
            Err(e) => format!("Не вдалося зареєструвати: {e}"),
        }
    }

    async fn cmd_settings(&self, upd: &IncomingUpdate) -> String {
        match self.session.settings(upd.chat_id).await {
            Ok(s) => format!(
                "⚙️ Налаштування\n• мова: {}\n• часовий пояс: {}\n• таймфрейм: {}\n• тиша: {}",
                s.language.as_str(),
                s.timezone,
                s.timeframe.as_str(),
                if s.muted {
                    "увімкнено"
                } else {
                    "вимкнено"
                }
            ),
            Err(e) => format!("Не вдалося прочитати налаштування: {e}"),
        }
    }

    async fn cmd_mute(&self, upd: &IncomingUpdate, muted: bool) -> String {
        match self.session.set_muted(upd.chat_id, muted).await {
            Ok(()) => {
                if muted {
                    "🔇 Алерти призупинено. Повернути: /unmute".to_string()
                } else {
                    "🔔 Алерти знову приходять.".to_string()
                }
            }
            Err(e) => format!("Не вдалося змінити налаштування: {e}"),
        }
    }

    async fn cmd_language(&self, upd: &IncomingUpdate, rest: &str) -> String {
        let code = rest.trim().to_lowercase();
        let lang = match code.as_str() {
            "uk" => Language::Uk,
            "en" => Language::En,
            _ => return "Мова: /language uk або /language en".to_string(),
        };
        match self.session.set_language(upd.chat_id, lang).await {
            Ok(()) => format!("Мова: {}", lang.as_str()),
            Err(e) => format!("Не вдалося змінити мову: {e}"),
        }
    }

    fn cmd_assets(&self, lang: &str) -> String {
        let snap = match self.snapshot.get() {
            Some(s) => s,
            None => return "Джерело зараз недоступне — спробуйте за хвилину.".to_string(),
        };
        let mut out = String::from("📊 Активи з сигналами\n");
        let mut rows: Vec<_> = snap.whale_assets.iter().collect();
        rows.sort_by(|a, b| {
            b.top10_pct
                .partial_cmp(&a.top10_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for a in rows.iter().take(15) {
            out.push_str(&format!(
                "• {} — top-10 {}% · top-50 {}%\n",
                a.symbol,
                a.top10_pct.round() as i64,
                a.top50_pct.round() as i64
            ));
        }
        if rows.is_empty() {
            out.push_str("• сигналів зараз немає\n");
        }
        out.push_str(&format!(
            "\n{}",
            if lang.starts_with("en") {
                "Data as of snapshot time."
            } else {
                "Дані на момент зрізу."
            }
        ));
        out
    }

    fn cmd_groups(&self, lang: &str) -> String {
        let snap = match self.snapshot.get() {
            Some(s) => s,
            None => return "Джерело зараз недоступне — спробуйте за хвилину.".to_string(),
        };
        let mut out = String::from("🧬 Капітал-групи\n");
        for g in snap.groups.iter().take(15) {
            let assets = g.common_assets.join(", ");
            out.push_str(&format!(
                "• група на {} учасників · спільні активи: {}\n",
                g.member_count,
                if assets.is_empty() { "—" } else { &assets }
            ));
        }
        if snap.groups.is_empty() {
            out.push_str("• груп зараз не виявлено\n");
        }
        out.push_str(&format!(
            "\n{}",
            if lang.starts_with("en") {
                "Addresses and transactions are never shown."
            } else {
                "Адреси й транзакції не показуються ніколи."
            }
        ));
        out
    }

    fn cmd_signal(&self, lang: &str, rest: &str) -> String {
        let sym = rest.trim().trim_start_matches('/').to_uppercase();
        if sym.is_empty() {
            return "Формат: /signal <SYMBOL>, наприклад /signal SOL".to_string();
        }
        let snap = match self.snapshot.get() {
            Some(s) => s,
            None => return "Джерело зараз недоступне — спробуйте за хвилину.".to_string(),
        };
        let mut found = false;
        let mut out = format!("🔎 {sym}\n");
        for a in snap
            .whale_assets
            .iter()
            .filter(|a| a.symbol.eq_ignore_ascii_case(&sym))
        {
            found = true;
            out.push_str(&format!(
                "• концентрація: top-10 {}% · top-50 {}%\n",
                a.top10_pct.round() as i64,
                a.top50_pct.round() as i64
            ));
            if let Some(sector) = &a.sector {
                out.push_str(&format!("• сектор: {sector}\n"));
            }
        }
        for s in snap
            .signals
            .iter()
            .filter(|s| s.phase == wr_client::Phase::Accumulating)
        {
            found = true;
            out.push_str(&format!(
                "• капітал-група: {} учасників · обсяг ${}\n",
                s.size,
                s.volume_usd.round() as i64
            ));
            if let Some(te) = s.top_emission_pct {
                out.push_str(&format!("• частка емісії: {te}%\n"));
            }
        }
        if !found {
            out.push_str("• актив не знайдено у поточному зрізі\n");
        }
        out.push_str(&format!(
            "\n{}",
            if lang.starts_with("en") {
                "Aggregates only — no wallet addresses, no transactions."
            } else {
                "Тільки агрегати — без адрес гаманців і без транзакцій."
            }
        ));
        out
    }

    /// Дайджест за період: добірка алертів, а не зріз стану.
    ///
    /// Мова — мова користувача, а не мова за замовчуванням: дайджест
    /// персональний, тому й мова в ньому персональна.
    async fn cmd_digest(&self, upd: &IncomingUpdate, rest: &str) -> String {
        let period = digest::DigestPeriod::parse(rest);
        let now = now_ms();

        // Мова користувача.
        let lang = match self.session.settings(upd.chat_id).await {
            Ok(s) => s.language.as_str().to_string(),
            Err(_) => self.default_language.clone(),
        };

        // Алерти за вікно, вже відфільтровані під користувача.
        let records = match self
            .session
            .alerts_since(upd.chat_id, now - period.window_ms())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "не вдалося прочитати алерти для дайджесту");
                return if lang.starts_with("en") {
                    "Could not read the alert history — try again in a minute.".to_string()
                } else {
                    "Не вдалося прочитати історію алертів — спробуйте за хвилину.".to_string()
                };
            }
        };

        let summary = digest::Digest::build(&records, period, now);
        summary.render(&lang)
    }

    async fn cmd_delete(&self, upd: &IncomingUpdate) -> String {
        let user_id = match upd.user_id {
            Some(id) => id,
            None => return "Не вдалося визначити користувача.".to_string(),
        };
        match self.session.purge(user_id).await {
            Ok(p) => format!(
                "🗑 Дані видалено: профілів {}, доставок {}, записів аудиту {}.",
                p.users, p.deliveries, p.audits
            ),
            Err(e) => format!("Не вдалося видалити дані: {e}"),
        }
    }

    async fn cmd_stats(&self) -> String {
        match self.admin.stats().await {
            Ok(s) => format!(
                "📈 Статистика\n• підключено: {}\n• алертів: {} (дублів відкинуто: {})\n• надіслано: {} · помилок: {} · ретраїв: {} · пропущено: {}",
                s.recipients,
                s.alerts_published,
                s.alerts_duplicate,
                s.messages_sent,
                s.messages_failed,
                s.messages_retried,
                s.messages_skipped
            ),
            Err(e) => format!("Не вдалося зібрати статистику: {e}"),
        }
    }

    async fn cmd_users(&self) -> String {
        match self.admin.stats().await {
            Ok(s) => format!("👥 Підключено: {}", s.recipients),
            Err(e) => format!("Не вдалося: {e}"),
        }
    }

    async fn cmd_broadcast(&self, upd: &IncomingUpdate, rest: &str) -> String {
        if rest.trim().is_empty() {
            return "Формат: /broadcast <текст>".to_string();
        }
        match self
            .admin
            .broadcast_manual(upd.chat_id, upd.username.as_deref(), rest.trim())
            .await
        {
            Ok(n) => format!("📣 Надіслано {n} підписникам."),
            Err(e) => format!("Не вдалося розіслати: {e}"),
        }
    }

    async fn cmd_block(&self, upd: &IncomingUpdate, rest: &str, blocked: bool) -> String {
        let target: i64 = match rest.trim().parse() {
            Ok(v) => v,
            Err(_) => return "Формат: /block <chat_id>".to_string(),
        };
        match self.admin.block(upd.chat_id, target, blocked).await {
            Ok(()) => {
                if blocked {
                    format!("Користувача {target} заблоковано.")
                } else {
                    format!("Користувача {target} розблоковано.")
                }
            }
            Err(e) => format!("Не вдалося: {e}"),
        }
    }

    fn hint(&self, _lang: &str) -> String {
        "Команди: /assets · /groups · /signal SYMBOL · /digest daily|weekly · /settings · /mute · /language uk|en"
            .to_string()
    }

    fn help(&self, lang: &str) -> String {
        if lang.starts_with("en") {
            "📡 Wallet Radar Alerts\n\nWhat I can do:\n• /assets — assets with whale concentration\n• /groups — capital groups (aggregates only)\n• /signal SYMBOL — current aggregate signal for an asset\n• /digest daily|weekly — alerts from the last day or week\n• /settings — language, timezone, timeframe\n• /mute / /unmute — pause alerts\n• /language uk|en\n• /delete — delete my data\n\nWhat I will never show: wallet addresses, tx hashes, individual transaction amounts."
                .to_string()
        } else {
            "📡 Wallet Radar Alerts\n\nЩо я вмію:\n• /assets — активи з концентрацією китів\n• /groups — капітал-групи (лише агрегати)\n• /signal SYMBOL — поточний агрегат по активу\n• /digest daily|weekly — алерти за добу або тиждень\n• /settings — мова, часовий пояс, таймфрейм\n• /mute / /unmute — тиша\n• /language uk|en\n• /delete — видалити мої дані\n\nЧого я не покажу ніколи: адреси гаманців, tx-hash, суми окремих транзакцій."
                .to_string()
        }
    }
}
