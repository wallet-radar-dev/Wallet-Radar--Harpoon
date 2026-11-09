//! Дайджест за період — добірка алертів, а не зріз стану.
//!
//! Раніше `/digest` показував поточний зріз джерела: «активів N, груп M».
//! Тепер це **добірка алертів за вікно часу** (добу або тиждень) —
//! скільки сигналів було, за які теми, які активи та які найважливіші.
//!
//! Джерело — журнал `alert_log`, де лежать **агрегати** (вид алерту, символ
//! актива, важливість, вікно, метрики). Тому добірка принципово не може
//! містити адрес гаманців, tx-hash чи суми окремих транзакцій: у журналі
//! такого просто немає.
//!
//! Ручні розсилки адміністратора (`kind = "manual"`) до добірки **не**
//! потрапляють — це службове повідомлення, а не ринковий сигнал.

use alert_engine::{Alert, AlertKind, Severity};
use storage::AlertRecord;

/// Період добірки.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DigestPeriod {
    /// Останні 24 години.
    #[default]
    Daily,
    /// Останні 7 днів.
    Weekly,
}

impl DigestPeriod {
    /// Розбір аргумента команди. Невідоме значення — добовий дайджест.
    pub fn parse(s: &str) -> DigestPeriod {
        match s.trim().to_lowercase().as_str() {
            "week" | "weekly" | "7d" | "7" => DigestPeriod::Weekly,
            _ => DigestPeriod::Daily,
        }
    }

    /// Довжина вікна в мілісекундах.
    pub fn window_ms(&self) -> i64 {
        match self {
            DigestPeriod::Daily => 24 * 60 * 60 * 1000,
            DigestPeriod::Weekly => 7 * 24 * 60 * 60 * 1000,
        }
    }

    /// Назва періоду для показу.
    pub fn label(&self, lang: &str) -> &'static str {
        let en = lang.starts_with("en");
        match (self, en) {
            (DigestPeriod::Daily, false) => "добу",
            (DigestPeriod::Daily, true) => "24 hours",
            (DigestPeriod::Weekly, false) => "тиждень",
            (DigestPeriod::Weekly, true) => "7 days",
        }
    }
}

/// Підсумок за період.
#[derive(Clone, Debug, Default)]
pub struct Digest {
    /// Період.
    pub period: DigestPeriod,
    /// Початок вікна (epoch-мс).
    pub since_ms: i64,
    /// Кінець вікна (epoch-мс).
    pub until_ms: i64,
    /// Скільки всього алертів у вікні.
    pub total: usize,
    /// Скільки з них високої важливості.
    pub high: usize,
    /// Скільки прийшло з неповного джерела.
    pub degraded: usize,
    /// Кількість за видами алертів (відсортовано за спаданням).
    pub by_kind: Vec<(AlertKind, usize)>,
    /// Найчастіші активи: (символ, кількість).
    pub top_assets: Vec<(String, usize)>,
    /// Найважливіші алерти періоду, згруповані за видом і активом.
    ///
    /// Групування потрібне, щоб у списку не було трьох однакових рядків
    /// «Розподіл · USDC» — замість цього один рядок із кількістю сигналів
    /// і найбільшим з них.
    pub notable: Vec<NotableGroup>,
}

/// Група алертів одного виду на одному активі.
#[derive(Clone, Debug)]
pub struct NotableGroup {
    /// Сильніший алерт групи.
    pub alert: Alert,
    /// Скільки алертів у групі.
    pub count: usize,
}

/// Скільки активів показувати в підсумку.
const TOP_ASSETS_LIMIT: usize = 5;
/// Скільки алертів показувати списком.
const NOTABLE_LIMIT: usize = 5;

