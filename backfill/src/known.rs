//! known — реєстр того, що вже з'ясовано. Пастки, артефакти, словники.
//!
//! Кожен пункт тут коштував або квоти Etherscan, або хибного висновку в звіті. Список
//! доповнюється; видаляти з нього можна лише те, що спростовано новими даними.
//!
//! Коди патернів (для PatternDb) тримаються теж тут, щоб два біни не назвали одне й те
//! саме різними словами й не розпалися на два патерни замість одного.

// ─────────────────────────── коди патернів ───────────────────────────
pub mod pat {
    // структурна роль (rolemap)
    pub const ENTRY: &str = "ENTRY"; // вхід капіталу ззовні
    pub const MAKER: &str = "MAKER"; // торгує, робить обсяг
    pub const FUNNEL: &str = "FUNNEL"; // збирає з багатьох (див. пастку SPAM_VICTIM!)
    pub const GATE: &str = "GATE"; // шлюз на біржу
    pub const TREASURY: &str = "TREASURY"; // казна, тримає й роздає
    pub const DISTRIBUTOR: &str = "DISTRIBUTOR"; // роздавач на багато адрес
    pub const DORMANT: &str = "DORMANT"; // спить
    pub const MASK: &str = "MASK"; // дрібний транзит
    pub const TRANSIT: &str = "TRANSIT"; // прийняв і одразу віддав далі
    pub const FUNDER: &str = "FUNDER"; // заправляє інших газом/капіталом

    /// Роль з rolemap (українською) → код патерну. Один словник на всі біни, щоб
    /// «МЕЙКЕР» і «MAKER» не роз'їхались на два різні патерни.
    pub fn from_role(role: &str) -> Option<&'static str> {
        Some(match role {
            "ВОРОНКА" => FUNNEL,
            "ВХІД_CEX" => ENTRY,
            "КАЗНА" => TREASURY,
            "МАСКА" => MASK,
            "МЕЙКЕР" => MAKER,
            "РОЗДАВАЧ" => DISTRIBUTOR,
            "СПЛЯЧИЙ" => DORMANT,
            "ТРАНЗИТ" => TRANSIT,
            "ФАНДЕР" => FUNDER,
            "ХОЛДЕР" => HOLDER,
            "ШЛЮЗ_CEX" => GATE,
            _ => return None, // «НЕМАЄ ДАНИХ» — не патерн, а відсутність спостереження
        })
    }

    // поведінка
    pub const MARKET_MAKER: &str = "MARKET-MAKER"; // двосторонній потік, короткий строк
    pub const FLIPPER: &str = "FLIPPER"; // тримає дні, не місяці
    pub const HOLDER: &str = "HOLDER"; // тримає довго
    pub const ALLOCATION: &str = "ALLOCATION"; // отримав з mint/genesis
    pub const CEX_DIRECT: &str = "CEX-DIRECT"; // виводить прямо на біржу
    pub const CEX_1HOP: &str = "CEX-1HOP"; // виводить через один хоп
    pub const CONDUCTOR: &str = "CONDUCTOR"; // фінансує ту саму команду в ≥2 активах
    pub const SWARM: &str = "SWARM"; // свіжа адреса з рою одного диспенсера
    pub const BULLRUN_ACTIVE: &str = "BULLRUN-ACTIVE";

    // технічні мітки
    pub const CONTRACT: &str = "CONTRACT";
    pub const CEX_LABELED: &str = "CEX-LABELED";
    pub const PROTOCOL: &str = "PROTOCOL"; // контракт протоколу (Lido/Aave/Sky)
    pub const SPAM_VICTIM: &str = "SPAM-VICTIM"; // «воронка», яка насправді жертва роздач
    pub const ARTIFACT: &str = "ARTIFACT"; // не рахувати в підсумках
}

// ─────────────────────────── пастки ───────────────────────────

/// Адреси, які роздували підсумки, поки їх не спіймали. Виключати з часток емісії.
/// (адреса, чому)
/// Адреси, які роздували підсумки, поки їх не спіймали. Тепер у базі
/// (`wallet_tags`, тег `known:artifact_addrs`) разом із поясненням-чому.
pub fn artifact_addrs() -> Vec<(String, String)> {
    let Ok(c) = wr_store::open() else { return Vec::new() };
    let Ok(mut q) = c.prepare(
        "SELECT w.address, COALESCE(w.notes, w.label, '') FROM wallet_tags g
         JOIN wallets w ON w.chain=g.chain AND w.address=g.address
         WHERE g.tag='known:artifact_addrs'",
    ) else {
        return Vec::new();
    };
    q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|it| it.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
}

