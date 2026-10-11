//! Імена тегів — переліками, а не рядками в місці виклику.
//!
//! Тег «голим» рядком має неприємну властивість: одрук не помилка, а тиша.
//! `snapshot.wallets_tagged("feed:know_cex")` поверне порожньо, модуль вирішить,
//! що біржових адрес немає, і працюватиме далі без жодного попередження.
//!
//! Тут кожен тег — константа, а `ALL` дає змогу перевірити на старті, що всі
//! вони справді є в базі. Одрук тоді падає один раз при завантаженні знімка,
//! а не тихо псує вердикти місяцями.
//!
//! Теги прив'язані до ФАЙЛУ походження (`feed:known_cex`, а не просто
//! `known_cex`) навмисно: однойменні переліки в різних модулях мали різний
//! вміст, і зведення їх в один тихо розширило б кожен.

/// Гаманці, спалені назавжди (нульова адреса, 0x…dead і похідні).
pub const BURN: &str = "burn";

/// Стартові точки розгортання графа, коли фронтир порожній.
pub const EXPAND_BOOTSTRAP: &str = "expand_bootstrap";

// ── біржові переліки (кожен модуль має СВІЙ, вміст різний) ──────────────────
pub const CEX_ROWS: &str = "cex:rows";
pub const FEED_KNOWN_CEX: &str = "feed:known_cex";
pub const NETFLOW_CEX_ADDRS: &str = "exchange_netflow:cex_addrs";
pub const NETFLOW_CEX_ADDRS_EXTRA: &str = "exchange_netflow:cex_addrs_extra";
pub const NETFLOW_KNOWN_CEX: &str = "exchange_netflow:known_cex";
pub const ROUTES_ASSETFLOW_KNOWN_CEX: &str = "routes:assetflow_known_cex";
pub const ROUTES_STRAT_KNOWN_CEX: &str = "routes:strat_known_cex";
pub const ROUTES_PB_CEX: &str = "routes:pb_cex";
pub const ROUTES_HUBS: &str = "routes:hubs";
pub const EXPAND_IS_CEX: &str = "expand:is_cex";
pub const FORENSICS_IS_KNOWN_CEX: &str = "forensics:is_known_cex";
pub const COORDGROUP_IS_HUB: &str = "coordgroup:is_hub";
pub const CONCENTRATION_KNOWN: &str = "concentration:known";
pub const TOKEN_GROUPS_KNOWN: &str = "token_groups:known";
pub const WALLET_GRAPH_KNOWN: &str = "wallet_graph:known";
pub const LIB_KNOWN_EXCHANGE: &str = "lib:known_exchange";
pub const PARIBU: &str = "paribu_deanon:paribu";

/// Адреси, що роздували підсумки, поки їх не спіймали (мітка = причина).
pub const KNOWN_ARTIFACT_ADDRS: &str = "known:artifact_addrs";

// ── мережа під наглядом ─────────────────────────────────────────────────────
pub const CEXSIGNAL_INFRA: &str = "cexsignal:infra";
pub const CEXSIGNAL_TREASURY: &str = "cexsignal:treasury";
pub const WALLETROLE_TREASURY: &str = "walletrole:treasury";
pub const WALLETROLE_DISPENSERS: &str = "walletrole:dispensers";
pub const WALLETROLE_COLLECTORS: &str = "walletrole:collectors";
pub const WALLETROLE_RELAYS: &str = "walletrole:relays";
pub const RELAY_MONITOR_RELAYS: &str = "relay_monitor:relays";
pub const CAMPWATCH_HOLDERS: &str = "campwatch:holders";

// ── токени ──────────────────────────────────────────────────────────────────
pub const DEXSWAP_STABLES: &str = "dexswap:stables";
pub const RGPRICE_STABLES: &str = "rgprice:stables";
pub const ROUTES_STRAT_UNIVERSE: &str = "routes:strat_universe";
pub const WORKERS_ETH_TOKENS: &str = "workers:eth_tokens";
pub const CAMPWATCH_TOKENS: &str = "campwatch:tokens";

/// Теги гаманців, які код справді вживає.
pub const WALLET_TAGS: &[&str] = &[
    BURN,
    EXPAND_BOOTSTRAP,
    CEX_ROWS,
    FEED_KNOWN_CEX,
    NETFLOW_CEX_ADDRS,
    NETFLOW_CEX_ADDRS_EXTRA,
    NETFLOW_KNOWN_CEX,
    ROUTES_ASSETFLOW_KNOWN_CEX,
    ROUTES_STRAT_KNOWN_CEX,
    ROUTES_PB_CEX,
    ROUTES_HUBS,
    EXPAND_IS_CEX,
    FORENSICS_IS_KNOWN_CEX,
    COORDGROUP_IS_HUB,
    CONCENTRATION_KNOWN,
    TOKEN_GROUPS_KNOWN,
    WALLET_GRAPH_KNOWN,
    LIB_KNOWN_EXCHANGE,
    PARIBU,
    KNOWN_ARTIFACT_ADDRS,
    CEXSIGNAL_INFRA,
    CEXSIGNAL_TREASURY,
    WALLETROLE_TREASURY,
    WALLETROLE_DISPENSERS,
    WALLETROLE_COLLECTORS,
    WALLETROLE_RELAYS,
    RELAY_MONITOR_RELAYS,
    CAMPWATCH_HOLDERS,
];

/// Теги токенів.
pub const TOKEN_TAGS: &[&str] =
    &[DEXSWAP_STABLES, RGPRICE_STABLES, ROUTES_STRAT_UNIVERSE, WORKERS_ETH_TOKENS, CAMPWATCH_TOKENS];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_duplicates_and_no_overlap() {
        // Той самий тег у двох переліках означав би, що гаманці й токени
        // діляться простором імен — і запит поверне суміш.
        let w: HashSet<_> = WALLET_TAGS.iter().collect();
        assert_eq!(w.len(), WALLET_TAGS.len(), "дубль серед тегів гаманців");
        let t: HashSet<_> = TOKEN_TAGS.iter().collect();
        assert_eq!(t.len(), TOKEN_TAGS.len(), "дубль серед тегів токенів");
        assert!(w.is_disjoint(&t), "тег вживається і для гаманців, і для токенів");
    }

    #[test]
    fn names_look_like_tags() {
        for t in WALLET_TAGS.iter().chain(TOKEN_TAGS) {
            assert!(!t.is_empty(), "порожнє ім'я тегу");
            assert_eq!(*t, &t.to_lowercase(), "тег не в нижньому регістрі: {t}");
            assert!(!t.contains(' '), "пробіл в імені тегу: {t}");
        }
    }
}
