//! Розбір відповідей Wallet Radar у типи.
//!
//! Правило одне: **копіюємо лише перелічені поля**. Whitelist тут не масив, який
//! треба тримати в синхроні з джерелом, а сам факт взяття поля: якщо ключа немає
//! в коді — немає його й у результаті. Адреси просто не копіюються.
//!
//! Поруч рахуємо, які «заборонені» ключі прийшли у відповіді (`address`,
//! `txHash`, `counterparty`, …). Це не помилка — джерело має право віддавати своє —
//! але цифра потрібна, щоб бачити: якщо раптом з'явиться нове поле з адресою, ми
//! про це дізнаємося з логу, а не з інциденту.

use crate::types::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Ключі, які ніколи не мають залишитися в результаті.
pub const DENIED_KEYS: &[&str] = &[
    "address",
    "addresses",
    "txHash",
    "tx_hash",
    "counterparty",
    "counterpartyEntity",
    "contract",
    "root",
    "wallet",
    "walletId",
    "walletAddress",
    "explorerUrl",
    "url",
    "link",
    "tx",
];

/// Короткий стабільний ідентифікатор для сутностей, у яких джерело дає адресу.
/// Адреса не зберігається — лише хеш, за яким можна впізнати ту саму сутність
/// між оновленнями.
pub fn opaque_id(prefix: &str, raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b":");
    h.update(raw.trim().to_lowercase().as_bytes());
    let digest = h.finalize();
    format!("{prefix}_{}", hex::encode(&digest[..8]))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn f64_field(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| x.as_f64())
}

fn u32_field(v: &Value, key: &str) -> Option<u32> {
    v.get(key).and_then(|x| x.as_u64()).map(|n| n as u32)
}

fn i64_field(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| x.as_i64())
}

/// Перевірити, чи відповідь не містить ключів, які мали б бути вирізані.
/// Повертає унікальні знайдені ключі — для логу.
pub fn denied_keys_present(v: &Value) -> Vec<&'static str> {
    let mut found = Vec::new();
    walk(v, 0, &mut found);
    found.sort_unstable();
    found.dedup();
    found
}

fn walk(v: &Value, depth: usize, found: &mut Vec<&'static str>) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if let Some(d) = DENIED_KEYS.iter().find(|d| d.eq_ignore_ascii_case(k)) {
                    found.push(d);
                }
                walk(val, depth + 1, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, depth + 1, found);
            }
        }
        _ => {}
    }
}

/// Подія з `/api/events` або з WS-кадру.
pub fn parse_event(v: &Value) -> Option<RawEvent> {
    let id = str_field(v, "id")?;
    if id.is_empty() {
        return None;
    }
    let wallet_id = str_field(v, "walletId").unwrap_or_else(|| "unknown".into());
    let network = str_field(v, "network")
        .map(|s| Network::parse(&s))
        .unwrap_or(Network::Other("unknown".into()));
    let flow = str_field(v, "kind")
        .map(|s| FlowKind::parse(&s))
        .unwrap_or(FlowKind::SelfTransfer);
    let asset_symbol = str_field(v, "asset").unwrap_or_default();
    let chain = str_field(v, "network").unwrap_or_default().to_lowercase();
    let amount = f64_field(v, "amount").unwrap_or(0.0);
    let value_usd = f64_field(v, "valueUsd").unwrap_or(0.0);
    let ts = i64_field(v, "timestamp").unwrap_or(0);
    Some(RawEvent {
        id,
        wallet_id,
        network,
        flow,
        asset_symbol,
        chain,
        amount,
        value_usd,
        ts,
    })
}

/// Розбір масиву подій.
pub fn parse_events(v: &Value) -> Vec<RawEvent> {
    v.as_array()
        .map(|items| items.iter().filter_map(parse_event).collect())
        .unwrap_or_default()
}

/// `/api/signals` → сімейні сигнали.
pub fn parse_signals(v: &Value) -> Vec<FamilySignal> {
    let items = match v.get("familySignals").and_then(|x| x.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|f| {
            let root = str_field(f, "root")?;
            Some(FamilySignal {
                family_id: opaque_id("fam", &root),
                size: u32_field(f, "size").unwrap_or(0),
                volume_usd: f64_field(f, "volumeUsd").unwrap_or(0.0),
                phase: str_field(f, "phase")
                    .map(|p| Phase::parse(&p))
                    .unwrap_or(Phase::Other),
                sector: str_field(f, "sector"),
                transfers: u32_field(f, "transfers").unwrap_or(0),
                top_emission_pct: f64_field(f, "topEmission"),
            })
        })
        .collect()
}

