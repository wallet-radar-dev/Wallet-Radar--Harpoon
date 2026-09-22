//! Port of server/clarity.js — likely CLARITY-Act regulatory status of an asset.
//! MAP and META are embedded verbatim from the JS (key order preserved via
//! serde_json's preserve_order) so /api/clarity-tags is byte-identical.

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

// Verbatim from clarity.js (extracted via clarityTags()). Insertion order matters.
const MAP_JSON: &str = r##"{"BTC":"commodity","ETH":"commodity","WETH":"commodity","STETH":"commodity","WBTC":"commodity","DOT":"commodity","GNO":"commodity","LTC":"commodity","BCH":"commodity","SOL":"commodity-likely","AVAX":"commodity-likely","MATIC":"commodity-likely","POL":"commodity-likely","ATOM":"commodity-likely","ADA":"commodity-likely","NEAR":"commodity-likely","ALGO":"commodity-likely","XLM":"commodity-likely","HBAR":"commodity-likely","XTZ":"commodity-likely","EGLD":"commodity-likely","LINK":"commodity-likely","UNI":"commodity-likely","COMP":"commodity-likely","CRV":"commodity-likely","CVX":"commodity-likely","AAVE":"commodity-likely","MKR":"commodity-likely","SNX":"commodity-likely","LDO":"commodity-likely","SUSHI":"commodity-likely","BAL":"commodity-likely","1INCH":"commodity-likely","SAND":"commodity-likely","MANA":"commodity-likely","AXS":"commodity-likely","GALA":"commodity-likely","APE":"commodity-likely","CHZ":"commodity-likely","ENJ":"commodity-likely","IMX":"commodity-likely","ENS":"commodity-likely","BAT":"commodity-likely","SHIB":"commodity-likely","FET":"commodity-likely","INJ":"commodity-likely","POKT":"commodity-likely","CAKE":"commodity-likely","GRT":"commodity-likely","RENDER":"commodity-likely","RNDR":"commodity-likely","VIRTUAL":"commodity-likely","PENGU":"commodity-likely","PRIME":"commodity-likely","MTA":"commodity-likely","TEL":"commodity-likely","LIT":"commodity-likely","BICO":"commodity-likely","ARB":"contested","OP":"contested","ZK":"contested","WLD":"contested","ONDO":"contested","EIGEN":"contested","ETHFI":"contested","WLFI":"contested","FF":"contested","STRK":"contested","SYRUP":"contested","H":"contested","USDT":"stablecoin","USDC":"stablecoin","DAI":"stablecoin","USDE":"stablecoin","USDS":"stablecoin","PYUSD":"stablecoin","RLUSD":"stablecoin","FDUSD":"stablecoin","TUSD":"stablecoin","GHO":"stablecoin","CRVUSD":"stablecoin","USD0":"stablecoin","USD1":"stablecoin","USDF":"stablecoin","USDG":"stablecoin","AUSD":"stablecoin","EURCV":"stablecoin","SATUSD":"stablecoin","APYUSD":"stablecoin","APXUSD":"stablecoin","REUSD":"stablecoin","USAT":"stablecoin","FRAX":"stablecoin","LUSD":"stablecoin","USDD":"stablecoin","BUIDL":"rwa","OUSG":"rwa","USYC":"rwa","USTBL":"rwa","JAAA":"rwa","BILL":"rwa","FDIT":"rwa","USDY":"rwa","XAUT":"rwa","KAU":"rwa","PAXG":"rwa","SAFO":"rwa","EURSAFO":"rwa","BDX":"rwa","BNB":"exchange","LEO":"exchange","OKB":"exchange","GT":"exchange","MX":"exchange","HTX":"exchange","CRO":"exchange","WBT":"exchange","BMX":"exchange","MNT":"exchange","NEXO":"exchange","KCS":"exchange","BGB":"exchange","SFP":"exchange"}"##;

