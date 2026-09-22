use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Port of server/labels.js. Public address labels imported from
/// brianleect/etherscan-labels and cached at dataset/labels-cache.json.
///
/// The map is behind a lock because the import worker REPLACES it every 12h. It
/// used to be loaded once at boot and never refreshed, which is worse than it
/// sounds: with no cache file the map is empty, `is_infra_addr` then returns false
/// for everything, and /api/cross-asset stops filtering exchanges out — reporting
/// them as "orchestrators". Wrong data, not merely stale.
pub struct Labels {
    map: RwLock<HashMap<String, LabelEntry>>,
    infra: Regex,
    file: PathBuf,
}

/// labels.js:2 — the public source, keyless.
const LABELS_URL: &str =
    "https://raw.githubusercontent.com/brianleect/etherscan-labels/main/data/etherscan/combined/combinedAllLabels.json";

struct LabelEntry {
    name: String,
    labels: Vec<String>,
}

// CEX_NAMES + INFRA_NAMES from server/labels.js:20-40, joined exactly as the JS does.
// Anything matching is infrastructure (exchange / bridge / market maker / protocol /
// scam) and can never be a conductor or a whale.
//
// This list is named, not categorical, because the label dataset has no "exchange" tag:
// Bittrex's tag is literally ["bittrex"]. An exchange missing from the list passes
// straight through — which is how Bittrex and Poloniex once ranked as "conductors".
// Never rely on it alone; conductor-finder pairs it with a behavioural filter.
const CEX_NAMES: &str = r"binance|coinbase|okx|okex|kraken|bybit|huobi|htx|kucoin|gate\.io|gateio|bitfinex|gemini|bitstamp|mexc|bitget|crypto-?com|robinhood|bittrex|poloniex|upbit|bithumb|coinone|korbit|coincheck|bitflyer|liquid|zaif|bitbank|hitbtc|bitmart|lbank|xt-?com|probit|whitebit|exmo|cex-?io|digifinex|bitrue|ascendex|bitmex|deribit|phemex|bingx|bitso|buenbit|luno|valr|bitkub|indodax|wazirx|coindcx|zebpay|btcturk|paribu|coinsbit|p2pb2b|bitpanda|kanga|zonda|yobit|livecoin|tidex|graviex|stex|coinex|bkex|bibox|bigone|hotbit|poloniex|cryptopia|idex|etherdelta|shapeshift|changelly|simpleswap|swapspace|godex|letsexchange|stealthex|quickex|alfacashier";
const INFRA_NAMES: &str = r"exchange|deposit|hot-?wallet|cold-?wallet|custody|bridge|router|relayer|wintermute|jump|cumberland|alameda|market-?maker|mev|flashbots|galaxy|amber|gsr|dwf|keyrock|b2c2|genesis|falconx|relay|gateway|hop|across|stargate|celer|multichain|synapse|wormhole|layerzero|orbiter|peg|vault|staking|treasury|fund|defi|dex|uniswap|sushiswap|curve|aave|compound|maker|lido|rocket|tornado|fixed-?float|changenow|sideshift|disperse|contract|airdrop-?hunter|phish|hack|scam|exploit|spam|sanction|blocked|ofac";

/// labels.js:42 INFRA_TAGS — tags that genuinely exist in the source taxonomy and mean
/// "infrastructure", not "whale". Matched by equality, unlike the name regex.
const INFRA_TAGS: [&str; 21] = [
    "exchange",
    "cex",
    "dex",
    "bridge",
    "mev-bot",
    "staking",
    "old-contract",
    "token-sale",
    "ico-wallets",
    "airdrop-hunter",
    "blocked",
    "phish-hack",
    "heist",
    "sanctioned",
    "yield-farming",
    "protocol",
    "sybil-delegate",
    "mining",
    "payments",
    "bridged-token",
    "stablecoin",
];

/// `{addr.toLowerCase(): {name, labels}}` → the in-memory map.
fn parse_labels(obj: serde_json::Map<String, Value>) -> HashMap<String, LabelEntry> {
    obj.into_iter()
        .map(|(addr, v)| {
            (
                addr.to_lowercase(),
                LabelEntry {
                    name: v["name"].as_str().unwrap_or_default().to_string(),
                    labels: v["labels"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|l| l.as_str().map(str::to_string)).collect())
                        .unwrap_or_default(),
                },
            )
        })
        .collect()
}

impl Labels {
    /// A missing or malformed cache yields an empty map, matching the JS `catch {}`.
    pub fn load(dataset_dir: &Path) -> Self {
        let infra = Regex::new(&format!("(?i){CEX_NAMES}|{INFRA_NAMES}")).expect("the name lists are compile-time constants");
        let file = dataset_dir.join("labels-cache.json");
        let map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| match v {
                Value::Object(obj) => Some(parse_labels(obj)),
                _ => None,
            })
            .unwrap_or_default();
        Self { map: RwLock::new(map), infra, file }
    }

    pub fn len(&self) -> usize {
        self.map.read().unwrap().len()
    }

    /// labels.js `refreshLabels` — download the public label set, swap it in, and
    /// rewrite the disk cache. Every failure is silent, as in the JS: we keep the
    /// labels we already have rather than blanking them.
    pub async fn refresh(&self) {
        let client = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build() {
            Ok(c) => c,
            Err(_) => return,
        };
        let resp = match client.get(LABELS_URL).header("accept", "application/json").send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return,
        };
        let Ok(Value::Object(obj)) = resp.json::<Value>().await else { return };
        // Write the cache in the same shape the JS does, so either server can read it.
        if let Ok(text) = serde_json::to_string(&Value::Object(obj.clone())) {
            let _ = crate::atomicw::write(&self.file, text);
        }
        let map = parse_labels(obj);
        let n = map.len();
        *self.map.write().unwrap() = map;
        tracing::info!("[labels] imported {n} public labels");
    }

    /// labels.js `infraReason(addr)` — WHY an address counts as infrastructure
    /// (`label:<name>` / `tag:<tag>`), or None. The reason travels into the data as
    /// evidence, so conductor rejections are auditable rather than a bare boolean.
    pub fn infra_reason(&self, addr: &str) -> Option<String> {
        let map = self.map.read().unwrap();
        let v = map.get(&addr.to_lowercase())?;
        if self.infra.is_match(&v.name) {
            return Some(format!("label:{}", v.name));
        }
        let tag = v.labels.iter().find(|l| INFRA_TAGS.contains(&l.as_str()) || self.infra.is_match(l))?;
        Some(format!("tag:{tag}"))
    }

    /// labels.js `isInfraAddr` — `infraReason(addr) != null`.
    pub fn is_infra_addr(&self, addr: &str) -> bool {
        self.infra_reason(addr).is_some()
    }

    /// server/labels.js:16 `labelOf(addr)` — the public label's `name`, or None.
    /// Used by `/api/wallet-graph` for the `pub?.name` fallbacks.
    pub fn label_name(&self, addr: &str) -> Option<String> {
        let map = self.map.read().unwrap();
        map.get(&addr.to_lowercase()).map(|v| v.name.clone()).filter(|s| !s.is_empty())
    }
}
