//! Port of server/strategy-phases.js — a slow background scan of each satellite
//! candidate's DCA phase, cached so /api/portfolio-strategy can answer instantly.
//!
//! This is the `flowTilt` the portfolio kernel takes: accumulation → overweight,
//! distribution/exit → underweight. Without this worker the tilt was always empty,
//! so the satellite sleeve fell back to pure size/CLARITY weighting.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

struct State {
    file: PathBuf,
    phases: serde_json::Map<String, Value>, // SYM -> {phase, E, ts}
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { file: PathBuf::from("strategy-phases.json"), phases: serde_json::Map::new() }))
}

pub fn init(dataset_dir: &Path) {
    let file = dataset_dir.join("strategy-phases.json");
    let phases = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut s = state().lock().unwrap();
    s.file = file;
    s.phases = phases;
}

/// index.js:997 — `getPhases()` is only ever consumed as the flow tilt
/// (`for (const k in ph) flowTilt[k] = ph[k].phase`), so that is what we expose.
pub fn flow_tilt() -> Value {
    let s = state().lock().unwrap();
    let mut out = serde_json::Map::new();
    for (sym, v) in &s.phases {
        out.insert(sym.clone(), v["phase"].clone());
    }
    Value::Object(out)
}

/// Record one asset's phase and persist. Returns the PREVIOUS phase, so the caller
/// can alert only on a change (the JS never alerts on the first scan).
pub fn record(sym: &str, phase: &Value, exit_ratio: &Value, now: i64) -> Option<String> {
    let mut s = state().lock().unwrap();
    let prev = s.phases.get(sym).and_then(|p| p["phase"].as_str()).map(String::from);
    s.phases.insert(sym.to_string(), json!({"phase": phase, "E": exit_ratio, "ts": now}));
    if let Ok(text) = serde_json::to_string(&Value::Object(s.phases.clone())) {
        let _ = crate::atomicw::write(&s.file, text);
    }
    prev
}

pub fn len() -> usize {
    state().lock().unwrap().phases.len()
}

/// strategy-phases.js `notifyPhase` — only these three phases are actionable.
/// Returns the alert to broadcast, or None for a phase nobody needs to hear about.
pub fn notify_phase(sym: &str, phase: &str, e: &Value, now: i64) -> Option<Value> {
    let (icon, title, body) = match phase {
        "accumulation" => (
            "🟢",
            format!("{sym}: кити почали накопичувати"),
            "Сигнал на вхід (DCA-IN) — синхронне накопичення.".to_string(),
        ),
        "distribution" => ("🟠", format!("{sym}: кити почали роздавати"), format!("Час скорочувати позицію (E={e}%).")),
        "exit" => (
            "🔴",
            format!("{sym}: 48% емісії вийшло — ВИХІД"),
            format!("Кити роздали ~половину. Повний вихід (E={e}%)."),
        ),
        _ => return None,
    };
    Some(json!({
        "type": "group-alert",
        "category": "strategy-phase",
        "icon": icon,
        "title": title,
        "body": body,
        "ts": now,
    }))
}

/// The Telegram body for a phase alert — strategy-phases.js sends the same text it shows.
pub fn phase_telegram(alert: &Value) -> String {
    format!(
        "{} <b>{}</b>\n{}",
        alert["icon"].as_str().unwrap_or(""),
        alert["title"].as_str().unwrap_or(""),
        alert["body"].as_str().unwrap_or("")
    )
}

/// strategy-phases.js `run` — the satellite candidates: contract-backed, non-stable,
/// and not BTC/ETH (they are core, never satellite), capped at 30.
pub fn scan_assets(universe: &[Value]) -> Vec<Value> {
    universe
        .iter()
        .filter(|a| {
            let sym = a["sym"].as_str().unwrap_or("");
            crate::bs::truthy(&a["c"]) && a["sector"] != json!("Stable") && !matches!(sym, "BTC" | "ETH")
        })
        .take(30)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_assets_drops_stables_and_the_core_pair() {
        let uni = json!([
            {"sym": "BTC", "c": "0x1", "sector": "L1"},
            {"sym": "ETH", "c": "0x2", "sector": "L1"},
            {"sym": "USDC", "c": "0x3", "sector": "Stable"},
            {"sym": "UNI", "c": "0x4", "sector": "DeFi"},
            {"sym": "NOC", "sector": "DeFi"}, // no contract → skipped
        ]);
        let got = scan_assets(uni.as_array().unwrap());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["sym"], json!("UNI"));
    }

    #[test]
    fn only_actionable_phases_alert() {
        assert!(notify_phase("UNI", "neutral", &json!(10), 0).is_none());
        assert!(notify_phase("UNI", "", &json!(10), 0).is_none());

        let a = notify_phase("UNI", "exit", &json!(48), 1).unwrap();
        assert_eq!(a["icon"], json!("🔴"));
        assert_eq!(a["title"], json!("UNI: 48% емісії вийшло — ВИХІД"));
        assert_eq!(a["body"], json!("Кити роздали ~половину. Повний вихід (E=48%)."));

        let g = notify_phase("LINK", "accumulation", &json!(3), 1).unwrap();
        assert_eq!(g["icon"], json!("🟢"));
        assert_eq!(g["category"], json!("strategy-phase"));
    }
}
