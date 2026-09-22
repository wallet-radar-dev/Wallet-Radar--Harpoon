//! Etherscan V2 з ротацією ключів і обробкою ліміту в ТІЛІ (NOTOK/rate limit — не порожньо).
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use serde_json::Value;

pub struct Es<'a> {
    keys: &'a [String],
    i: Cell<usize>,
    tx_cache: RefCell<HashMap<(u64, String), i64>>,
    code_cache: RefCell<HashMap<(u64, String), bool>>,
}

impl<'a> Es<'a> {
    pub fn new(keys: &'a [String]) -> Es<'a> {
        Es { keys, i: Cell::new(0),
             tx_cache: RefCell::new(HashMap::new()), code_cache: RefCell::new(HashMap::new()) }
    }
    fn key(&self) -> &str {
        if self.keys.is_empty() { return "" }
        let k = &self.keys[self.i.get() % self.keys.len()];
        self.i.set(self.i.get() + 1);
        k
    }

    /// GET з ретраями; повертає result як Value (None при стійкому збої/ліміті).
    pub fn call(&self, chain: u64, module_action: &str, extra: &str) -> Option<Value> {
        for _ in 0..7 {
            let url = format!(
                "https://api.etherscan.io/v2/api?chainid={chain}&{module_action}{extra}&apikey={}",
                self.key());
            if let Ok(resp) = ureq::get(&url).timeout(std::time::Duration::from_secs(30)).call() {
                if let Ok(v) = resp.into_json::<Value>() {
                    let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    let res_s = v.get("result").and_then(|r| r.as_str()).unwrap_or("");
                    let limited = msg == "NOTOK"
                        || res_s.contains("rate limit") || res_s.contains("Max")
                        || res_s.contains("Too many");
                    if !limited { return v.get("result").cloned() }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        None
    }

    /// Кількість вихідних tx (nonce) — дешева класифікація біржа/приватний. Кешується.
    pub fn tx_count(&self, chain: u64, addr: &str) -> i64 {
        let k = (chain, addr.to_lowercase());
        if let Some(&v) = self.tx_cache.borrow().get(&k) { return v }
        let r = self.call(chain, "module=proxy&action=eth_getTransactionCount",
                          &format!("&address={addr}&tag=latest"));
        let n = r.and_then(|v| v.as_str().map(|s| s.to_string()))
            .and_then(|s| i64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(-1);
        self.tx_cache.borrow_mut().insert(k, n);
        n
    }

    /// Баланс native у ETH (f64).
    pub fn balance_eth(&self, chain: u64, addr: &str) -> f64 {
        let r = self.call(chain, "module=account&action=balance",
                          &format!("&address={addr}&tag=latest"));
        r.and_then(|v| v.as_str().map(|s| s.to_string()))
            .and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 1e18
    }

    /// Чи адреса — контракт (має код). Кешується.
    pub fn is_contract(&self, chain: u64, addr: &str) -> bool {
        let k = (chain, addr.to_lowercase());
        if let Some(&v) = self.code_cache.borrow().get(&k) { return v }
        let c = self.call(chain, "module=proxy&action=eth_getCode", &format!("&address={addr}&tag=latest"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .map(|s| s != "0x" && s != "0x0" && !s.is_empty()).unwrap_or(false);
        self.code_cache.borrow_mut().insert(k, c);
        c
    }

    /// Вихідні native-перекази (to, ETH) — вибірка останніх offset по txlist+internal.
    pub fn outbound_native(&self, chain: u64, addr: &str, offset: u32) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        for act in ["txlist", "txlistinternal"] {
            let r = self.call(chain, &format!("module=account&action={act}"),
                &format!("&address={addr}&startblock=0&endblock=99999999&page=1&offset={offset}&sort=desc"));
            if let Some(arr) = r.as_ref().and_then(Value::as_array) {
                for t in arr {
                    let fr = t.get("from").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                    let to = t.get("to").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                    let v = t.get("value").and_then(|x| x.as_str()).unwrap_or("0");
                    if fr == addr.to_lowercase() && !to.is_empty() && to != addr.to_lowercase() {
                        let eth = v.parse::<f64>().unwrap_or(0.0) / 1e18;
                        if eth > 0.0 { out.push((to, eth)); }
                    }
                }
            }
        }
        out
    }

    /// Номер блока найближче ДО заданого часу (unix).
    pub fn block_by_time(&self, chain: u64, ts: u64) -> Option<u64> {
        self.call(chain, "module=block&action=getblocknobytime",
                  &format!("&timestamp={ts}&closest=before"))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .and_then(|s| s.parse::<u64>().ok())
    }

    /// Native-баланс (ETH) на конкретному блоці (історичний зріз, потребує archive у Etherscan).
    pub fn balance_at(&self, chain: u64, addr: &str, block: u64) -> f64 {
        let r = self.call(chain, "module=proxy&action=eth_getBalance",
                          &format!("&address={addr}&tag=0x{block:x}"));
        r.and_then(|v| v.as_str().map(|s| s.to_string()))
            .and_then(|s| i128::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .map(|w| w as f64 / 1e18).unwrap_or(0.0)
    }

    /// Час першої транзакції адреси (unix).
    pub fn first_tx_ts(&self, chain: u64, addr: &str) -> Option<u64> {
        let r = self.call(chain, "module=account&action=txlist",
            &format!("&address={addr}&startblock=0&endblock=99999999&page=1&offset=1&sort=asc"));
        r.as_ref().and_then(|v| v.as_array()).and_then(|a| a.first())
            .and_then(|t| t.get("timeStamp")).and_then(|x| x.as_str())
            .and_then(|s| s.parse().ok())
    }

    /// Перший вхідний фандер (найраніший переказ value>0 на адресу).
    pub fn first_funder(&self, chain: u64, addr: &str) -> Option<String> {
        for act in ["txlist", "txlistinternal"] {
            let r = self.call(chain, &format!("module=account&action={act}"),
                &format!("&address={addr}&startblock=0&endblock=99999999&page=1&offset=30&sort=asc"));
            if let Some(arr) = r.as_ref().and_then(|v| v.as_array()) {
                for t in arr {
                    let to = t.get("to").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                    let fr = t.get("from").and_then(|x| x.as_str()).unwrap_or("").to_lowercase();
                    let val = t.get("value").and_then(|x| x.as_str()).unwrap_or("0");
                    if to == addr.to_lowercase() && !fr.is_empty() && fr != addr.to_lowercase()
                        && val != "0" && !val.is_empty() {
                        return Some(fr);
                    }
                }
            }
        }
        None
    }
}
