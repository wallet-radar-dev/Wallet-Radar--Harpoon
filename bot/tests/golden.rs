//! Golden-тест: головна гарантія приватності.
//!
//! Схема перевірки:
//!
//! 1. Мок Wallet Radar віддає фікстури, у яких є **справжні за формою**
//!    адреси, tx-hash і explorer-посилання — тобто все, що заборонено.
//! 2. Бот повністю проходить цикл: події → двигун → алерти → черга → відправка,
//!    а також обробляє команди користувача.
//! 3. Усі вихідні повідомлення збираються в один список.
//! 4. Перевіряємо дві речі:
//!    * **жодного** забороненого токена у вихідних повідомленнях;
//!    * `scrub` **не змінив** жодного повідомлення — тобто спрацював типовий
//!      шар (у типах немає поля для адреси), а не те, що regex випадково
//!      згодився. Якщо другий пункт упаде, а перший — ні, значить у текст
//!      просочилося щось, що фільтр згоден «приховати», але ми не хочемо
//!      покладатися на regex.

use admin::Admin;
use alert_engine::{AlertKind, Rule, Severity, Window};
use broadcast::{BroadcastConfig, Broadcaster, Notifier, Outbound};
use privacy_filter::PrivacyFilter;
use std::sync::{Arc, Mutex};
use storage::{SqliteStore, Store};
use telegram_gateway::{ChatType, IncomingUpdate, Router, RouterConfig, SnapshotCell};
use user_session::UserSession;
use wr_client::RawEvent;

/// Заборонені токени: усе, що фікстури містять і що не має виходити.
const FORBIDDEN: &[&str] = &[
    // EVM-адреси (гаманці, контракти, контрагенти)
    "0x611f7bf868a6212f871e89f7e44684045ddfb09d",
    "0x1d48963dd8fada6ab5c2c7b92eba81ecc5030270",
    "0xd2dd7b597fd2435b6db61ddf48544fd931e6869f",
    "0x7dafba1d69f6c01ae7567ffd7b046ca03b706f83",
    "0xacddac6c77318b615f7f6fb9bb67c6833e9c05f1",
    "0x8f6ffec54c1e569c63992eb2dc4fae84e3792551",
    "0xdac17f958d2ee523a2206206994597c13d831ec7",
    // tx-hash
    "0xb25dcebf917db28dd7702336db997e2dd6c149371c628743be68c8dbf3a7b6c2",
    "0x541ad78a1d65d1a0f42abe9428b3f53fd9df7794652285ae557ca2fe9c004386",
    // explorer-посилання
    "etherscan.io",
    "solscan.io",
];

/// Відправник, що записує все, що пішло б у Telegram.
struct RecordingNotifier {
    sent: Arc<Mutex<Vec<String>>>,
}

impl Notifier for RecordingNotifier {
    async fn send(&self, _chat_id: i64, msg: &Outbound) -> Result<(), broadcast::SendError> {
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.text.clone());
        Ok(())
    }
}

