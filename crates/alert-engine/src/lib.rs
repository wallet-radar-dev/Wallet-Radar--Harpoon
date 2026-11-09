//! Двигун алертів: сирі події + зріз агрегатів → готовий алерт.
//!
//! Два важливі рішення:
//!
//! 1. **Алерт будується з вікна, а не з однієї події.** Подія «надійшов актив X
//!    на 12 400 USD» — це шум. Алерт «3 учасники накопичили X на 42 800 USD за
//!    15 хв» — це рішення. Віконний підхід робить систему стійкою до пропусків:
//!    втрачена подія зменшує доказову базу, але не створює хибного алерту.
//!
//! 2. **Ключ ідемпотентності — це доказ, а не подія.** `AlertKey` = хеш від
//!    (вид, актив, початок вікна, хеш набору доказів). Той самий доказ у тому
//!    самому вікні → той самий ключ → повтор відкидається на рівні сховища, ще
//!    до того, як хтось щось отримав.
//!
//! Структури алерту **не мають поля для адреси** — див. [`Metrics`]. Тому
//! рендер не може випадково показати гаманця: йому просто нема звідки.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use wr_client::{CrossAssetRow, Phase, RawEvent, Snapshot};

/// Вид алерту.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlertKind {
    WhaleAccumulation,
    WhaleDistribution,
    PumpPhase,
    LiquidationSpike,
    CapitalGroupChange,
    CrossAssetAnomaly,
}

impl AlertKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertKind::WhaleAccumulation => "whale_accumulation",
            AlertKind::WhaleDistribution => "whale_distribution",
            AlertKind::PumpPhase => "pump_phase",
            AlertKind::LiquidationSpike => "liquidation_spike",
            AlertKind::CapitalGroupChange => "capital_group_change",
            AlertKind::CrossAssetAnomaly => "cross_asset_anomaly",
        }
    }

    pub fn topic(&self) -> &'static str {
        match self {
            AlertKind::CapitalGroupChange => "group",
            AlertKind::CrossAssetAnomaly => "cross_asset",
            AlertKind::LiquidationSpike | AlertKind::PumpPhase => "liquidation",
            _ => "whale",
        }
    }
}

/// Напрям руху.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Accumulation,
    Distribution,
}

/// Важливість.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Medium,
    High,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Medium => "medium",
            Severity::High => "high",
        }
    }

    pub fn parse(s: &str) -> Severity {
        match s.trim().to_lowercase().as_str() {
            "info" => Severity::Info,
            "high" => Severity::High,
            _ => Severity::Medium,
        }
    }
}

/// Часове вікно агрегації.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Window {
    W15m,
    W1h,
    W24h,
}

impl Window {
    pub fn as_str(&self) -> &'static str {
        match self {
            Window::W15m => "15m",
            Window::W1h => "1h",
            Window::W24h => "24h",
        }
    }

    pub fn parse(s: &str) -> Option<Window> {
        match s.trim().to_lowercase().as_str() {
            "15m" => Some(Window::W15m),
            "1h" => Some(Window::W1h),
            "24h" => Some(Window::W24h),
            _ => None,
        }
    }

    pub fn ms(&self) -> i64 {
        match self {
            Window::W15m => 15 * 60 * 1000,
            Window::W1h => 60 * 60 * 1000,
            Window::W24h => 24 * 60 * 60 * 1000,
        }
    }
}

/// Актив для показу.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetLabel {
    pub symbol: String,
    pub name: String,
    pub chain: String,
}

impl AssetLabel {
    pub fn unknown(symbol: impl Into<String>) -> AssetLabel {
        let symbol = symbol.into();
        AssetLabel {
            name: symbol.clone(),
            symbol,
            chain: String::new(),
        }
    }
}

/// Агрегати алерту.
///
/// **Поля для адреси тут немає й не буде.** Усе, що ми показуємо про гаманців —
/// агрегати: скільки учасників, яка сума за вікно, яка частка емісії групи.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub participants: u32,
    pub total_value_usd: f64,
    pub group_share_pct: Option<f64>,
    pub concentration_top10_pct: Option<f64>,
    pub group_size: Option<u32>,
    pub window: Window,
}

/// Ключ ідемпотентності алерту.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AlertKey(String);