const META_JSON: &str = r##"{"commodity":{"short":"Товар","label":"Digital commodity · CFTC","color":"#34d399","note":"Децентралізований, без емітента-контролера — ймовірно під юрисдикцією CFTC."},"commodity-likely":{"short":"Ймовірно товар","label":"Ймовірно digital commodity","color":"#2dd4bf","note":"Усталений децентралізований протокол; кваліфікується, якщо тримає тест 20% контролю."},"stablecoin":{"short":"Стейблкоїн","label":"Стейблкоїн · окремий режим","color":"#5b9dff","note":"Окремий режим CLARITY (спільно SEC+CFTC) + вимоги BSA/AML. Не «digital commodity»."},"rwa":{"short":"RWA","label":"Токенізований RWA / цінний папір","color":"#a78bfa","note":"Токенізовані трежеріс/фонди/золото — ймовірно цінний папір (SEC)."},"exchange":{"short":"Біржовий","label":"Біржовий токен · спірний","color":"#fb923c","note":"Централізований емітент-біржа — найслабший кейс «товару», радше цінний папір."},"contested":{"short":"Спірний","label":"Спірний · поки не зрілий","color":"#f87171","note":"Велика частка фонду/команди (>20%) або активний емітент — поки НЕ проходить тест зрілості."},"unknown":{"short":"Залежить","label":"Малий / новий — залежить","color":"#94a3b8","note":"Нішевий або новий актив — статус залежить від реальної децентралізації."}}"##;

struct Clarity {
    map_value: Value,
    meta_value: Value,
    lookup: HashMap<String, String>,
    re_commodity: Regex,
    re_stable: Regex,
    re_rwa: Regex,
}

fn clarity() -> &'static Clarity {
    static C: OnceLock<Clarity> = OnceLock::new();
    C.get_or_init(|| {
        let map_value: Value = serde_json::from_str(MAP_JSON).unwrap();
        let lookup = map_value
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        Clarity {
            meta_value: serde_json::from_str(META_JSON).unwrap(),
            map_value,
            lookup,
            // Regexes verbatim from clarity.js:60-62.
            re_commodity: Regex::new(r"^W?(BTC|ETH)$|^STETH$|^WSTETH$|^WBETH$|^CBETH$|^RETH$").unwrap(),
            re_stable: Regex::new(r"USD|EUR|GBP|^DAI$|^GHO$|^FRAX$|^LUSD$|^MIM$|^USTC$|^PYUSD$").unwrap(),
            re_rwa: Regex::new(r"XAU|GOLD|PAXG|^KAU$|^KAG$|TBILL|BILL|TREASUR|^OUSG$|^USYC$|^BUIDL$|^USTBL$|^JAAA$").unwrap(),
        }
    })
}

/// clarity.js:59-64
fn auto_classify(s: &str) -> &'static str {
    let c = clarity();
    if c.re_commodity.is_match(s) {
        "commodity"
    } else if c.re_stable.is_match(s) {
        "stablecoin"
    } else if c.re_rwa.is_match(s) {
        "rwa"
    } else {
        "commodity-likely"
    }
}

/// clarity.js:65-69 — `{ symbol, status, ...CLARITY_META[status] }`.
#[allow(dead_code)]
pub fn clarity_status(symbol: &str) -> Value {
    let s = symbol.trim().to_uppercase();
    let status = clarity()
        .lookup
        .get(&s)
        .cloned()
        .unwrap_or_else(|| auto_classify(&s).to_string());
    let mut out = serde_json::Map::new();
    out.insert("symbol".into(), Value::String(s));
    out.insert("status".into(), Value::String(status.clone()));
    if let Some(Value::Object(meta)) = clarity().meta_value.get(&status) {
        for (k, v) in meta {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// clarity.js:72-74 — served by /api/clarity-tags.
pub fn clarity_tags() -> Value {
    let c = clarity();
    serde_json::json!({ "map": c.map_value, "meta": c.meta_value })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_classify_probes() {
        // (symbol, expected status) from Node clarityStatus(...).status
        let probes = [
            ("WBTC", "commodity"),
            ("RETH", "commodity"),
            ("CBETH", "commodity"),
            ("XMONEYUSD", "stablecoin"),
            ("EURT", "stablecoin"),
            ("GOLDX", "rwa"),
            ("TBILLX", "rwa"),
            ("FOOBAR", "commodity-likely"),
            ("MIM", "stablecoin"),
            ("USTC", "stablecoin"),
            ("", "commodity-likely"),
            ("btc", "commodity"), // lowercased then uppercased
        ];
        for (sym, want) in probes {
            let got = clarity_status(sym);
            assert_eq!(got["status"], *want, "clarity_status({sym})");
        }
    }

    #[test]
    fn map_lookup_beats_regex() {
        // USDT is in MAP as stablecoin (also matches regex) — MAP wins, same result.
        assert_eq!(clarity_status("USDT")["status"], "stablecoin");
        // H is a bare contested symbol only reachable via MAP.
        assert_eq!(clarity_status("H")["status"], "contested");
        // spread of META fields present
        assert_eq!(clarity_status("BTC")["short"], "Товар");
    }
}
