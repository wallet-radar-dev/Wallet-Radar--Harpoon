use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::RwLock;

/// Port of server/store.js. Wallets and events stay as raw `Value` so unknown
/// fields survive a round trip untouched — the frontend reads fields this
/// server never names.
pub struct Store {
    file: PathBuf,
    data: RwLock<Value>,
}

impl Store {
    pub fn load(data_dir: PathBuf) -> Self {
        let file = data_dir.join("data.json");
        let data = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({"wallets": [], "events": [], "cursors": {}, "baselines": {}}));
        Self { file, data: RwLock::new(data) }
    }

    pub fn wallets(&self) -> Value {
        self.data.read().unwrap()["wallets"].clone()
    }

    /// store.js:45 — `data.wallets.unshift(w); persist()`. Newest-first, like events.
    pub fn add_wallet(&self, w: Value) {
        {
            let mut d = self.data.write().unwrap();
            if !d["wallets"].is_array() {
                d["wallets"] = json!([]);
            }
            d["wallets"].as_array_mut().unwrap().insert(0, w);
        }
        self.persist();
    }

    /// store.js:46 — `data.wallets = data.wallets.filter((w) => w.id !== id); persist()`.
    /// Removing an id that isn't there is a no-op, and still 200s, exactly as in Node.
    pub fn remove_wallet(&self, id: &str) {
        {
            let mut d = self.data.write().unwrap();
            if let Some(ws) = d["wallets"].as_array_mut() {
                ws.retain(|w| w["id"].as_str() != Some(id));
            }
        }
        self.persist();
    }

    /// store.js:46 — `data.events.slice(0, limit)`. Already newest-first on disk.
    pub fn events(&self, limit: usize) -> Value {
        let d = self.data.read().unwrap();
        match d["events"].as_array() {
            Some(a) => Value::Array(a.iter().take(limit).cloned().collect()),
            None => json!([]),
        }
    }

    /// store.js:47 — `data.events.unshift(ev); if len>500 truncate; persist()`.
    /// Newest-first, capped at 500, written straight through.
    pub fn add_event(&self, ev: Value) {
        {
            let mut d = self.data.write().unwrap();
            if !d["events"].is_array() {
                d["events"] = json!([]);
            }
            let events = d["events"].as_array_mut().unwrap();
            events.insert(0, ev);
            if events.len() > 500 {
                events.truncate(500);
            }
        }
        self.persist();
    }

    /// store.js:48 — `key in data.cursors ? data.cursors[key] : null`.
    pub fn get_cursor(&self, key: &str) -> Value {
        self.data.read().unwrap()["cursors"].get(key).cloned().unwrap_or(Value::Null)
    }

    /// store.js:49 — `data.cursors[key] = val; persist()`.
    pub fn set_cursor(&self, key: &str, val: Value) {
        {
            let mut d = self.data.write().unwrap();
            if !d["cursors"].is_object() {
                d["cursors"] = json!({});
            }
            d["cursors"][key] = val;
        }
        self.persist();
    }

    pub fn wallet_count(&self) -> usize {
        self.data.read().unwrap()["wallets"].as_array().map_or(0, |a| a.len())
    }

    pub fn event_count(&self) -> usize {
        self.data.read().unwrap()["events"].as_array().map_or(0, |a| a.len())
    }

    /// store.js:50 — `data.baselines[addr] || null`. Missing → `Null`.
    pub fn get_baseline(&self, addr: &str) -> Value {
        self.data.read().unwrap()["baselines"].get(addr).cloned().unwrap_or(Value::Null)
    }

    /// store.js:51 — `data.baselines[addr] = snap; persist()`. The JS debounces the
    /// write by 500 ms; we write straight through (baselines are set rarely, on an
    /// explicit `arm=1`). `preserve_order` keeps every other key in place on disk.
    pub fn set_baseline(&self, addr: &str, snap: Value) {
        {
            let mut d = self.data.write().unwrap();
            if !d["baselines"].is_object() {
                d["baselines"] = json!({});
            }
            d["baselines"][addr] = snap;
        }
        self.persist();
    }

    /// store.js `persist` — `JSON.stringify(data, null, 2)` to the data file.
    fn persist(&self) {
        let d = self.data.read().unwrap();
        if let Ok(s) = serde_json::to_string_pretty(&*d) {
            if let Err(e) = crate::atomicw::write(&self.file, s) {
                tracing::error!("persist error: {e}");
            }
        }
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &PathBuf {
        &self.file
    }
}
