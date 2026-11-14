//! Одноразова розсилка оголошення всім активним підписникам.
//!
//! Інструмент адміністрування: пише ручний алерт у чергу доставок — так само,
//! як команда `/broadcast`, але без залежності від сесії адміністратора в
//! Telegram. Повідомлення проходить **фільтр приватності** і потрапляє в аудит.
//!
//! Надсилає не сам: доставки забирає воркер запущеного сервісу — отже,
//! ліміти Telegram, ретраї та ідемпотентність працюють ті самі.
//!
//! Використання:
//! ```text
//! send-notice <сховище> <admin_chat_id> <admin_username> <файл із текстом>
//! ```
//!
//! `<сховище>` — шлях до файлу SQLite (`/opt/wallet-radar-alerts-bot/data/bot.db`)
//! або URL бази Postgres (`postgres://…`). Бот на проді працює на Postgres,
//! тож інструмент має писати в ту саму чергу, яку читає воркер сервісу.

use admin::Admin;
use broadcast::{BroadcastConfig, Broadcaster, Notifier, Outbound};
use privacy_filter::PrivacyFilter;
use std::sync::Arc;
use storage::{PostgresStore, SqliteStore, Store};

/// Заглушка: надсилає саме воркер сервісу, а не цей інструмент.
struct DeferredNotifier;

impl Notifier for DeferredNotifier {
    async fn send(&self, _chat_id: i64, _msg: &Outbound) -> Result<(), broadcast::SendError> {
        // Надсилає воркер сервісу, який читає ту саму чергу.
        Ok(())
    }
}

/// Основна робота — одна й та сама для будь-якого бекенда сховища.
async fn run<S>(
    store: S,
    admin_chat_id: i64,
    admin_username: &str,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: Store + Clone + Send + Sync + 'static,
{
    let sanitizer = PrivacyFilter::new("приховано");
    let broadcaster = Arc::new(Broadcaster::new(
        store.clone(),
        DeferredNotifier,
        sanitizer.clone(),
        BroadcastConfig::default(),
    ));
    let admin = Admin::new(
        store,
        broadcaster,
        sanitizer,
        vec![admin_username.to_string()],
    );

    let queued = admin
        .broadcast_manual(admin_chat_id, Some(admin_username), text)
        .await?;

    println!("поставлено в чергу доставок: {queued}");
    if queued == 0 {
        println!(
            "нічого не поставлено: або немає активних підписників, або такий самий \
             текст уже був розісланий у цю хвилину"
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let target = args.next().ok_or(
        "вкажи сховище: send-notice <шлях до bot.db | postgres://…> <admin_chat_id> \
         <admin_username> <файл>",
    )?;
    let admin_chat_id: i64 = args
        .next()
        .ok_or("вкажи chat_id адміністратора")?
        .parse()
        .map_err(|_| "chat_id адміністратора має бути числом")?;
    let admin_username = args.next().ok_or("вкажи юзернейм адміністратора")?;
    let text_path = args.next().ok_or("вкажи шлях до файлу з текстом")?;

    let raw = std::fs::read_to_string(&text_path)?;
    let text = raw.trim();

    // Фільтр приватності — навіть ручний текст не має витікати.
    let sanitizer = PrivacyFilter::new("приховано");
    let report = sanitizer.scrub(text);
    if !report.is_clean() {
        println!(
            "увага: фільтр змінив {} фрагментів тексту (заборонені токени)",
            report.count()
        );
    }

    // Сховище: SQLite-файл або Postgres-URL. Бекенд визначається за виглядом
    // аргументу — так інструмент працює з тим самим станом, з якого читає
    // воркер сервісу.
    if target.contains("://") {
        let store = PostgresStore::connect(&target, &[], 2).await?;
        run(store, admin_chat_id, &admin_username, &report.text).await
    } else {
        let store = SqliteStore::open(&target)?;
        run(store, admin_chat_id, &admin_username, &report.text).await
    }
}
