#![allow(dead_code)]
//! Token sector / narrative classification for the Smart Money view.
//!
//! Axis: `depin | memecoin | defi | rwa | other` — a *narrative* axis, distinct
//! from `clarity.rs`'s regulatory axis (commodity/stablecoin/…). Hybrid source,
//! in priority order:
//!   1. a curated `SYMBOL -> sector` map (high-confidence majors),
//!   2. a disk cache learned from CoinGecko (`dataset/sector-cache.json`),
//!   3. `universe.json`'s own `sector` field, mapped into the five buckets,
//!   4. otherwise `other`.
//!
//! Unknowns (that fall through to `other` with a real contract) are resolved in
//! the background against CoinGecko's contract endpoint and written to the cache,
//! so coverage self-heals over time without ever blocking a request.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

use serde_json::Value;

/// The five buckets the Smart Money view groups by.
// «ai» додано 01.09.2026: хайризиковий ешелон каскаду складається переважно з
// AI-активів, а без власного сектора вони губились серед "other" — тобто саме те,
// що треба було виділити, було невидимим.
pub const SECTORS: [&str; 6] = ["depin", "memecoin", "defi", "rwa", "ai", "other"];

fn curated() -> &'static HashMap<&'static str, &'static str> {
    static M: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        let mut add = |list: &[&'static str], sec: &'static str| {
            for s in list {
                m.insert(*s, sec);
            }
        };
        // AI та агенти. Список курований: автоматично за назвою їх не відрізнити —
        // половина проєктів пише «AI» в описі, не будучи ним.
        add(
            &[
                "TAO", "FET", "AGIX", "OCEAN", "RNDR", "RENDER", "AKT", "GRT", "ARKM", "AI16Z",
                "VIRTUAL", "TRAC", "NMR", "CTXC", "PHB", "AIOZ", "PAAL", "TURBO", "ALI", "GNO",
                "SENT", "ZKP", "THQ", "ERA", "XCN", "CPOOL", "OGN", "CYBER", "BIGTIME", "ALT",
            ],
            "ai",
        );
        add(
            &[
                "SHIB", "PEPE", "DOGE", "FLOKI", "BONK", "WIF", "MOG", "BRETT", "POPCAT", "MEME",
                "ELON", "TURBO", "SPX", "PONKE", "MEW", "NEIRO", "GIGA", "PNUT", "GOAT",
                "FARTCOIN", "BABYDOGE", "LADYS", "WOJAK", "ANDY", "TOSHI", "DEGEN", "HARRY",
                "MICHI", "BOBO", "APU", "AIDOGE", "PORK", "SNEK",
            ],
            "memecoin",
        );
        add(
            &[
                "UNI", "AAVE", "MKR", "SKY", "CRV", "CVX", "COMP", "SNX", "LDO", "SUSHI", "BAL",
                "1INCH", "DYDX", "GMX", "PENDLE", "FXS", "RPL", "AERO", "VELO", "MORPHO", "ENA",
                "CAKE", "SPELL", "YFI", "BADGER", "ALCX", "LQTY", "RUNE", "OSMO", "GNS",
                "SILO", "EUL", "FLUID", "COW", "PRISMA", "STG", "RDNT", "ZRX", "KNC",
            ],
            "defi",
        );
        add(
            &[
                "ONDO", "BUIDL", "OUSG", "USYC", "USTBL", "JAAA", "FDIT", "USDY", "XAUT",
                "PAXG", "KAU", "KAG", "POLYX", "RIO", "CFG", "MPL", "TRU", "GFI", "OM", "PROPS",
                "VBILL", "JTRSY", "THBILL", "BENJI", "TBILL", "USTB", "WTGXX", "SAFO", "EURSAFO",
                "BDX",
            ],
            "rwa",
        );
        add(
            &[
                "RENDER", "RNDR", "FIL", "AR", "HNT", "IOTX", "AKT", "THETA", "TFUEL", "GRT",
                "POKT", "LPT", "DIMO", "NOS", "IO", "AIOZ", "PEAQ", "GEOD", "XNET", "DBR", "ATH",
                "JASMY", "MOBILE", "IOT", "WIFI", "HONEY", "NKN", "STORJ", "GLM", "FLUX",
                "SHDW", "RSS3",
            ],
            "depin",
        );
        // top-100 whale-assets expansion (2026-07-18): tokens that were falling
        // through to  but do belong to one of the four narratives.
        add(&["PENGU"], "memecoin");
        add(
            &["WLFI", "DEXE", "ETHFI", "OHM", "SYRUP", "FF", "EIGEN"],
            "defi",
        );
        add(&["EUTBL", "XAUM", "VNXAU", "XAGM", "XGZ"], "rwa");
        // Majors CoinGecko mis-tags for this axis (stablecoins -> DeFi; oracle /
        // tokenization infra like LINK -> RWA). Pin them so the curated map (top
        // priority) stays authoritative and CoinGecko can never override them.
        add(
            &[
                "USDT", "USDC", "DAI", "USDS", "USDE", "USDG", "PYUSD", "USD1", "USDD",
                "RLUSD", "USDF", "TUSD", "FDUSD", "USD0", "GHO", "FRAX", "CRVUSD", "EURC",
                "AUSD", "USDTB", "USDAI", "APXUSD", "USAT", "USDM", "LINK", "QNT", "WLD",
                "ICP",
            ],
            "other",
        );
        // Broader high-confidence coverage across the full whale-asset universe.
        add(
            &[
                "NPC", "KISHU", "KENDU", "PEIPEI", "CAW", "CATE", "KEKIUS", "MANYU",
                "OMIKAMI", "PEPECOIN", "DOG", "OSAK", "BONE",
            ],
            "memecoin",
        );
        add(
            &[
                "QUICK", "HEGIC", "LEND", "SOLV", "BIFI", "MCB", "AUTO", "XVS", "BEL",
                "NXM", "TORN", "SDEX", "SDT", "ANGLE", "DODO", "DRV", "TEMPLE", "SYN",
                "FARM", "BNT", "MLN", "GEAR", "GTC", "PUFFER", "POLS", "SD", "HFT",
                "UMA", "DDX", "OGN", "AEVO", "REZ", "SWELL", "KRL", "CPOOL", "RESOLV",
                "DPI", "SPK", "USUAL",
            ],
            "defi",
        );
        add(
            &[
                "PHA", "HOPR", "RLC", "ANKR", "TRAC", "HOT", "XYO", "OCEAN", "BLZ",
                "MVL", "AUDIO",
            ],
            "depin",
        );
        m
    })
}

