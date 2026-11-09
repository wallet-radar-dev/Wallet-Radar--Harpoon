//! Доменні типи Wallet Radar, приведені до вигляду, придатного для алертів.
//!
//! Важлива властивість цього модуля: **тут немає поля, у якому могла б жити
//! адреса гаманця**. Адреси приходять у відповідях API (`root`, `members[].address`,
//! `counterparty`, `txHash`), але розбір їх просто не копіює — назовно йде
//! `family_id`/`group_id` (хеш), а не адреса. Тому текст алерту фізично не може
//! містити адресу: їй немає звідки взяти.
//!
//! Перший крейт, який бачить сирі дані джерела. Сирий JSON не перетинає його межу.

use serde::{Deserialize, Serialize};

/// Мережа. Відома — enum; невідома — рядок (щоб нова мережа не ламала розбір).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Network {
    Eth,
    Bsc,
    Base,
    Arbitrum,
    Polygon,
    Solana,
    Tron,
    Other(String),
}

impl Network {
    pub fn parse(s: &str) -> Network {
        match s.trim().to_lowercase().as_str() {
            "eth" | "ethereum" => Network::Eth,
            "bsc" | "bnb" => Network::Bsc,
            "base" => Network::Base,
            "arbitrum" | "arb" => Network::Arbitrum,
            "polygon" | "matic" => Network::Polygon,
            "sol" | "solana" => Network::Solana,
            "tron" | "trx" => Network::Tron,
            other => Network::Other(other.to_string()),
        }
    }
}

/// Напрям руху коштів.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowKind {
    In,
    Out,
    SelfTransfer,
}

impl FlowKind {
    pub fn parse(s: &str) -> FlowKind {
        match s.trim().to_lowercase().as_str() {
            "in" | "buy" => FlowKind::In,
            "out" | "sell" => FlowKind::Out,
            _ => FlowKind::SelfTransfer,
        }
    }
}

/// Подія гаманця — сира, але вже без нікчемного для нас.
///
/// `tx_hash` ми свідомо **не парсимо**: у ньому немає потреби для алертів, а
/// кожне розширене поле — це ще одна можливість протікнути дещо зайве.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: String,
    pub wallet_id: String,
    pub network: Network,
    pub flow: FlowKind,
    pub asset_symbol: String,
    pub chain: String,
    pub amount: f64,
    pub value_usd: f64,
    pub ts: i64,
}

/// Актив для показу: тікер, назва, мережа. Без контракту — адреса контракту
/// теж не показуємо (правило: жодних explorer-посилань із конкретною адресою).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetLabel {
    pub symbol: String,
    pub name: String,
    pub chain: String,
}

/// Фаза капітал-групи.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Accumulating,
    Distributing,
    Other,
}

impl Phase {
    pub fn parse(s: &str) -> Phase {
        match s.trim().to_lowercase().as_str() {
            "accumulating" => Phase::Accumulating,
            "distributing" => Phase::Distributing,
            _ => Phase::Other,
        }
    }
}

/// Сигнал родини гаманців. `family_id` — хеш, а не адреса «root».
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilySignal {
    pub family_id: String,
    pub size: u32,
    pub volume_usd: f64,
    pub phase: Phase,
    pub sector: Option<String>,
    pub transfers: u32,
    pub top_emission_pct: Option<f64>,
}

/// Смуга концентрації.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConcentrationBand {
    Low,
    Mid,
    High,
}

impl ConcentrationBand {
    pub fn parse(s: &str) -> ConcentrationBand {
        match s.trim() {
            ">80" => ConcentrationBand::High,
            "40-80" => ConcentrationBand::Mid,
            _ => ConcentrationBand::Low,
        }
    }
}

/// Актив із виміряною концентрацією в гаманцях.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WhaleAsset {
    pub symbol: String,
    pub name: String,
    pub top10_pct: f64,
    pub top50_pct: f64,
    pub band: ConcentrationBand,
    pub sector: Option<String>,
}

/// Капітал-група — агрегована. Адреси учасників не зберігаються узагалі;
/// `group_id` — хеш складу.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalGroup {
    pub group_id: String,
    pub size: u32,
    pub member_count: u32,
    pub common_assets: Vec<String>,
}

/// Рядок cross-asset: скільки гаманців тримають актив і яку частку.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossAssetRow {
    pub symbol: String,
    pub holders: u32,
    pub share_pct: f64,
}

/// Вікно ліквідацій.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiqWindow {
    pub long_usd: f64,
    pub short_usd: f64,
    pub total_usd: f64,
    pub count: u32,
}

/// Зріз ліквідацій.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiqSnapshot {
    pub w5m: LiqWindow,
    pub w1h: LiqWindow,
    pub w24h: LiqWindow,
}

/// Архетип спільноти.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Archetype {
    pub name: String,
    pub members: u32,
}

/// Повний зріз агрегатів джерела.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub as_of: i64,
    pub signals: Vec<FamilySignal>,
    pub whale_assets: Vec<WhaleAsset>,
    pub groups: Vec<CapitalGroup>,
    pub cross_asset: Vec<CrossAssetRow>,
    pub liquidations: Option<LiqSnapshot>,
    pub archetypes: Vec<Archetype>,
    /// Зрізаний signal — з `/api/track-record`. Там уже агрегати й caveats,
    /// але текст все одно проходить фільтр на виході.
    pub track_record: Option<serde_json::Value>,
    /// Чи джерело взагаді відповідає на зріз.
    ///
    /// Секції деградують у порожні, тож зріз майже ніколи не падає. Ця
    /// ознака — єдиний спосіб відрізнити «джерело впало» від «сервер
    /// відповів 404 на секцію, якої тут немає». Від неї залежить буфер
    /// черги розсилки (див. `docs/ARCHITECTURE.md` §8.3.1).
    pub source_reachable: bool,
}

/// Кадр з WebSocket.
#[derive(Clone, Debug)]
pub enum ServerMsg {
    /// Початковий кадр: останні події (сервер віддає 60).
    Init { events: Vec<RawEvent> },
    /// Одна нова подія.
    Event(RawEvent),
    /// Інший JSON (сервер шле ще й поведінкові сповіщення та події груп).
    /// Їхній склад не гарантований, тож ми їх не розбираємо — лише рахуємо.
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_parsing_is_case_insensitive() {
        assert_eq!(Network::parse("ETH"), Network::Eth);
        assert_eq!(Network::parse("solana"), Network::Solana);
        assert_eq!(Network::parse("zksync"), Network::Other("zksync".into()));
    }

    #[test]
    fn flow_parsing_defaults_to_self_transfer() {
        assert_eq!(FlowKind::parse("in"), FlowKind::In);
        assert_eq!(FlowKind::parse("OUT"), FlowKind::Out);
        assert_eq!(FlowKind::parse("weird"), FlowKind::SelfTransfer);
    }
}
