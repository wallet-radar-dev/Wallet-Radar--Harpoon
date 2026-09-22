use serde_json::Value;
use std::path::{Path, PathBuf};

/// Mirrors `readJson` in server/index.js:649 — a missing file, an unreadable
/// file, and malformed JSON are all indistinguishable to callers: they get None
/// and substitute their own default. Do not surface these as errors.
pub struct Dataset {
    root: PathBuf,
}

impl Dataset {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self { root: root.as_ref().to_path_buf() }
    }

    pub fn read(&self, file: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.root.join(file)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// `readJson(f) || fallback` as a single call.
    pub fn read_or(&self, file: &str, fallback: Value) -> Value {
        self.read(file).unwrap_or(fallback)
    }

    /// Write a value as pretty JSON to `<dir>/<file>` — used by the gated file-
    /// refreshing workers (`JSON.stringify(result, null, 2)` on the Node side).
    pub fn write(&self, file: &str, value: &Value) {
        if let Ok(s) = serde_json::to_string_pretty(value) {
            let _ = crate::atomicw::write(self.root.join(file), s);
        }
    }

    /// The dataset directory — used to lazily load large artefacts (graph-db,
    /// the smart-money model) outside the `read` JSON-into-Value path.
    pub fn root(&self) -> &Path {
        &self.root
    }
}