/// Токени масових спам-роздач. Їх присутність НЕ означає, що адреса щось збирає.
/// Саме вони зробили 421 фальшиву «воронку».
pub const SPAM_TOKENS: &[&str] = &[
    "unilife", "gemini3", "epepe", "elon air", "chinahorse", "ha138com", "metaai",
    "flashloan", "chatgpt", "xena", "aidoge", "0claim", "visit", "reward", ".com", ".net",
    "airdrop", "claim",
];

/// Токени-обгортки протоколів: їх «холдер» майже завжди сам контракт протоколу,
/// а не учасник групи. Спіймано на sUSDS 23% і wstETH 8.33% «емісії».
pub const PROTOCOL_TOKENS: &[&str] = &[
    "susds", "wsteth", "steth", "aeth", "aethweth", "aethusdt", "aethusdc", "aethwbtc",
    "cdai", "cusdc", "ceth", "ausdc", "ausdt", "adai", "yv", "crv", "cvxcrv",
];

/// Слова-мітки бірж і депозитів.
pub const CEX_WORDS: &[&str] = &[
    "binance", "coinbase", "okx", "kraken", "bybit", "kucoin", "bitfinex", "huobi", "mexc",
    "crypto.com", "bithumb", "gemini", "bitstamp", "gate.io", "upbit", "bitget", "htx",
    "deposit", "otc", "exchange",
];

pub fn is_cex_label(label: &str) -> bool {
    let l = label.to_lowercase();
    CEX_WORDS.iter().any(|w| l.contains(w))
}

pub fn is_spam_token(sym: &str) -> bool {
    let s = sym.to_lowercase();
    SPAM_TOKENS.iter().any(|w| s.contains(w))
}

pub fn is_protocol_token(sym: &str) -> bool {
    let s = sym.to_lowercase();
    PROTOCOL_TOKENS.iter().any(|w| s == *w || s.starts_with(w))
}

/// Повертає причину, чому адресу визнано артефактом. Тип змінився з
/// `&'static str` на `String`: пояснення тепер приходить із бази, а не з
/// компільованої таблиці.
pub fn is_artifact(addr: &str) -> Option<String> {
    let a = addr.to_lowercase();
    artifact_addrs().into_iter().find(|(x, _)| *x == a).map(|(_, why)| why)
}

// ─────────────────────────── правила збору ───────────────────────────

/// Etherscan віддає щонайбільше стільки записів на запит — незалежно від `offset`.
/// Якщо повернулось рівно стільки, дані ОБРІЗАНО: треба ділити діапазон блоків.
pub const PAGE_MAX: usize = 1000;

/// Ліміт відповіді NOTOK приходить як HTTP 200. Тіло: {"status":"0","message":"NOTOK"}.
/// Без цієї перевірки відмова зараховується як «порожньо» і втрачається тихо.
pub fn is_rate_limited(body: &str) -> bool {
    body.contains("\"status\":\"0\"")
        && (body.contains("Max rate limit") || body.contains("NOTOK") || body.contains("rate limit"))
}

/// Відповідь, яку МОЖНА вважати чесним «порожньо».
pub fn is_empty_ok(body: &str) -> bool {
    body.contains("No transactions found") || body.contains("No records found")
}

/// Ретрай теж списується з квоти (100k/добу на ключ, 3 запити/с). Тому пауза, а не шквал.
pub const RATE_PER_SEC: f64 = 3.0;
pub const DAILY_QUOTA: u64 = 100_000;

/// Скільки ETH узагалі існує (порядок величини, ~120 млн).
///
/// Потрібне як межа здорового глузду. P&L нової групи показав «куплено 255 078 529 ETH» —
/// удвічі більше за всю емісію. Причина не в помилці арифметики, а в природі мережі:
/// капітал ходить по колу (доведено взаємними парами з симетрією 67–91%), і кожен оберт
/// зараховується як нова купівля, щойно контрагент опиняється поза списком членів.
/// Тому будь-який ОБІГ, що перевищує емісію, не можна тлумачити як обсяг торгівлі.
pub const ETH_SUPPLY: f64 = 120_000_000.0;

/// Чи правдоподібний сукупний обсяг. Повертає причину, якщо ні.
pub fn sanity_volume(amount: f64, supply: f64, what: &str) -> Option<String> {
    if supply > 0.0 && amount > supply {
        return Some(format!(
            "{what}: обсяг {amount:.0} перевищує емісію {supply:.0} у {:.1}× — це циркуляція, а не купівлі",
            amount / supply
        ));
    }
    None
}

/// ETH-значення беруться ЛИШЕ з рядків k=ext|int. Токенні рядки теж мають поле `v`,
/// і ділення їх на 1e18 колись дало «19.8 трлн ETH».
pub fn is_eth_row(kind: &str) -> bool {
    kind == "ext" || kind == "int"
}
