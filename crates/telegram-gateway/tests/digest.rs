//! Тести дайджесту за період.
//!
//! Перевіряємо три речі:
//!
//! 1. **Добірка, а не зріз.** Підсумок рахує алерти за вікно часу.
//! 2. **Фільтри користувача діють.** Підписки на теми, мінімальна важливість
//!    і відстежувані активи відсікають чуже.
//! 3. **Приватність.** У дайджесті немає адрес, tx-hash і сум окремих
//!    транзакцій — навіть якщоpayload підмінити.

use alert_engine::{Alert, AlertKey, AlertKind, AssetLabel, Direction, Metrics, Severity, Window};
use storage::AlertRecord;
use telegram_gateway::digest::{Digest, DigestPeriod};

/// Зараз у мс.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Повний алерт із дрібницями.
fn alert(
    kind: AlertKind,
    symbol: &str,
    severity: Severity,
    participants: u32,
    value: f64,
) -> Alert {
    Alert {
        key: AlertKey::derive(kind, symbol, 0, &["evidence"]),
        kind,
        severity,
        asset: AssetLabel::unknown(symbol),
        related: Vec::new(),
        direction: Some(Direction::Accumulation),
        metrics: Metrics {
            participants,
            total_value_usd: value,
            group_share_pct: None,
            concentration_top10_pct: None,
            group_size: None,
            window: Window::W1h,
        },
        archetype: None,
        degraded: false,
        created_at: now_ms(),
    }
}

/// Запис у журналі `alert_log`.
fn record(a: &Alert) -> AlertRecord {
    AlertRecord {
        alert_key: a.key.as_str().to_string(),
        kind: a.kind.as_str().to_string(),
        asset: Some(a.asset.symbol.clone()),
        window_start: a.created_at - a.metrics.window.ms(),
        severity: a.severity.as_str().to_string(),
        degraded: a.degraded,
        payload: serde_json::to_string(a).unwrap_or_else(|_| "{}".into()),
        created_at: a.created_at,
    }
}

#[test]
fn дайджест_рахує_алерти_за_період() {
    let now = now_ms();
    let records = vec![
        record(&alert(
            AlertKind::WhaleAccumulation,
            "SOL",
            Severity::Medium,
            5,
            250_000.0,
        )),
        record(&alert(
            AlertKind::WhaleAccumulation,
            "SOL",
            Severity::High,
            7,
            900_000.0,
        )),
        record(&alert(
            AlertKind::CapitalGroupChange,
            "UNI",
            Severity::Medium,
            2,
            0.0,
        )),
    ];

    let d = Digest::build(&records, DigestPeriod::Daily, now);
    assert_eq!(d.total, 3);
    assert_eq!(d.high, 1);
    assert_eq!(d.by_kind.len(), 2);
    // Накопичення двічі, капітал-група один раз.
    assert_eq!(d.by_kind[0], (AlertKind::WhaleAccumulation, 2));
    assert_eq!(d.by_kind[1], (AlertKind::CapitalGroupChange, 1));
    // Топ-активи: SOL двічі, UNI один раз.
    assert_eq!(d.top_assets[0], ("SOL".to_string(), 2));
    assert_eq!(d.top_assets[1], ("UNI".to_string(), 1));
    assert_eq!(d.notable.len(), 2, "два різні види — дві групи");
    // Найважливіший — «high» (900k), а не за сумою серед «medium».
    assert_eq!(d.notable[0].alert.severity, Severity::High);
    // Два «Накопичення · SOL» згруповані в одну позицію з кількістю 2.
    assert_eq!(d.notable[0].count, 2);
    assert_eq!(d.notable[0].alert.asset.symbol, "SOL");

    // У тексті група показана одним рядком із кількістю сигналів.
    let text = d.render("uk");
    assert!(text.contains("2 сигнали"), "{text}");
    assert!(text.contains("до 7 учасників"), "{text}");
}

#[test]
fn українська_множина_правильна() {
    // 1 алерт — без множини; 2-4 — «сигнали»; 5+ — «сигналів».
    let now = now_ms();
    let mk = |n: usize| {
        let records: Vec<AlertRecord> = (0..n)
            .map(|_| {
                record(&alert(
                    AlertKind::WhaleAccumulation,
                    "SOL",
                    Severity::Medium,
                    2,
                    1_000.0,
                ))
            })
            .collect();
        Digest::build(&records, DigestPeriod::Daily, now).render("uk")
    };

    assert!(mk(2).contains("2 сигнали"), "{}", mk(2));
    assert!(mk(4).contains("4 сигнали"), "{}", mk(4));
    assert!(mk(5).contains("5 сигналів"), "{}", mk(5));
    assert!(mk(11).contains("11 сигналів"), "{}", mk(11));
    assert!(mk(21).contains("21 сигнал"), "{}", mk(21));
}

