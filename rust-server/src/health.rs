//! Port of server/health.js — worker liveness seen from the outside: each worker's
//! dataset file, its `status` block, and how long ago it was last written. A file
//! nobody has touched for >45 min means the worker behind it has quietly stalled.

use serde_json::{json, Value};
use std::path::Path;

/// health.js WORKERS — the seven files a stalled worker would stop touching, each with the
/// age at which silence stops being normal.
///
/// The threshold used to be a flat 45 min for all seven, and that made the watchdog worse
/// than useless: scan and backtest run on a 3h cycle and active-groups on 1.5h, so all three
/// were reported stalled every single tick, healthy or not. Permanent alarm is indistinguishable
/// from no alarm — the noise is exactly what hid the one worker that had genuinely stopped.
/// So each worker is now measured against its own cycle (main.rs), doubled: one missed run is
/// slow, two is stuck.
const WORKERS: [(&str, &str, &str, i64); 7] = [
    ("scan", "wallet-asset-scan.json", "Скан активів", 360),   // 3h cycle
    ("whale", "whale-finder.json", "Мисливець на китів", 240), // 2h cycle
    ("forensics", "asset-forensics.json", "Forensics", 90),    // 30min cycle
    ("conductors", "conductors.json", "Диригенти", 45),        // 10min cycle
    ("active", "active-groups.json", "Активні групи", 180),    // 1.5h cycle
    ("graph", "graph-db.json", "Граф БД", 45),                 // continuous (1 node / 6s)
    ("backtest", "backtest.json", "Бектест", 360),             // 3h cycle
];

/// health.js:24-30 — one worker's row. `age_min` of `None` means "we could not read
/// an mtime", which counts as stale, same as the JS `ageMin == null || ageMin > max_age`.
fn entry(key: &str, label: &str, exists: bool, status: &Value, age_min: Option<i64>, max_age: i64) -> Value {
    if !exists {
        return json!({"key": key, "label": label, "exists": false, "ok": false, "ageMin": null});
    }
    let stale = age_min.is_none_or(|a| a > max_age);
    let pick = |k: &str| status.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "key": key,
        "label": label,
        "exists": true,
        "processed": pick("processed"),
        "total": pick("total"),
        "running": pick("running"),
        "ageMin": age_min,
        "ok": !stale,
    })
}

/// health.js `health()` — `{workers, healthy, total}`.
pub fn health(dataset_dir: &Path, now_ms: i64) -> Value {
    let workers: Vec<Value> = WORKERS
        .iter()
        .map(|(key, file, label, max_age)| {
            let p = dataset_dir.join(file);
            if !p.exists() {
                return entry(key, label, false, &Value::Null, None, *max_age);
            }
            // A file that exists but won't parse still has an mtime — the JS keeps
            // `status = null` and the age, so the row can still be "ok".
            let status = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .map(|j| j.get("status").cloned().unwrap_or(Value::Null))
                .unwrap_or(Value::Null);
            let age_min = std::fs::metadata(&p)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    let mtime = d.as_millis() as i64;
                    // Math.round((now - mtime) / 60000)
                    crate::jsmath::js_round((now_ms - mtime) as f64 / 60000.0) as i64
                });
            entry(key, label, true, &status, age_min, *max_age)
        })
        .collect();
    let healthy = workers.iter().filter(|w| w["ok"] == json!(true)).count();
    json!({"workers": workers, "healthy": healthy, "total": workers.len()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_threshold_and_shape() {
        let st = json!({"running": true, "processed": 6, "total": 520});
        // The worker's own threshold decides: at the limit still ok, one minute past it stale.
        assert_eq!(entry("k", "L", true, &st, Some(45), 45)["ok"], json!(true));
        assert_eq!(entry("k", "L", true, &st, Some(46), 45)["ok"], json!(false));
        // An unreadable mtime is stale, not ok.
        assert_eq!(entry("k", "L", true, &st, None, 45)["ok"], json!(false));
        assert_eq!(entry("k", "L", true, &st, Some(1), 45)["processed"], json!(6));

        // A missing file reports exists:false and carries no status fields at all.
        let miss = entry("k", "L", false, &Value::Null, None, 45);
        assert_eq!(miss, json!({"key": "k", "label": "L", "exists": false, "ok": false, "ageMin": null}));

        // A file with no `status` block keeps the keys but leaves them null.
        let none = entry("k", "L", true, &Value::Null, Some(3), 45);
        assert_eq!(none["processed"], Value::Null);
        assert_eq!(none["ok"], json!(true));
    }

    /// The regression that made the watchdog noise: a 3h-cycle worker written 100 min ago is
    /// perfectly healthy, and the old flat 45-min rule called it stalled every single tick.
    #[test]
    fn a_long_cycle_worker_is_not_stalled_just_because_it_is_slow() {
        let st = json!({"running": false});
        let (scan, whale, active, backtest) = (360, 240, 180, 360);
        for max_age in [scan, whale, active, backtest] {
            assert_eq!(entry("k", "L", true, &st, Some(100), max_age)["ok"], json!(true));
        }
        // But genuinely stuck is still stuck: active-groups silent for 44h.
        assert_eq!(entry("active", "Активні групи", true, &st, Some(2635), active)["ok"], json!(false));
        // And a fast worker gets no free pass from the change.
        assert_eq!(entry("conductors", "Диригенти", true, &st, Some(100), 45)["ok"], json!(false));
    }
}