/// `universe.json` sector → our bucket. Loaded once.
fn universe_map(root: &Path) -> &'static HashMap<String, &'static str> {
    static U: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    U.get_or_init(|| {
        let mut m = HashMap::new();
        if let Ok(txt) = std::fs::read_to_string(root.join("universe.json")) {
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&txt) {
                for a in arr {
                    let sym = a["sym"].as_str().unwrap_or("").to_uppercase();
                    let bucket = match a["sector"].as_str().unwrap_or("") {
                        "Meme" => "memecoin",
                        "DeFi" => "defi",
                        "RWA" => "rwa",
                        // universe has no DePIN sector of its own; everything else
                        // (L1/L2/AI/Oracle/Gaming/Infra/Stable) is not one of our
                        // four narratives → other.
                        _ => continue,
                    };
                    if !sym.is_empty() {
                        m.insert(sym, bucket);
                    }
                }
            }
        }
        m
    })
}

static CACHE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
static ROOT: OnceLock<PathBuf> = OnceLock::new();
static ATTEMPTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Load the disk cache once and remember the dataset root. Idempotent — safe to
/// call on every request.
pub fn init(root: &Path) {
    let _ = ROOT.set(root.to_path_buf());
    CACHE.get_or_init(|| {
        let mut m = HashMap::new();
        if let Ok(txt) = std::fs::read_to_string(root.join("sector-cache.json")) {
            if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&txt) {
                for (k, v) in o {
                    if let Some(s) = v.as_str() {
                        m.insert(k, s.to_string());
                    }
                }
            }
        }
        RwLock::new(m)
    });
    // Warm the universe map too (cheap, once).
    let _ = universe_map(root);
}

/// Tokenized real-world assets are reliably detectable by NAME (tokenized
/// treasuries / stocks / ETFs / gold / bond funds), which scales to the whole
/// whale-asset universe without hand-listing every ticker.
fn rwa_name_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        // No `\b` word-boundaries: they do not survive the deploy path, and a
        // bare gold/silver term would over-match gaming tokens (Adventure Gold).
        // Confirmed tokenized metals without a "tokenized"/"xStock" name are
        // pinned in the curated map instead.
        regex::Regex::new(
            r"(?i)treasur(y|ies)|t-?bill|tokeni[sz]ed|xstock|money market|real world|us government|government securities|short duration|overnight (swap|fund)|bullion|clo fund|credit fund|high yield bond",
        )
        .unwrap()
    })
}

/// Stablecoins are  (not a narrative). Name/symbol heuristic — only reached
/// AFTER the RWA-name check, so a tokenized T-bill named "… USD …" is not stolen.
fn is_stable(sym: &str, name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("stablecoin")
        || n.contains("stable ")
        || n.contains("dollar")
        || n.contains("tether")
        || n.contains("euro")
        || sym.starts_with("USD")
        || sym.ends_with("USD")
}