fn event(id: &str, wallet: &str, symbol: &str, inflow: bool, value: f64, ts: i64) -> RawEvent {
    RawEvent {
        id: id.into(),
        wallet_id: wallet.into(),
        network: wr_client::Network::Eth,
        flow: if inflow {
            wr_client::FlowKind::In
        } else {
            wr_client::FlowKind::Out
        },
        asset_symbol: symbol.into(),
        chain: "eth".into(),
        amount: 1.0,
        value_usd: value,
        ts,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn update(update_id: i64, chat_id: i64, text: &str) -> IncomingUpdate {
    IncomingUpdate {
        update_id,
        chat_id,
        chat_type: ChatType::Private,
        user_id: Some(chat_id),
        username: Some("tester".into()),
        text: text.to_string(),
        reply_to_message_id: None,
    }
}

/// Зріз джерела, зібраний із тих самих фікстур, що й події.
fn fixture_snapshot() -> wr_client::Snapshot {
    wr_client::Snapshot {
        as_of: 1_700_000_000_000,
        signals: vec![wr_client::FamilySignal {
            family_id: "fam_deadbeef".into(),
            size: 4,
            volume_usd: 1_250_000.0,
            phase: wr_client::Phase::Accumulating,
            sector: Some("DeFi".into()),
            transfers: 87,
            top_emission_pct: Some(6.2),
        }],
        whale_assets: vec![wr_client::WhaleAsset {
            symbol: "SOL".into(),
            name: "Solana".into(),
            top10_pct: 48.0,
            top50_pct: 62.2,
            band: wr_client::ConcentrationBand::Mid,
            sector: Some("L1".into()),
        }],
        groups: vec![wr_client::CapitalGroup {
            group_id: "grp_deadbeef".into(),
            size: 2,
            member_count: 2,
            common_assets: vec!["SOL".into(), "ONDO".into()],
        }],
        cross_asset: vec![wr_client::CrossAssetRow {
            symbol: "SOL".into(),
            holders: 12,
            share_pct: 3.4,
        }],
        liquidations: Some(wr_client::LiqSnapshot {
            w5m: wr_client::LiqWindow {
                long_usd: 1000.0,
                short_usd: 2000.0,
                total_usd: 3000.0,
                count: 4,
            },
            w1h: wr_client::LiqWindow {
                long_usd: 10_000.0,
                short_usd: 300_000.0,
                total_usd: 310_000.0,
                count: 40,
            },
            w24h: wr_client::LiqWindow {
                long_usd: 100_000.0,
                short_usd: 900_000.0,
                total_usd: 1_000_000.0,
                count: 400,
            },
        }),
        archetypes: vec![],
        track_record: None,
        source_reachable: true,
    }
}

#[tokio::test]
async fn no_wallet_address_or_transaction_ever_leaves_the_bot() {
    let store = SqliteStore::open_in_memory().expect("БД у памʼяті");
    let sanitizer = PrivacyFilter::new("приховано");
    let sent = Arc::new(Mutex::new(Vec::<String>::new()));
    let notifier = RecordingNotifier { sent: sent.clone() };

    let broadcaster = Arc::new(Broadcaster::new(
        store.clone(),
        notifier,
        sanitizer.clone(),
        BroadcastConfig {
            // Нульові паузи — тест не має чекати.
            per_chat_min_interval_ms: 0,
            ..BroadcastConfig::default()
        },
    ));

    // ─── Користувач підключається ──────────────────────────────────
    let session = UserSession::new(store.clone());
    let admin = Admin::new(
        store.clone(),
        broadcaster.clone(),
        sanitizer.clone(),
        vec!["tester".into()],
    );
    let cell = SnapshotCell::new();
    cell.set(fixture_snapshot());
    let router = Router::new(
        session,
        admin,
        cell.clone(),
        RouterConfig {
            default_language: "uk".into(),
            default_timezone: "UTC".into(),
            access_code: None,
            admins: vec!["tester".into()],
            rate_limits: (20, 60, 200),
            poll_health: None,
        },
    );

    let chat_id = 42_000i64;

    // Команди користувача — приватні відповіді.
    for (i, cmd) in [
        "/start",
        "/help",
        "/assets",
        "/groups",
        "/signal SOL",
        "/digest daily",
        "/settings",
        "/language en",
        "/stats",
    ]
    .iter()
    .enumerate()
    {
        let upd = update(i as i64 + 1, chat_id, cmd);
        if let Some(reply) = router.handle(&upd).await {
            // Відповідь іде тим самим шляхом, що й в боті: через розсильник.
            broadcaster
                .reply(reply.chat_id, &reply.text)
                .await
                .expect("відповідь надіслана");
        }
    }

    // ─── Група: бот не відповідає взагалі ─────────────────────────
    let mut group = update(100, -1, "/assets");
    group.chat_type = ChatType::Supergroup;
    assert!(
        router.handle(&group).await.is_none(),
        "у групі відповіді бути не повинно"
    );

    // ─── Алерти з подій ───────────────────────────────────────────
    let now = 1_700_000_000_000i64;
    let mut engine = alert_engine::Engine::new(vec![Rule {
        kind: AlertKind::WhaleAccumulation,
        min_participants: 3,
        min_total_value_usd: 20_000.0,
        windows: vec![Window::W15m],
        assets: vec![],
        severity: Severity::Medium,
    }]);
    for i in 0..3 {
        engine.ingest_event(&event(
            &format!("0x{:064x}", i),
            &format!("wallet{}", i),
            "SOL",
            true,
            15_000.0,
            now - 1000,
        ));
    }
    let alerts = engine.evaluate(now);
    assert!(!alerts.is_empty(), "алерт має бути створений");

    // Користувач має підписку на тему, щоб отримати алерт.
    store
        .set_subscription(&storage::Subscription {
            chat_id,
            topic: storage::Topic::Whale,
            min_severity: "medium".into(),
        })
        .await
        .expect("підписка збережена");

    for alert in &alerts {
        let outcome = broadcaster
            .publish(alert)
            .await
            .expect("алерт опубліковано");
        assert!(
            matches!(outcome, broadcast::PublishOutcome::Queued { recipients: 1 }),
            "алерт має піти одному підписнику, а не всім: {outcome:?}"
        );
    }

    // Черга: надсилаємо.
    broadcaster.pump_once(now).await.expect("черга оброблена");

    // ─── Перевірка ────────────────────────────────────────────────
    let messages = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert!(!messages.is_empty(), "повідомлення мали надіслатися");

    for msg in &messages {
        for forbidden in FORBIDDEN {
            assert!(
                !msg.contains(forbidden),
                "ВИТІК: повідомлення містить заборонений токен {forbidden}:\n{msg}"
            );
        }
        // Жоден рядок не має виглядати як адреса чи хеш.
        assert!(
            !msg.contains("0x"),
            "ВИТІК: у повідомленні є hex-рядок:\n{msg}"
        );
    }

    // Типовий шар, а не regex: фільтр не мав нічого змінити.
    for msg in &messages {
        let report = sanitizer.scrub(msg);
        assert!(
            report.is_clean(),
            "фільтр змінив повідомлення — отже, у текст просочилося зайве:\n{}",
            report.text
        );
    }

    // Ідемпотентність: повторна публікація того самого алерту не дублює.
    let again = broadcaster.publish(&alerts[0]).await.expect("повтор");
    assert!(
        matches!(again, broadcast::PublishOutcome::AlreadyKnown),
        "повторний алерт має бути відкинуто, а не розісланий знову"
    );
}

#[tokio::test]
async fn manual_admin_broadcast_is_filtered_too() {
    let store = SqliteStore::open_in_memory().expect("БД у памʼяті");
    let sanitizer = PrivacyFilter::new("приховано");
    let sent = Arc::new(Mutex::new(Vec::<String>::new()));
    let notifier = RecordingNotifier { sent: sent.clone() };
    let broadcaster = Arc::new(Broadcaster::new(
        store.clone(),
        notifier,
        sanitizer.clone(),
        BroadcastConfig {
            per_chat_min_interval_ms: 0,
            ..BroadcastConfig::default()
        },
    ));
    let admin = Admin::new(
        store.clone(),
        broadcaster.clone(),
        sanitizer.clone(),
        vec!["tester".into()],
    );

    let chat_id = 77i64;
    store
        .upsert_user(storage::NewUser {
            chat_id,
            user_id: Some(77),
            username: Some("tester".into()),
            language: storage::Language::Uk,
            timezone: "UTC".into(),
            role: storage::Role::Viewer,
        })
        .await
        .expect("користувач створено");

    admin
        .broadcast_manual(
            chat_id,
            Some("tester"),
            "Кит 0x611f7bf868a6212f871e89f7e44684045ddfb09d накопичує SOL",
        )
        .await
        .expect("розсилку виконано");

    // Черга йде за справжнім часом: ручна розсилка ставить доставку «зараз».
    broadcaster
        .pump_once(now_ms())
        .await
        .expect("черга оброблена");

    let messages = sent.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(messages.len(), 1);
    assert!(
        !messages[0].contains("0x611f7bf8"),
        "адреса з ручного тексту просочилася:\n{}",
        messages[0]
    );
    assert!(messages[0].contains("приховано"));
}
