//! ПАТЕРН snapshot (Фаза 1.3 плану): знімок стану координованих груп із ДАТОЮ -> історія.
//! Читає coord-<chain>.json (результат synchro) + рахує підсумок, дописує рядок у group-history.csv.
//! Так накопичується ДИНАМІКА груп у часі — те, чого немає в конкурентів і що найцінніше в продажу.
//! Cron-ready. Аргументи: chains=1,42161,137  ts=<unix, дефолт системний>
use std::collections::HashSet;
use std::io::Write;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::dict;
use crate::core::PResult;
use super::{Pattern, csv, num};

pub struct Snapshot;

impl Pattern for Snapshot {
    fn name(&self) -> &str { "snapshot" }
    fn about(&self) -> &str { "знімок стану координованих груп із датою -> історія (Фаза 1.3)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let chains = csv(args, "chains", "1,42161,137");
        let ts = num(args, "ts", 0.0) as u64;
        let ts = if ts > 0 { ts } else {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs()).unwrap_or(0)
        };
        let out = format!("{}/group-history.csv", ctx.dir);
        let new = !std::path::Path::new(&out).exists();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&out)
            .map_err(|e| e.to_string())?;
        if new {
            writeln!(f, "ts,chain,dict_ver,n_clusters,n_coord_wallets,n_emission_buyups,top_token,top_cluster")
                .map_err(|e| e.to_string())?;
        }
        for ch in &chains {
            let path = format!("{}/coord-{}.json", ctx.dir, ch);
            let Ok(s) = std::fs::read_to_string(&path) else {
                eprintln!("[snapshot] нема {path} — спершу synchro"); continue };
            let Ok(cl) = serde_json::from_str::<Value>(&s) else { continue };
            let arr = cl.as_array().cloned().unwrap_or_default();
            let n_clusters = arr.len();
            // унікальні координовані гаманці
            let mut wallets: HashSet<String> = HashSet::new();
            let mut buyups = 0usize;
            let mut top_token = String::new();
            let mut top_cluster = 0i64;
            for c in &arr {
                let size = c.get("cluster").and_then(Value::as_i64).unwrap_or(0);
                let grab = c.get("grab").and_then(Value::as_f64).unwrap_or(0.0);
                if grab >= dict::grab_emission() { buyups += 1; }
                if size > top_cluster {
                    top_cluster = size;
                    top_token = c.get("sym").and_then(Value::as_str).unwrap_or("").to_string();
                }
                if let Some(ws) = c.get("wallets").and_then(Value::as_array) {
                    for w in ws { if let Some(a) = w.as_str() { wallets.insert(a.to_lowercase()); } }
                }
            }
            writeln!(f, "{ts},{ch},{},{n_clusters},{},{buyups},{},{top_cluster}",
                dict::dict_version(), wallets.len(), top_token)
                .map_err(|e| e.to_string())?;
            println!("[snapshot] chain {ch}: {n_clusters} кластерів, {} координованих гам, {buyups} викупів емісії, топ {top_token}({top_cluster})",
                wallets.len());
        }
        // Фаза 0.3: журнал перерахунку (входи — coord-файли мереж)
        let ins: Vec<String> = chains.iter().map(|ch| format!("{}/coord-{}.json", ctx.dir, ch)).collect();
        crate::core::journal::record(&ctx.dir, "snapshot", &out, &ins.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        println!("=> історія: {out} (dict {})", dict::dict_version());
        Ok(())
    }
}