impl AlertKey {
    /// Вивести ключ із доказу.
    pub fn derive(kind: AlertKind, asset: &str, window_start: i64, evidence: &[&str]) -> AlertKey {
        let mut h = Sha256::new();
        h.update(kind.as_str().as_bytes());
        h.update(b"|");
        h.update(asset.as_bytes());
        h.update(b"|");
        h.update(window_start.to_le_bytes());
        h.update(b"|");
        // Доказ сортуємо: порядок не має значення, набір — має.
        let mut sorted: Vec<&str> = evidence.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        for e in sorted {
            h.update(e.as_bytes());
            h.update(b",");
        }
        AlertKey(hex::encode(h.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Готовий алерт.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub key: AlertKey,
    pub kind: AlertKind,
    pub severity: Severity,
    pub asset: AssetLabel,
    /// Супутні активи того самого сигналу — щоб не надсилати по алерту на
    /// кожен актив (було понад 20 повідомлень замість одного).
    pub related: Vec<AssetLabel>,
    pub direction: Option<Direction>,
    pub metrics: Metrics,
    pub archetype: Option<String>,
    /// Джерело було неповним (дірка в стрічці або збій) — показуємо це користувачеві.
    pub degraded: bool,
    pub created_at: i64,
}

impl Alert {
    /// Текст алерту. Мова: `uk` або `en`.
    ///
    /// Єдиний спосіб отримати текст алерту — значить, і єдине місце, де він може
    /// з'явитися. Фільтр на виході — друга лінія оборони.
    pub fn render(&self, lang: &str) -> String {
        let en = lang.starts_with("en");
        let mut out = String::new();

        let (icon, title) = match self.kind {
            AlertKind::WhaleAccumulation => (
                "🐋",
                if en {
                    "Accumulation"
                } else {
                    "Накопичення"
                },
            ),
            AlertKind::WhaleDistribution => (
                "🐋",
                if en {
                    "Distribution"
                } else {
                    "Розподіл"
                },
            ),
            AlertKind::PumpPhase => (
                "🚀",
                if en {
                    "Pump phase"
                } else {
                    "Памп-фаза"
                },
            ),
            AlertKind::LiquidationSpike => (
                "💥",
                if en {
                    "Liquidations"
                } else {
                    "Ліквідації"
                },
            ),
            AlertKind::CapitalGroupChange => (
                "🧬",
                if en {
                    "Capital group"
                } else {
                    "Капітал-група"
                },
            ),
            AlertKind::CrossAssetAnomaly => ("🔀", "Cross-asset"),
        };
        // Без parse_mode Telegram не розбере «*жирний*», тож форматуємо текстом.
        out.push_str(&format!("{icon} {title}"));

        if !self.asset.symbol.is_empty() {
            out.push_str(&format!(" · {}", self.asset.symbol));
        }
        out.push('\n');

        if !self.related.is_empty() {
            let names: Vec<String> = self.related.iter().map(|a| a.symbol.clone()).collect();
            let label = if en { "also" } else { "також" };
            out.push_str(&format!("• {label}: {}\n", names.join(", ")));
        }

        let m = &self.metrics;
        let window_label = m.window.as_str();
        let participants = if en {
            format!("{} participants", m.participants)
        } else {
            format!("{} учасників", m.participants)
        };
        let sum_label = if en {
            format!("${} over {}", fmt_usd(m.total_value_usd), window_label)
        } else {
            format!("${} за {}", fmt_usd(m.total_value_usd), window_label)
        };
        // Сума потрібна лише там, де вона є: для змін складу групи її немає,
        // і «$0» виглядало б як помилка, а не як агрегат.
        if m.total_value_usd > 0.0 {
            out.push_str(&format!("• {participants} · {sum_label}\n"));
        } else {
            out.push_str(&format!("• {participants}\n"));
        }

        if let Some(share) = m.group_share_pct {
            let label = if en {
                "group share of emission"
            } else {
                "частка емісії групи"
            };
            out.push_str(&format!("• {label}: {}%\n", fmt_num(share)));
        }
        if let Some(top) = m.concentration_top10_pct {
            let label = if en {
                "top-10 concentration"
            } else {
                "концентрація top-10"
            };
            out.push_str(&format!("• {label}: {}%\n", fmt_num(top)));
        }
        if let Some(size) = m.group_size {
            let label = if en {
                "group members"
            } else {
                "учасників у групі"
            };
            out.push_str(&format!("• {label}: {size}\n"));
        }
        if let Some(a) = &self.archetype {
            let label = if en { "archetype" } else { "архетип" };
            let value = match a.as_str() {
                "new" => {
                    if en {
                        "new group"
                    } else {
                        "нова група"
                    }
                }
                "changed" => {
                    if en {
                        "composition changed"
                    } else {
                        "зміна складу"
                    }
                }
                other => other,
            };
            out.push_str(&format!("• {label}: {value}\n"));
        }
        if let Some(d) = self.direction {
            let label = if en {
                match d {
                    Direction::Accumulation => "direction: accumulation",
                    Direction::Distribution => "direction: distribution",
                }
            } else {
                match d {
                    Direction::Accumulation => "напрям: накопичення",
                    Direction::Distribution => "напрям: розподіл",
                }
            };
            out.push_str(&format!("• {label}\n"));
        }
        if self.degraded {
            let label = if en {
                "source was incomplete — data may be missing"
            } else {
                "джерело було неповним — дані можуть бути не всі"
            };
            out.push_str(&format!("⚠️ {label}\n"));
        }
        out
    }
}

/// Правило алерту (з конфіга).
#[derive(Clone, Debug)]
pub struct Rule {
    pub kind: AlertKind,
    pub min_participants: u32,
    pub min_total_value_usd: f64,
    pub windows: Vec<Window>,
    /// Порожній список — усі активи.
    pub assets: Vec<String>,
    pub severity: Severity,
}

impl Rule {
    pub fn covers_asset(&self, symbol: &str) -> bool {
        self.assets.is_empty() || self.assets.iter().any(|a| a.eq_ignore_ascii_case(symbol))
    }
}

/// Подія у вікні (зберігається тільки те, з чого рахується агрегат).
#[derive(Clone, Debug)]
struct BucketEvent {
    ts: i64,
    inflow: bool,
    value_usd: f64,
    wallet_id: String,
    id: String,
}

#[derive(Default)]
struct Bucket {
    events: Vec<BucketEvent>,
}

/// Двигун.
pub struct Engine {
    rules: Vec<Rule>,
    buckets: HashMap<String, Bucket>,
    snapshot: Option<Snapshot>,
    /// Склад груп, який ми бачили востаннє — щоб ловити зміни.
    known_groups: HashMap<String, u32>,
    /// Ключі, які вже видано в цьому процесі (не дублюємо між оцінками).
    emitted: HashSet<AlertKey>,
    degraded: bool,
}

impl Engine {
    pub fn new(rules: Vec<Rule>) -> Engine {
        Engine {
            rules,
            buckets: HashMap::new(),
            snapshot: None,
            known_groups: HashMap::new(),
            emitted: HashSet::new(),
            degraded: false,
        }
    }

    pub fn set_degraded(&mut self, degraded: bool) {
        self.degraded = degraded;
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    /// Додати подію у вікна.
    pub fn ingest_event(&mut self, ev: &RawEvent) {
        if ev.asset_symbol.is_empty() {
            return;
        }
        let bucket = self.buckets.entry(ev.asset_symbol.clone()).or_default();
        bucket.events.push(BucketEvent {
            ts: ev.ts,
            inflow: matches!(ev.flow, wr_client::FlowKind::In),
            value_usd: ev.value_usd,
            wallet_id: ev.wallet_id.clone(),
            id: ev.id.clone(),
        });
    }

    /// Відкинути події, що вийшли за 24 години.
    fn prune(&mut self, now: i64) {
        let cutoff = now - Window::W24h.ms();
        self.buckets.retain(|_, b| {
            b.events.retain(|e| e.ts >= cutoff);
            !b.events.is_empty()
        });
    }

    /// Оновити зріз джерела й вирвати з нього алерти, які не require стрічки.
    pub fn apply_snapshot(&mut self, snap: &Snapshot) -> Vec<Alert> {
        let mut out = Vec::new();

        // 1. Зміни складу капітал-груп: нова група або зміна числа учасників.
        let mut current: HashMap<String, u32> = HashMap::new();
        for g in &snap.groups {
            current.insert(g.group_id.clone(), g.member_count);
            match self.known_groups.get(&g.group_id) {
                None => {
                    out.push(self.group_alert(
                        g.group_id.clone(),
                        g.member_count,
                        g.common_assets.clone(),
                        "new",
                        snap.as_of,
                    ));
                }
                Some(prev) if *prev != g.member_count => {
                    out.push(self.group_alert(
                        g.group_id.clone(),
                        g.member_count,
                        g.common_assets.clone(),
                        "changed",
                        snap.as_of,
                    ));
                }
                _ => {}
            }
        }
        self.known_groups = current;

        // 2. Ліквідації: сплеск шортів за годину.
        if let Some(liq) = snap.liquidations {
            let threshold = self
                .rules
                .iter()
                .find(|r| r.kind == AlertKind::LiquidationSpike)
                .map(|r| r.min_total_value_usd)
                .unwrap_or(250_000.0);
            if liq.w1h.total_usd >= threshold {
                let key = AlertKey::derive(
                    AlertKind::LiquidationSpike,
                    "market",
                    liq.w1h.total_usd as i64 / 3_600_000,
                    &["liquidations"],
                );
                if self.emitted.insert(key.clone()) {
                    out.push(Alert {
                        key,
                        kind: AlertKind::LiquidationSpike,
                        related: Vec::new(),
                        severity: Severity::High,
                        asset: AssetLabel::unknown("MARKET"),
                        direction: None,
                        metrics: Metrics {
                            participants: liq.w1h.count,
                            total_value_usd: liq.w1h.total_usd,
                            group_share_pct: None,
                            concentration_top10_pct: None,
                            group_size: None,
                            window: Window::W1h,
                        },
                        archetype: None,
                        degraded: self.degraded,
                        created_at: snap.as_of,
                    });
                }
            }
        }

        // 3. Cross-asset: актив, який тримає багато гаманців-«мультиактивів».
        //
        //    Один алерт на ВІКНО, а не на кожен актив: на живих даних таких
        //    активів два десятки, і алерт на кожен — це два десятки
        //    повідомлень кожному підписнику за один зріз. Топ-3 за часткою —
        //    це сигнал, а не шум.
        if let Some(rule) = self
            .rules
            .iter()
            .find(|r| r.kind == AlertKind::CrossAssetAnomaly)
        {
            let mut rows: Vec<&CrossAssetRow> = snap
                .cross_asset
                .iter()
                .filter(|row| {
                    row.holders >= rule.min_participants && rule.covers_asset(&row.symbol)
                })
                .collect();
            if !rows.is_empty() {
                rows.sort_by(|a, b| {
                    b.share_pct
                        .partial_cmp(&a.share_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let top: Vec<&CrossAssetRow> = rows.into_iter().take(3).collect();
                let symbols: Vec<String> = top.iter().map(|r| r.symbol.clone()).collect();
                let evidence: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
                let key = AlertKey::derive(
                    AlertKind::CrossAssetAnomaly,
                    &symbols.join(","),
                    snap.as_of / 3_600_000,
                    &evidence,
                );
                if self.emitted.insert(key.clone()) {
                    let participants = top.iter().map(|r| r.holders).max().unwrap_or(0);
                    out.push(Alert {
                        key,
                        kind: AlertKind::CrossAssetAnomaly,
                        severity: rule.severity,
                        asset: AssetLabel::unknown(symbols[0].clone()),
                        related: symbols[1..]
                            .iter()
                            .map(|s| AssetLabel::unknown(s.clone()))
                            .collect(),
                        direction: None,
                        metrics: Metrics {
                            participants,
                            total_value_usd: 0.0,
                            group_share_pct: top.first().map(|r| r.share_pct),
                            concentration_top10_pct: None,
                            group_size: None,
                            window: Window::W1h,
                        },
                        archetype: None,
                        degraded: self.degraded,
                        created_at: snap.as_of,
                    });
                }
            }
        }

        self.snapshot = Some(snap.clone());
        out
    }

    fn group_alert(
        &self,
        group_id: String,
        member_count: u32,
        common_assets: Vec<String>,
        what: &str,
        now: i64,
    ) -> Alert {
        let key = AlertKey::derive(
            AlertKind::CapitalGroupChange,
            &group_id,
            now / 600_000,
            &[&group_id],
        );
        Alert {
            key,
            kind: AlertKind::CapitalGroupChange,
            severity: Severity::Medium,
            asset: AssetLabel::unknown(
                common_assets
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "GROUP".into()),
            ),
            related: Vec::new(),
            direction: None,
            metrics: Metrics {
                participants: member_count,
                total_value_usd: 0.0,
                group_share_pct: None,
                concentration_top10_pct: None,
                group_size: Some(member_count),
                window: Window::W1h,
            },
            archetype: Some(what.to_string()),
            degraded: self.degraded,
            created_at: now,
        }
    }

    /// Оцінити всі вікна й видати алерти, що дозріли.
    pub fn evaluate(&mut self, now: i64) -> Vec<Alert> {
        self.prune(now);
        let mut out = Vec::new();

        let snapshot = self.snapshot.clone();
        let assets: Vec<String> = self.buckets.keys().cloned().collect();

        for symbol in assets {
            let bucket = match self.buckets.get(&symbol) {
                Some(b) => b,
                None => continue,
            };
            for rule in self.rules.clone() {
                if !matches!(
                    rule.kind,
                    AlertKind::WhaleAccumulation | AlertKind::WhaleDistribution
                ) {
                    continue;
                }
                if !rule.covers_asset(&symbol) {
                    continue;
                }
                let inflow = matches!(rule.kind, AlertKind::WhaleAccumulation);
                for window in rule.windows.clone() {
                    let wms = window.ms();
                    let window_start = now - (now % wms);
                    let since = now - wms;

                    let mut participants: HashSet<&str> = HashSet::new();
                    let mut total = 0.0f64;
                    let mut evidence: Vec<&str> = Vec::new();
                    for e in bucket.events.iter() {
                        if e.ts < since || e.ts > now {
                            continue;
                        }
                        if e.inflow != inflow {
                            continue;
                        }
                        if e.value_usd <= 0.0 {
                            continue;
                        }
                        participants.insert(e.wallet_id.as_str());
                        total += e.value_usd;
                        evidence.push(e.id.as_str());
                    }
                    if participants.len() < rule.min_participants as usize {
                        continue;
                    }
                    if total < rule.min_total_value_usd {
                        continue;
                    }

                    let key = AlertKey::derive(rule.kind, &symbol, window_start, &evidence);
                    if !self.emitted.insert(key.clone()) {
                        continue;
                    }

                    let (group_share, concentration) = match &snapshot {
                        Some(s) => (
                            self.group_share_for(s, &symbol),
                            s.whale_assets
                                .iter()
                                .find(|a| a.symbol.eq_ignore_ascii_case(&symbol))
                                .map(|a| a.top10_pct),
                        ),
                        None => (None, None),
                    };
                    let group_size = snapshot.as_ref().and_then(|s| {
                        s.signals
                            .iter()
                            .find(|f| f.phase == Phase::Accumulating)
                            .map(|f| f.size)
                    });

                    out.push(Alert {
                        key,
                        kind: rule.kind,
                        severity: rule.severity,
                        asset: AssetLabel::unknown(symbol.clone()),
                        related: Vec::new(),
                        direction: Some(if inflow {
                            Direction::Accumulation
                        } else {
                            Direction::Distribution
                        }),
                        metrics: Metrics {
                            participants: participants.len() as u32,
                            total_value_usd: total,
                            group_share_pct: group_share,
                            concentration_top10_pct: concentration,
                            group_size,
                            window,
                        },
                        archetype: None,
                        degraded: self.degraded,
                        created_at: now,
                    });
                }
            }
        }
        out
    }

    fn group_share_for(&self, snap: &Snapshot, symbol: &str) -> Option<f64> {
        snap.signals
            .iter()
            .filter(|f| f.top_emission_pct.is_some())
            .max_by(|a, b| {
                a.top_emission_pct
                    .unwrap_or(0.0)
                    .partial_cmp(&b.top_emission_pct.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|f| f.top_emission_pct.unwrap_or(0.0))
            .filter(|_| !symbol.is_empty())
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

fn fmt_num(v: f64) -> String {
    if (v.fract()).abs() < 0.05 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wr_client::FlowKind;

    fn ev(id: &str, wallet: &str, symbol: &str, inflow: bool, value: f64, ts: i64) -> RawEvent {
        RawEvent {
            id: id.into(),
            wallet_id: wallet.into(),
            network: wr_client::Network::Eth,
            flow: if inflow { FlowKind::In } else { FlowKind::Out },
            asset_symbol: symbol.into(),
            chain: "eth".into(),
            amount: 1.0,
            value_usd: value,
            ts,
        }
    }

    fn rules() -> Vec<Rule> {
        vec![Rule {
            kind: AlertKind::WhaleAccumulation,
            min_participants: 3,
            min_total_value_usd: 20_000.0,
            windows: vec![Window::W15m],
            assets: vec![],
            severity: Severity::Medium,
        }]
    }

    #[test]
    fn alert_requires_enough_participants_and_sum() {
        let mut e = Engine::new(rules());
        let now = 1_000_000_000_000;
        e.ingest_event(&ev("1", "w1", "SOL", true, 10_000.0, now - 1000));
        e.ingest_event(&ev("2", "w2", "SOL", true, 10_000.0, now - 900));
        assert!(e.evaluate(now).is_empty(), "двох учасників замало");

        e.ingest_event(&ev("3", "w3", "SOL", true, 10_000.0, now - 800));
        let alerts = e.evaluate(now);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].metrics.participants, 3);
        assert_eq!(alerts[0].metrics.total_value_usd, 30_000.0);
    }

    #[test]
    fn the_same_evidence_yields_the_same_key() {
        let mut e = Engine::new(rules());
        let now = 1_000_000_000_000;
        e.ingest_event(&ev("1", "w1", "SOL", true, 10_000.0, now - 1000));
        e.ingest_event(&ev("2", "w2", "SOL", true, 10_000.0, now - 900));
        e.ingest_event(&ev("3", "w3", "SOL", true, 10_000.0, now - 800));
        let first = e.evaluate(now);
        assert_eq!(first.len(), 1);
        // Повторна оцінка тих самих даних не дає другого алерту.
        let second = e.evaluate(now);
        assert!(second.is_empty());
        assert_eq!(first[0].key.as_str().len(), 64);
    }

    #[test]
    fn outflow_does_not_trigger_an_accumulation_alert() {
        let mut e = Engine::new(rules());
        let now = 1_000_000_000_000;
        for i in 0..4 {
            e.ingest_event(&ev(
                &i.to_string(),
                &format!("w{i}"),
                "SOL",
                false,
                10_000.0,
                now - 1000,
            ));
        }
        assert!(e.evaluate(now).is_empty());
    }

    #[test]
    fn old_events_are_pruned() {
        let mut e = Engine::new(rules());
        let now = 1_000_000_000_000;
        for i in 0..4 {
            e.ingest_event(&ev(
                &i.to_string(),
                &format!("w{i}"),
                "SOL",
                true,
                10_000.0,
                now - Window::W24h.ms() - 1000,
            ));
        }
        assert!(
            e.evaluate(now).is_empty(),
            "події поза вікном не мають давати алерт"
        );
    }

    #[test]
    fn rendered_text_has_no_wallet_addresses() {
        let mut e = Engine::new(rules());
        let now = 1_000_000_000_000;
        for i in 0..3 {
            e.ingest_event(&ev(
                &format!("0x{:064x}", i),
                &format!("wmrg3e7q{}", i),
                "SOL",
                true,
                10_000.0,
                now - 1000,
            ));
        }
        let a = e.evaluate(now).remove(0);
        let text = a.render("uk");
        assert!(text.contains("SOL"));
        assert!(!text.contains("0x"));
        assert!(!text.contains("wmrg3e7qb"));
    }

    #[test]
    fn window_start_is_aligned_so_key_is_stable_inside_the_window() {
        let k1 = AlertKey::derive(AlertKind::WhaleAccumulation, "SOL", 1_000_000, &["a"]);
        let k2 = AlertKey::derive(AlertKind::WhaleAccumulation, "SOL", 1_000_000, &["a"]);
        let k3 = AlertKey::derive(AlertKind::WhaleAccumulation, "SOL", 1_000_001, &["a"]);
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn cross_asset_gives_one_alert_per_window_not_one_per_asset() {
        let mut e = Engine::new(vec![Rule {
            kind: AlertKind::CrossAssetAnomaly,
            min_participants: 10,
            min_total_value_usd: 0.0,
            windows: vec![Window::W1h],
            assets: vec![],
            severity: Severity::Info,
        }]);
        let mut snap = Snapshot::default();
        for (i, sym) in ["OKB", "ARB", "BICO", "RLUSD", "LIT"].iter().enumerate() {
            snap.cross_asset.push(wr_client::CrossAssetRow {
                symbol: (*sym).to_string(),
                holders: 20,
                share_pct: 10.0 - i as f64,
            });
        }
        let alerts = e.apply_snapshot(&snap);
        assert_eq!(
            alerts.len(),
            1,
            "повинно бути один алерт на вікно, а не по одному на актив"
        );
        assert_eq!(alerts[0].asset.symbol, "OKB", "перший — найбільша частка");
        assert_eq!(alerts[0].related.len(), 2, "решта топ-3 — у супутніх");
        let text = alerts[0].render("uk");
        assert!(text.contains("також: ARB, BICO"), "текст: {text}");
    }

    #[test]
    fn evidence_order_does_not_change_the_key() {
        let k1 = AlertKey::derive(AlertKind::WhaleAccumulation, "SOL", 1, &["b", "a"]);
        let k2 = AlertKey::derive(AlertKind::WhaleAccumulation, "SOL", 1, &["a", "b"]);
        assert_eq!(k1, k2);
    }
}