impl Digest {
    /// Зібрати підсумок із журналу алертів.
    ///
    /// `records` уже відфільтровано під користувача (теми, важливість,
    /// відстежувані активи) — тут лише агрегація.
    pub fn build(records: &[AlertRecord], period: DigestPeriod, until_ms: i64) -> Digest {
        let since_ms = until_ms - period.window_ms();
        let mut digest = Digest {
            period,
            since_ms,
            until_ms,
            ..Digest::default()
        };

        let mut kinds: Vec<(AlertKind, usize)> = Vec::new();
        let mut assets: Vec<(String, usize)> = Vec::new();
        let mut notable: Vec<Alert> = Vec::new();

        for r in records {
            // За межами вікна — не в добірку. Фільтр дублює запит до БД,
            // але робить підсумок коректним навіть якщо його зібрали з
            // «ширшого» набору записів.
            if r.created_at < since_ms {
                continue;
            }
            // Вид алерту: невідомий (наприклад, ручна розсилка) — не ринок.
            let kind = match AlertKind::parse(&r.kind) {
                Some(k) => k,
                None => continue,
            };
            digest.total += 1;
            if Severity::parse(&r.severity) >= Severity::High {
                digest.high += 1;
            }
            if r.degraded {
                digest.degraded += 1;
            }

            match kinds.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, c)) => *c += 1,
                None => kinds.push((kind, 1)),
            }

            let symbol = r.asset.clone().unwrap_or_default();
            if !symbol.is_empty() {
                match assets.iter_mut().find(|(s, _)| *s == symbol) {
                    Some((_, c)) => *c += 1,
                    None => assets.push((symbol, 1)),
                }
            }

            // Повний алерт — для рядка «найважливіше». Якщо payload
            // не розбирається — лишаємо запис без метрик, а не падаємо.
            if let Ok(alert) = serde_json::from_str::<Alert>(&r.payload) {
                notable.push(alert);
            }
        }

        // Найважливіші — спершу за важливістю, потім за сумою.
        notable.sort_by(|a, b| {
            b.severity.cmp(&a.severity).then(
                b.metrics
                    .total_value_usd
                    .partial_cmp(&a.metrics.total_value_usd)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });

        // Групуємо за видом і активом: один рядок замість десятка однакових.
        let mut groups: Vec<NotableGroup> = Vec::new();
        for a in notable {
            let symbol = a.asset.symbol.clone();
            match groups
                .iter_mut()
                .find(|g| g.alert.kind == a.kind && g.alert.asset.symbol == symbol)
            {
                Some(g) => {
                    g.count += 1;
                    // Сильніший алерт групи: за важливістю, потім за сумою.
                    if a.severity > g.alert.severity
                        || (a.severity == g.alert.severity
                            && a.metrics.total_value_usd > g.alert.metrics.total_value_usd)
                    {
                        g.alert = a;
                    }
                }
                None => groups.push(NotableGroup { alert: a, count: 1 }),
            }
        }
        groups.truncate(NOTABLE_LIMIT);
        digest.notable = groups;

        // Види — за спаданням кількості, потім за назвою (стабільно).
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.as_str().cmp(b.0.as_str())));
        digest.by_kind = kinds;

        // Активи — за спаданням кількості, потім за назвою.
        assets.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        assets.truncate(TOP_ASSETS_LIMIT);
        digest.top_assets = assets;

        digest
    }

    /// Текст дайджесту для користувача.
    pub fn render(&self, lang: &str) -> String {
        let en = lang.starts_with("en");
        if self.total == 0 {
            return if en {
                format!(
                    "📬 Digest for the last {}\n• no alerts in this period",
                    self.period.label(lang)
                )
            } else {
                format!(
                    "📬 Дайджест за {} — сигналів не було",
                    self.period.label(lang)
                )
            };
        }

        let mut out = if en {
            format!("📬 Digest for the last {}\n", self.period.label(lang))
        } else {
            format!("📬 Дайджест за {}\n", self.period.label(lang))
        };
        out.push_str(&format!(
            "• {}: {}{}\n",
            if en { "alerts" } else { "алертів" },
            self.total,
            if self.high > 0 {
                if en {
                    format!(" (high severity: {})", self.high)
                } else {
                    format!(" (високої важливості: {})", self.high)
                }
            } else {
                String::new()
            }
        ));
        if self.degraded > 0 {
            out.push_str(&format!(
                "• {}: {}\n",
                if en {
                    "from an incomplete source"
                } else {
                    "з неповного джерела"
                },
                self.degraded
            ));
        }

        out.push_str(if en {
            "\nBy topic:\n"
        } else {
            "\nЗа темами:\n"
        });
        for (kind, count) in &self.by_kind {
            out.push_str(&format!(
                "• {} {} — {}\n",
                kind_icon(*kind),
                kind_label(*kind, en),
                count
            ));
        }

        if !self.top_assets.is_empty() {
            let list: Vec<String> = self
                .top_assets
                .iter()
                .map(|(s, c)| format!("{s} ({c})"))
                .collect();
            out.push_str(&format!(
                "\n{}: {}\n",
                if en {
                    "Top assets"
                } else {
                    "Топ-активи"
                },
                list.join(", ")
            ));
        }

        if !self.notable.is_empty() {
            out.push_str(if en {
                "\nMost notable:\n"
            } else {
                "\nНайважливіше:\n"
            });
            for g in &self.notable {
                out.push_str(&format!("• {}\n", notable_line(g, en)));
            }
        }

        out.push_str(&format!(
            "\n{}",
            if en {
                "Aggregates only — no wallet addresses, no transactions."
            } else {
                "Тільки агрегати — без адрес гаманців і без транзакцій."
            }
        ));
        out
    }
}

