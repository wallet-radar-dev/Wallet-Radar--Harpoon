//! ПАТЕРН synchro: синхронний вхід у токен / викуп емісії.
//! Перше НАБУТТЯ токена кожним гаманцем -> ковзне вікно -> кластер, що зайшов разом.
//! grab = cluster/total_holders (~1.0 = група забрала всю емісію).
//! Аргументи: chains=1,42161,137  window_h=24  min_cluster=8
use std::collections::HashMap;
use crate::core::ctx::Ctx;
use crate::core::data::{stream_txfull, is_spam_symbol};
use crate::core::PResult;
use super::{Pattern, csv, num};

pub struct Synchro;

impl Pattern for Synchro {
    fn name(&self) -> &str { "synchro" }
    fn about(&self) -> &str { "синхронний вхід у токен / викуп емісії (координований кластер)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let chains = csv(args, "chains", "1");
        let win = (num(args, "window_h", 24.0) * 3600.0) as u64;
        let minc = num(args, "min_cluster", 8.0) as usize;

        // (token) -> {wallet: min_ts набуття}, + символ токена
        let mut acq: HashMap<String, HashMap<String, u64>> = HashMap::new();
        let mut sym_of: HashMap<String, String> = HashMap::new();
        for ch in &chains {
            stream_txfull(&ctx.txfull(ch), |tx| {
                if tx.k != "token" || tx.tok.is_empty() || tx.t != tx.w || tx.ts == 0 { return }
                let m = acq.entry(tx.tok.clone()).or_default();
                let e = m.entry(tx.w.clone()).or_insert(tx.ts);
                if tx.ts < *e { *e = tx.ts }
                if !tx.sym.is_empty() { sym_of.entry(tx.tok.clone()).or_insert(tx.sym.clone()); }
            });
        }
        eprintln!("[synchro] токенів із набуттями: {}", acq.len());

        // кластери
        let mut clusters: Vec<(usize, usize, f64, f64, String)> = Vec::new(); // cluster,total,grab,span_h,sym
        for (tok, wallets) in &acq {
            if wallets.len() < 10 { continue }
            let sym = sym_of.get(tok).cloned().unwrap_or_default();
            if is_spam_symbol(&sym) { continue }
            let mut ts: Vec<u64> = wallets.values().copied().collect();
            ts.sort_unstable();
            // найщільніше вікно
            let (mut bi, mut bj, mut j) = (0usize, 0usize, 0usize);
            for i in 0..ts.len() {
                if j < i { j = i }
                while j + 1 < ts.len() && ts[j + 1] - ts[i] <= win { j += 1 }
                if j - i > bj - bi { bi = i; bj = j }
            }
            let size = bj - bi + 1;
            if size >= minc {
                let span_h = (ts[bj] - ts[bi]) as f64 / 3600.0;
                let grab = size as f64 / wallets.len() as f64;
                clusters.push((size, wallets.len(), grab, span_h, sym));
            }
        }
        // сортуємо за викупом емісії: великий тісний кластер = майже всі холдери
        clusters.sort_by(|a, b| {
            let ka = if a.2 >= 0.7 { a.0 } else { 0 };
            let kb = if b.2 >= 0.7 { b.0 } else { 0 };
            kb.cmp(&ka).then(b.2.partial_cmp(&a.2).unwrap()).then(a.3.partial_cmp(&b.3).unwrap())
        });
        println!("[synchro] кластерів (>= {minc} у {}г): {}", win / 3600, clusters.len());
        println!("{:<12}{:>7}{:>8}{:>8}{:>9}", "токен", "синхро", "з_усіх", "викуп%", "вікно,г");
        for (size, total, grab, span, sym) in clusters.iter().take(25) {
            println!("  {:<10}{:>7}{:>8}{:>6}%{:>9.1}",
                sym.chars().take(9).collect::<String>(), size, total, (grab * 100.0) as i64, span);
        }
        Ok(())
    }
}
