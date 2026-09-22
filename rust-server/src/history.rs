//! Port of server/history.js — the file-backed time series behind every "Δ over N
//! days" number. A singleton, like the JS module: loaded from dataset/history.json
//! once, mutated in place, persisted explicitly by whoever wrote to it.
//!
//! The Rust server previously only READ history.json; nothing ever recorded into
//! it, so with Rust as sole server the series would never grow and every trend
//! (market rotation, social spike) would stay permanently cold.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// history.js CAP — ~15 days at one point per 30-minute cycle.
const CAP: usize = 720;
/// Points closer together than this merge into the last one instead of appending.
const MERGE_WINDOW_MS: i64 = 600_000;

struct State {
    file: PathBuf,
    store: serde_json::Map<String, Value>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { file: PathBuf::from("history.json"), store: serde_json::Map::new() }))
}

/// Load dataset/history.json into the singleton. Call once at boot; a missing or
/// malformed file leaves an empty store, exactly as the JS `catch { store = {} }`.
pub fn init(dataset_dir: &Path) {
    let file = dataset_dir.join("history.json");
    let store = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let mut s = state().lock().unwrap();
    s.file = file;
    s.store = store;
}

/// history.js `record(key, fields, ms)` — one point per ~30-minute cycle. A point
/// less than 10 minutes after the last one UPDATES it (fields merged, `t` bumped)
/// rather than appending, so a busy caller can't inflate the series.
pub fn record(key: &str, fields: &Value, ms: i64) {
    let mut s = state().lock().unwrap();
    let entry = s.store.entry(key.to_string()).or_insert_with(|| json!([]));
    if !entry.is_array() {
        *entry = json!([]);
    }
    let arr = entry.as_array_mut().expect("just ensured it is an array");
    let merge = arr.last().is_some_and(|l| ms - l["t"].as_i64().unwrap_or(0) < MERGE_WINDOW_MS);
    if merge {
        let last = arr.last_mut().expect("checked non-empty");
        if let (Some(dst), Some(src)) = (last.as_object_mut(), fields.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
            dst.insert("t".into(), json!(ms));
        }
    } else {
        let mut p = json!({"t": ms});
        if let (Some(dst), Some(src)) = (p.as_object_mut(), fields.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        arr.push(p);
    }
    if arr.len() > CAP {
        arr.drain(0..arr.len() - CAP);
    }
}

pub fn series(key: &str) -> Value {
    let s = state().lock().unwrap();
    s.store.get(key).cloned().unwrap_or_else(|| json!([]))
}

pub fn latest(key: &str) -> Value {
    let s = state().lock().unwrap();
    s.store.get(key).and_then(|a| a.as_array()).and_then(|a| a.last().cloned()).unwrap_or(Value::Null)
}

/// history.js `delta(key, field, sinceMs)` — how much `field` moved across the
/// window. `None` whenever the JS returns null: fewer than 2 points, the window
/// lands on the newest point itself, or either end is missing the field.
pub fn delta(key: &str, field: &str, since_ms: i64) -> Option<Value> {
    let s = state().lock().unwrap();
    let a = s.store.get(key)?.as_array()?;
    if a.len() < 2 {
        return None;
    }
    let now = a.last()?;
    // `a.find((p) => p.t >= sinceMs) || a[0]`
    let past = a.iter().find(|p| p["t"].as_i64().unwrap_or(0) >= since_ms).unwrap_or(&a[0]);
    // `past === now` — identity in JS; the newest point is the only one that can
    // alias here, so comparing timestamps is equivalent.
    if past["t"] == now["t"] || now[field].is_null() || past[field].is_null() {
        return None;
    }
    let (from, to) = (past[field].as_f64()?, now[field].as_f64()?);
    Some(json!({
        "from": past[field],
        "to": now[field],
        "diff": crate::jsmath::js_num(crate::jsmath::round_fixed(to - from, 2)),
        "fromT": past["t"],
        "toT": now["t"],
    }))
}

/// history.js `persist()` — compact JSON (no pretty-printing), best effort.
pub fn persist() {
    let s = state().lock().unwrap();
    if let Ok(text) = serde_json::to_string(&s.store) {
        let _ = crate::atomicw::write(&s.file, text);
    }
}

/// The store is a process-wide singleton, so every test that touches it — here and
/// in `market` — must hold this lock and start from a clean store, or `cargo test`'s
/// threads will interleave their points. Also parks `persist()` on a temp file.
#[cfg(test)]
pub(crate) fn reset_for_test() -> std::sync::MutexGuard<'static, ()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = L.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner());
    let mut s = state().lock().unwrap();
    s.store = serde_json::Map::new();
    s.file = std::env::temp_dir().join("wr-history-test.json");
    guard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_merges_inside_the_window_and_appends_outside() {
        let _g = reset_for_test();
        record("market", &json!({"btcDom": 50.0}), 1_000_000);
        // +5 min → merges into the same point, updating fields and t.
        record("market", &json!({"btcDom": 51.0, "ethDom": 18.0}), 1_300_000);
        let s = series("market");
        assert_eq!(s.as_array().unwrap().len(), 1);
        assert_eq!(s[0]["btcDom"], json!(51.0));
        assert_eq!(s[0]["ethDom"], json!(18.0));
        assert_eq!(s[0]["t"], json!(1_300_000));

        // +11 min from the last point → a new point.
        record("market", &json!({"btcDom": 49.0}), 1_960_000);
        assert_eq!(series("market").as_array().unwrap().len(), 2);
    }

    #[test]
    fn delta_reports_the_move_and_null_cases() {
        let _g = reset_for_test();
        record("market", &json!({"btcDom": 50.0}), 0);
        record("market", &json!({"btcDom": 48.5}), 3_600_000);
        let d = delta("market", "btcDom", 0).unwrap();
        assert_eq!(d["from"], json!(50.0));
        assert_eq!(d["to"], json!(48.5));
        assert_eq!(d["diff"], json!(-1.5)); // toFixed(2), unary + → number

        // A window that only contains the newest point → past === now → null.
        assert!(delta("market", "btcDom", 3_600_000).is_none());
        // A field neither end carries → null.
        assert!(delta("market", "ethDom", 0).is_none());
        // Fewer than 2 points → null.
        drop(_g);
        let _g2 = reset_for_test();
        record("solo", &json!({"x": 1.0}), 0);
        assert!(delta("solo", "x", 0).is_none());
    }

    #[test]
    fn series_is_capped() {
        let _g = reset_for_test();
        for i in 0..(CAP as i64 + 50) {
            // 11 minutes apart so every point appends.
            record("k", &json!({"v": i}), i * 660_000);
        }
        let s = series("k");
        let a = s.as_array().unwrap();
        assert_eq!(a.len(), CAP);
        // The oldest points were dropped from the front.
        assert_eq!(a[0]["v"], json!(50));
    }
}