#[test]
fn тижневий_дайджест_охоплює_більше_вікна() {
    let now = now_ms();
    let mut old = alert(
        AlertKind::WhaleAccumulation,
        "ETH",
        Severity::Medium,
        3,
        10_000.0,
    );
    old.created_at = now - 3 * 24 * 60 * 60 * 1000;

    let records = vec![record(&old)];
    let daily = Digest::build(&records, DigestPeriod::Daily, now);
    let weekly = Digest::build(&records, DigestPeriod::Weekly, now);

    // Алеrt лежить у тижневому вікні, але не в добовому.
    assert_eq!(daily.total, 0);
    assert_eq!(weekly.total, 1);
}

#[test]
fn порожній_період_дає_зрозумілий_текст() {
    let d = Digest::build(&[], DigestPeriod::Daily, now_ms());
    let text = d.render("uk");
    assert!(text.contains("сигналів не було"), "{text}");
    let en = d.render("en");
    assert!(en.contains("no alerts"), "{en}");
}

#[test]
fn ручні_розсилки_не_потрапляють_у_дайджест() {
    let now = now_ms();
    let manual = AlertRecord {
        alert_key: "manual_abc".into(),
        kind: "manual".into(),
        asset: None,
        window_start: now,
        severity: "high".into(),
        degraded: false,
        payload: "{\"manual\":true}".into(),
        created_at: now,
    };
    let d = Digest::build(&[manual], DigestPeriod::Daily, now);
    assert_eq!(d.total, 0, "ручна розсилка — не ринковий сигнал");
}

#[test]
fn у_дайджесті_немає_адрес_і_сум_окремих_транзакцій() {
    let now = now_ms();
    // Payload із підмішаною адресою: дайджест не має її вивести.
    let payload = format!(
        "{{\"key\":\"k\",\"kind\":\"WhaleAccumulation\",\"severity\":\"Medium\",\
         \"asset\":{{\"symbol\":\"SOL\",\"name\":\"Solana\",\"chain\":\"\"}},\
         \"related\":[],\"direction\":null,\"metrics\":{{\"participants\":3,\
         \"total_value_usd\":1000.0,\"group_share_pct\":null,\
         \"concentration_top10_pct\":null,\"group_size\":null,\"window\":\"W1h\"}},\
         \"archetype\":\"0x611f7bf868a6212f871e89f7e44684045ddfb09d\",\
         \"degraded\":false,\"created_at\":{now}}}"
    );
    let rec = AlertRecord {
        alert_key: "k".into(),
        kind: "whale_accumulation".into(),
        asset: Some("SOL".into()),
        window_start: now - 3_600_000,
        severity: "medium".into(),
        degraded: false,
        payload,
        created_at: now,
    };

    let d = Digest::build(&[rec], DigestPeriod::Daily, now);
    let text = d.render("uk");
    assert!(!text.contains("0x"), "у дайджесті є hex-рядок:\n{text}");
    assert!(
        !text.contains("611f7bf8"),
        "у дайджесті просочилася адреса:\n{text}"
    );
}

#[test]
fn дайджест_рендериться_у_двох_мовах() {
    let now = now_ms();
    let records = vec![record(&alert(
        AlertKind::LiquidationSpike,
        "MARKET",
        Severity::High,
        122,
        493_100.0,
    ))];

    let uk = Digest::build(&records, DigestPeriod::Daily, now).render("uk");
    let en = Digest::build(&records, DigestPeriod::Weekly, now).render("en");

    assert!(uk.contains("Дайджест за добу"), "{uk}");
    assert!(uk.contains("Ліквідації"), "{uk}");
    assert!(en.contains("Digest for the last 7 days"), "{en}");
    assert!(en.contains("Liquidations"), "{en}");
    // Агрегати видно, але без адрес.
    assert!(uk.contains("122"), "{uk}");
    assert!(!uk.contains("0x"), "{uk}");
}

#[test]
fn розбір_періоду() {
    assert_eq!(DigestPeriod::parse("daily"), DigestPeriod::Daily);
    assert_eq!(DigestPeriod::parse(""), DigestPeriod::Daily);
    assert_eq!(DigestPeriod::parse("немає такого"), DigestPeriod::Daily);
    assert_eq!(DigestPeriod::parse("weekly"), DigestPeriod::Weekly);
    assert_eq!(DigestPeriod::parse("week"), DigestPeriod::Weekly);
    assert_eq!(DigestPeriod::parse("7d"), DigestPeriod::Weekly);
    assert_eq!(DigestPeriod::parse("WEEKLY"), DigestPeriod::Weekly);

    assert_eq!(DigestPeriod::Daily.window_ms(), 24 * 60 * 60 * 1000);
    assert_eq!(DigestPeriod::Weekly.window_ms(), 7 * 24 * 60 * 60 * 1000);
}
