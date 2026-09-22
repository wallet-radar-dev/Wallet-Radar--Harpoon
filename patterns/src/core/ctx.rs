//! Контекст: шляхи до даних, ключі Etherscan, ліниве завантаження group-membership.
use std::collections::{BTreeMap, HashMap};
use std::cell::RefCell;

pub struct Ctx {
    /// Тека з даними laptop-краулу (txfull-*.jsonl тощо).
    pub dir: String,
    /// Ключі Etherscan (для fetch-патернів).
    pub keys: Vec<String>,
    gm: RefCell<Option<HashMap<String, String>>>,
    gm_path: String,
}

impl Ctx {
    pub fn new(args: &BTreeMap<String, String>) -> Ctx {
        let dir = args.get("dir").cloned().unwrap_or_else(||
            "/opt/wallet-tracker/dataset/backfill/local-from-laptop".to_string());
        let keys = std::fs::read_to_string(format!("{dir}/keys.txt"))
            .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
            .unwrap_or_default();
        let gm_path = args.get("groups").cloned().unwrap_or_else(||
            "/opt/wallet-tracker/dataset/backfill/group-membership.json".to_string());
        Ctx { dir, keys, gm: RefCell::new(None), gm_path }
    }

    /// txfull-файл для мережі за chainid.
    pub fn txfull(&self, chain: &str) -> String {
        format!("{}/txfull-{}.jsonl", self.dir, chain)
    }

    /// group-membership (адреса->група) паралельної сесії, лінива загрузка.
    pub fn group_of(&self, addr: &str) -> Option<String> {
        if self.gm.borrow().is_none() {
            let mut m = HashMap::new();
            if let Ok(s) = std::fs::read_to_string(&self.gm_path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(w) = v.get("wallets").and_then(|w| w.as_object()) {
                        for (k, val) in w {
                            let g = val.get("group").and_then(|g| g.as_str())
                                .or_else(|| val.as_str()).unwrap_or("?");
                            m.insert(k.to_lowercase(), g.to_string());
                        }
                    }
                }
            }
            *self.gm.borrow_mut() = Some(m);
        }
        self.gm.borrow().as_ref().unwrap().get(&addr.to_lowercase()).cloned()
    }

    /// Чи адреса поза мережею паралельної (нова/чужа).
    pub fn is_outsider(&self, addr: &str) -> bool {
        match self.group_of(addr).as_deref() {
            None | Some("NODATA") => true,
            _ => false,
        }
    }
}