/// Іконка виду алерту — та сама, що й в живому алерті.
fn kind_icon(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::WhaleAccumulation | AlertKind::WhaleDistribution => "🐋",
        AlertKind::PumpPhase => "🚀",
        AlertKind::LiquidationSpike => "💥",
        AlertKind::CapitalGroupChange => "🧬",
        AlertKind::CrossAssetAnomaly => "🔀",
    }
}

/// Назва виду алерту.
fn kind_label(kind: AlertKind, en: bool) -> &'static str {
    match (kind, en) {
        (AlertKind::WhaleAccumulation, false) => "Накопичення",
        (AlertKind::WhaleAccumulation, true) => "Accumulation",
        (AlertKind::WhaleDistribution, false) => "Розподіл",
        (AlertKind::WhaleDistribution, true) => "Distribution",
        (AlertKind::PumpPhase, false) => "Памп-фаза",
        (AlertKind::PumpPhase, true) => "Pump phase",
        (AlertKind::LiquidationSpike, false) => "Ліквідації",
        (AlertKind::LiquidationSpike, true) => "Liquidations",
        (AlertKind::CapitalGroupChange, false) => "Капітал-група",
        (AlertKind::CapitalGroupChange, true) => "Capital group",
        (AlertKind::CrossAssetAnomaly, _) => "Cross-asset",
    }
}

/// Короткий рядок про групу алертів — лише агрегати.
fn notable_line(g: &NotableGroup, en: bool) -> String {
    let a = &g.alert;
    let mut s = format!("{} {}", kind_icon(a.kind), kind_label(a.kind, en));
    if !a.asset.symbol.is_empty() {
        s.push_str(&format!(" · {}", a.asset.symbol));
    }
    if !a.related.is_empty() {
        let names: Vec<String> = a.related.iter().map(|r| r.symbol.clone()).collect();
        let label = if en { "also" } else { "також" };
        s.push_str(&format!(" ({label}: {})", names.join(", ")));
    }
    let m = &a.metrics;
    let participants = if en {
        format!("{} participants", m.participants)
    } else {
        format!("{} учасників", m.participants)
    };
    if g.count > 1 {
        // Кілька сигналів на той самий вид і актив — кажемо про це одним
        // рядком, а не дублюємо.
        s.push_str(&format!(
            " — {} {}",
            g.count,
            if en {
                "signals"
            } else {
                plural_uk(g.count, "сигнал", "сигнали", "сигналів")
            }
        ));
        s.push_str(&format!(
            ", {}",
            if en {
                format!("up to {participants}")
            } else {
                format!("до {participants}")
            }
        ));
    } else {
        s.push_str(&format!(" — {participants}"));
    }
    if m.total_value_usd > 0.0 {
        s.push_str(&format!(
            " · ${} {} {}",
            fmt_usd(m.total_value_usd),
            if en { "over" } else { "за" },
            m.window.as_str()
        ));
    }
    if a.degraded {
        s.push_str(if en {
            " (incomplete source)"
        } else {
            " (неповне джерело)"
        });
    }
    s
}

/// Українська множина: 1 сигнал · 2 сигнали · 5 сигналів.
fn plural_uk(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let rem100 = n % 100;
    let rem10 = n % 10;
    if (11..=14).contains(&rem100) {
        return many;
    }
    match rem10 {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

fn fmt_usd(v: f64) -> String {
    if v >= 1_000_000.0 {
        format!("{:.2}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{v:.0}")
    }
}