/// `/api/whale-assets` → активи з концентрацією.
pub fn parse_whale_assets(v: &Value) -> Vec<WhaleAsset> {
    let items = match v.get("assets").and_then(|x| x.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|a| {
            let symbol = str_field(a, "symbol")?;
            Some(WhaleAsset {
                name: str_field(a, "name").unwrap_or_else(|| symbol.clone()),
                symbol,
                top10_pct: f64_field(a, "top10pct").unwrap_or(0.0),
                top50_pct: f64_field(a, "top50pct").unwrap_or(0.0),
                band: str_field(a, "band")
                    .map(|b| ConcentrationBand::parse(&b))
                    .unwrap_or(ConcentrationBand::Low),
                sector: str_field(a, "sector"),
            })
        })
        .collect()
}

/// `/api/capital-groups` → групи. Адреси учасників не копіюються: з них лишається
/// лише кількість і хеш складу.
pub fn parse_capital_groups(v: &Value) -> Vec<CapitalGroup> {
    let items = match v.get("groups").and_then(|x| x.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    items
        .iter()
        .map(|g| {
            let members = g.get("members").and_then(|x| x.as_array());
            let member_count = members.map(|m| m.len() as u32).unwrap_or(0);
            let group_id = opaque_id(
                "grp",
                &members
                    .map(|m| {
                        m.iter()
                            .filter_map(|x| str_field(x, "address"))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default(),
            );
            CapitalGroup {
                group_id,
                size: u32_field(g, "size").unwrap_or(member_count),
                member_count,
                common_assets: g
                    .get("commonAssets")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// `/api/cross-asset` → рядки по символах (без адрес).
pub fn parse_cross_asset(v: &Value) -> Vec<CrossAssetRow> {
    let items = match v.get("multiAsset").and_then(|x| x.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut rows: Vec<CrossAssetRow> = Vec::new();
    for item in items {
        let count = u32_field(item, "count").unwrap_or(0);
        let total_share = f64_field(item, "totalSharePct").unwrap_or(0.0);
        let assets = item.get("assets").and_then(|x| x.as_array());
        if let Some(assets) = assets {
            for a in assets {
                if let Some(symbol) = str_field(a, "symbol") {
                    let share = f64_field(a, "share").unwrap_or(0.0);
                    rows.push(CrossAssetRow {
                        symbol: symbol.clone(),
                        holders: count,
                        share_pct: if total_share > 0.0 {
                            share / total_share * 100.0
                        } else {
                            share
                        },
                    });
                }
            }
        }
    }
    rows
}

fn parse_liq_window(v: &Value) -> Option<LiqWindow> {
    let w = v.as_object()?;
    Some(LiqWindow {
        long_usd: w.get("long").and_then(|x| x.as_f64()).unwrap_or(0.0),
        short_usd: w.get("short").and_then(|x| x.as_f64()).unwrap_or(0.0),
        total_usd: w.get("total").and_then(|x| x.as_f64()).unwrap_or(0.0),
        count: w.get("count").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
    })
}

/// `/api/liquidations` → зріз ліквідацій.
pub fn parse_liquidations(v: &Value) -> Option<LiqSnapshot> {
    let w5m = parse_liq_window(v.get("w5m")?)?;
    let w1h = parse_liq_window(v.get("w1h")?)?;
    let w24h = parse_liq_window(v.get("w24h")?)?;
    Some(LiqSnapshot { w5m, w1h, w24h })
}

/// `/api/meta-subnets` → архетипи.
pub fn parse_archetypes(v: &Value) -> Vec<Archetype> {
    let items = match v.get("archetypes").and_then(|x| x.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|a| {
            let name = str_field(a, "name").or_else(|| a.as_str().map(|s| s.to_string()))?;
            Some(Archetype {
                name,
                members: u32_field(a, "members")
                    .or_else(|| u32_field(a, "size"))
                    .unwrap_or(0),
            })
        })
        .collect()
}

/// Кадр із WS: `{"type":"init","events":[...]}` або `{"type":"event","event":{…}}`.
pub fn parse_server_msg(v: &Value) -> ServerMsg {
    match v.get("type").and_then(|x| x.as_str()) {
        Some("init") => ServerMsg::Init {
            events: parse_events(v.get("events").unwrap_or(&Value::Null)),
        },
        Some("event") => match v.get("event").and_then(parse_event) {
            Some(ev) => ServerMsg::Event(ev),
            None => ServerMsg::Other,
        },
        _ => ServerMsg::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_parsing_takes_only_whitelisted_fields() {
        let v = json!({
            "id": "0xabc:eth:nat",
            "walletId": "wmrg3e7qb",
            "network": "eth",
            "kind": "out",
            "asset": "ETH",
            "amount": 1.5,
            "valueUsd": 4200.0,
            "timestamp": 1794180119000i64,
            "txHash": "0xdeadbeef",
            "counterparty": "0x611f7bf868a6212f871e89f7e44684045ddfb09d"
        });
        let ev = parse_event(&v).expect("подія парситься");
        assert_eq!(ev.wallet_id, "wmrg3e7qb");
        assert_eq!(ev.value_usd, 4200.0);
        // Сирих полів у типі немає — перевіряємо, що вони й не прийшли в текст.
        let debug = format!("{ev:?}");
        assert!(
            !debug.contains("deadbeef"),
            "txHash не має просочуватися: {debug}"
        );
        assert!(
            !debug.contains("611f7bf8"),
            "адреса не має просочуватися: {debug}"
        );
    }

    #[test]
    fn denied_keys_are_reported_for_observability() {
        let v = json!({"groups": [{"members": [{"address": "0x1"}]}]});
        let found = denied_keys_present(&v);
        assert!(found.contains(&"address"));
    }

    #[test]
    fn groups_keep_counts_but_not_addresses() {
        let v = json!({"groups": [{
            "size": 2,
            "members": [
                {"address": "0x611f7bf868a6212f871e89f7e44684045ddfb09d", "count": 11},
                {"address": "0x1d48963dd8fada6ab5c2c7b92eba81ecc5030270", "count": 3}
            ],
            "commonAssets": ["UNI", "SHIB"]
        }]});
        let groups = parse_capital_groups(&v);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].member_count, 2);
        assert_eq!(groups[0].common_assets, vec!["UNI", "SHIB"]);
        let debug = format!("{:?}", groups[0]);
        assert!(
            !debug.contains("611f7bf8"),
            "адреса не має просочуватися: {debug}"
        );
        assert!(groups[0].group_id.starts_with("grp_"));
    }

    #[test]
    fn group_id_is_stable_across_parses() {
        let v = json!({"groups": [{"members": [{"address": "0xabc"}]}]});
        let a = parse_capital_groups(&v);
        let b = parse_capital_groups(&v);
        assert_eq!(a[0].group_id, b[0].group_id);
    }

    #[test]
    fn signals_hash_the_root_instead_of_exposing_it() {
        let v = json!({"familySignals": [{
            "root": "0x611f7bf868a6212f871e89f7e44684045ddfb09d",
            "size": 5, "volumeUsd": 1000.0, "phase": "accumulating",
            "sector": "DeFi", "transfers": 42, "topEmission": 6.2
        }]});
        let s = parse_signals(&v);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].phase, Phase::Accumulating);
        assert_eq!(s[0].top_emission_pct, Some(6.2));
        assert!(!format!("{:?}", s[0]).contains("611f7bf8"));
    }

    #[test]
    fn server_msg_dispatch() {
        let init = parse_server_msg(&json!({"type": "init", "events": [
            {"id": "a", "network": "eth", "kind": "in", "asset": "SOL", "timestamp": 1}
        ]}));
        assert!(matches!(init, ServerMsg::Init { ref events } if events.len() == 1));

        let ev = parse_server_msg(&json!({"type": "event", "event": {
            "id": "b", "network": "eth", "kind": "out", "asset": "SOL", "timestamp": 2
        }}));
        assert!(matches!(ev, ServerMsg::Event(_)));

        // Невідомий тип — не помилка, а «інше».
        assert!(matches!(
            parse_server_msg(&json!({"type": "group"})),
            ServerMsg::Other
        ));
    }
}
