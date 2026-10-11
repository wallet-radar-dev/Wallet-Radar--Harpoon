//! Типи довідника.
//!
//! До цього кожен модуль тягнув із бази кортежі `(String, String)` і сам
//! вирішував, що в них. Типи прибирають цілий клас помилок: переплутати
//! місцями контракт і символ більше не можна мовчки.

use std::fmt;

/// Вид гаманця. Відповідає `CHECK` у схемі, але дає компіляторові знати перелік.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WalletKind {
    /// Біржовий гарячий гаманець.
    Cex,
    Bridge,
    Contract,
    Pool,
    Router,
    /// Звичайна адреса. САМЕ ЦЕ — спостережувані гаманці, а не інфраструктура.
    Eoa,
    Defi,
    Issuer,
    Burn,
    Unknown,
}

impl WalletKind {
    pub fn parse(s: &str) -> Self {
        match s {
            "cex" => Self::Cex,
            "bridge" => Self::Bridge,
            "contract" => Self::Contract,
            "pool" => Self::Pool,
            "router" => Self::Router,
            "eoa" => Self::Eoa,
            "defi" => Self::Defi,
            "issuer" => Self::Issuer,
            "burn" => Self::Burn,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cex => "cex",
            Self::Bridge => "bridge",
            Self::Contract => "contract",
            Self::Pool => "pool",
            Self::Router => "router",
            Self::Eoa => "eoa",
            Self::Defi => "defi",
            Self::Issuer => "issuer",
            Self::Burn => "burn",
            Self::Unknown => "unknown",
        }
    }

    /// Чи є це інфраструктурою, яку виключають із кандидатів.
    ///
    /// `Eoa` і `Unknown` сюди НЕ входять навмисно: у довіднику лежать і
    /// гаманці під наглядом, і визнати їх інфраструктурою означало б викинути
    /// з аналізу рівно ті адреси, заради яких він існує.
    pub fn is_infrastructure(self) -> bool {
        matches!(
            self,
            Self::Cex | Self::Bridge | Self::Contract | Self::Pool | Self::Router | Self::Defi
                | Self::Issuer | Self::Burn
        )
    }
}

impl fmt::Display for WalletKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Гаманець із довідника.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallet {
    pub chain: String,
    /// Завжди у нижньому регістрі — це гарантує `CHECK` у схемі.
    pub address: String,
    pub kind: WalletKind,
    /// Мітка («Binance 14», «🏦 КОНЦЕНТРАТОР»). `None` — мітки немає.
    pub label: Option<String>,
}

impl Wallet {
    /// Мітка або, за її відсутності, назва виду — щоб у звіті не було порожнечі.
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or_else(|| self.kind.as_str())
    }
}

/// Вид «справжніх грошей» для P&L.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Money {
    Usd,
    Eth,
    Matic,
}

impl Money {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "usd" => Some(Self::Usd),
            "eth" => Some(Self::Eth),
            "matic" => Some(Self::Matic),
            _ => None,
        }
    }
}

/// Токен із довідника.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub chain: String,
    /// Нижній регістр, гарантовано схемою.
    pub contract: String,
    /// `None` — символ невідомий. НЕ `"?"`: заглушка виглядає як факт.
    pub symbol: Option<String>,
    /// `None` — десяткових не знаємо, тож у число суму переводити НЕ МОЖНА.
    pub decimals: Option<u32>,
    pub sector: Option<String>,
    /// `Some` — це «справжні гроші» для P&L.
    pub money: Option<Money>,
}

impl Token {
    /// Чи придатний токен для перерахунку сирої суми в число.
    ///
    /// Без `decimals` перерахунок дав би помилку в мільйони разів, тож
    /// краще чесно відповісти «ні», ніж підставити 18 за замовчуванням.
    pub fn is_measurable(&self) -> bool {
        self.decimals.is_some()
    }

    pub fn symbol_or_contract(&self) -> &str {
        self.symbol.as_deref().unwrap_or(&self.contract)
    }
}

/// Роль гаманця у розслідуванні.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Seed,
    Concentrator,
    Distributor,
    Core,
    Counterparty,
    Paymaster,
    Treasury,
    Hub,
    Victim,
    Mask,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Concentrator => "concentrator",
            Self::Distributor => "distributor",
            Self::Core => "core",
            Self::Counterparty => "counterparty",
            Self::Paymaster => "paymaster",
            Self::Treasury => "treasury",
            Self::Hub => "hub",
            Self::Victim => "victim",
            Self::Mask => "mask",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_wallets_are_not_infrastructure() {
        // Регресія, що коштувала б тихого зникнення спостережуваних адрес
        // з усіх кандидатів: `eoa` НЕ інфраструктура.
        assert!(!WalletKind::Eoa.is_infrastructure());
        assert!(!WalletKind::Unknown.is_infrastructure());
        assert!(WalletKind::Cex.is_infrastructure());
        assert!(WalletKind::Burn.is_infrastructure());
    }

    #[test]
    fn kind_roundtrip() {
        for k in ["cex", "bridge", "contract", "pool", "router", "eoa", "defi", "issuer", "burn"] {
            assert_eq!(WalletKind::parse(k).as_str(), k, "вид {k} не пережив перетворення");
        }
        // невідоме не вигадуємо
        assert_eq!(WalletKind::parse("щось нове"), WalletKind::Unknown);
    }

    #[test]
    fn token_without_decimals_is_not_measurable() {
        let t = Token {
            chain: "eth".into(),
            contract: "0xabc".into(),
            symbol: None,
            decimals: None,
            sector: None,
            money: Some(Money::Usd),
        };
        assert!(!t.is_measurable(), "токен без decimals визнано придатним для лічби");
        // за відсутності символу показуємо контракт, а не "?"
        assert_eq!(t.symbol_or_contract(), "0xabc");
    }
}