/// Classify one token. Never blocks: pure map / regex / cache lookups.
/// Priority: curated map → RWA-by-name → stablecoin-by-name → CG cache → universe.
pub fn sector_of(symbol: &str, name: &str, contract: &str) -> String {
    let sym = symbol.trim().to_uppercase();
    if let Some(s) = curated().get(sym.as_str()) {
        return (*s).to_string();
    }
    if rwa_name_re().is_match(name) {
        return "rwa".to_string();
    }
    if is_stable(&sym, name) {
        return "other".to_string();
    }
    let c = contract.trim().to_lowercase();
    if !c.is_empty() {
        if let Some(cache) = CACHE.get() {
            if let Some(s) = cache.read().unwrap().get(&c) {
                return s.clone();
            }
        }
    }
    if let Some(root) = ROOT.get() {
        if let Some(s) = universe_map(root).get(&sym) {
            return (*s).to_string();
        }
    }
    "other".to_string()
}

/// Map a CoinGecko `categories` list into one of our buckets. Order matters:
/// RWA / DePIN / meme are checked before DeFi because a token can carry several
/// tags and the narrower narrative should win.
fn map_categories(cats: &[String]) -> &'static str {
    let joined = cats.join(" | ").to_lowercase();
    let has = |n: &str| joined.contains(n);
    // Precedence matters. Stablecoins first: CoinGecko double-tags them as DeFi,
    // but here a stablecoin is , never DeFi. RWA requires an explicit
    // real-world-asset tag (not a bare "tokeniz…", which also hits oracle/infra
    // tokens like LINK). DeFi requires the full "decentralized finance" tag.
    if has("stablecoin") {
        "other"
    } else if has("meme") {
        "memecoin"
    } else if has("real world asset")
        || has("(rwa)")
        || has("tokenized treasur")
        || has("tokenized stock")
        || has("tokenized gold")
        || has("tokenized asset")
    {
        "rwa"
    } else if has("depin") || has("decentralized physical") {
        "depin"
    } else if has("decentralized finance") {
        "defi"
    } else {
        "other"
    }
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(9))
            .user_agent("wallet-radar/1.0")
            .build()
            .unwrap()
    })
}

/// CoinGecko: categories for an ERC-20 by contract. `[]` on any error / 429.
async fn cg_categories(contract: &str) -> Option<Vec<String>> {
    // Some(cats) on a real 200 (cats may be empty = genuinely no narrative tag);
    // None on 429 / transport error — a None must NEVER be cached, or a
    // classifiable token would be poisoned as  forever.
    let url = format!("https://api.coingecko.com/api/v3/coins/ethereum/contract/{contract}");
    match http().get(&url).header("accept", "application/json").send().await {
        Ok(r) if r.status().is_success() => {
            let v: Value = r.json().await.ok()?;
            Some(
                v["categories"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
            )
        }
        _ => None,
    }
}

fn save_cache() {
    let (Some(cache), Some(root)) = (CACHE.get(), ROOT.get()) else {
        return;
    };
    let map = cache.read().unwrap();
    let obj: serde_json::Map<String, Value> =
        map.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    let tmp = root.join("sector-cache.json.tmp");
    if std::fs::write(&tmp, Value::Object(obj).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, root.join("sector-cache.json"));
    }
}

/// Background: resolve up to `LIMIT` unknown (symbol, contract) pairs via CoinGecko
/// and write them to the cache. Each contract is attempted at most once per process
/// run. Gentle on the free tier (~1 req / 1.5s). Even an `other` verdict is cached,
/// so we do not re-query a token CoinGecko simply has no narrative tag for.
pub async fn enrich(missing: Vec<(String, String)>) {
    const LIMIT: usize = 25;
    let att = ATTEMPTED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut did = 0usize;
    let mut fails = 0u32;
    let mut wrote = false;
    for (_sym, contract) in missing {
        if did >= LIMIT || fails >= 4 {
            break; // hit a 429 wall — leave the rest for the next request
        }
        let c = contract.trim().to_lowercase();
        if c.is_empty() {
            continue;
        }
        if att.lock().unwrap().contains(&c) {
            continue; // already resolved this run
        }
        match cg_categories(&c).await {
            Some(cats) => {
                let sec = map_categories(&cats);
                if let Some(cache) = CACHE.get() {
                    cache.write().unwrap().insert(c.clone(), sec.to_string());
                }
                att.lock().unwrap().insert(c);
                wrote = true;
                did += 1;
                fails = 0;
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
            None => {
                fails += 1;
                tokio::time::sleep(Duration::from_millis(3000)).await;
            }
        }
    }
    if wrote {
        save_cache();
    }
}
