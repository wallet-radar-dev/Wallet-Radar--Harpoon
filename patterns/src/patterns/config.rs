//! ПАТЕРН config (план X.1): показати ЕФЕКТИВНУ конфігурацію рушія —
//! які пороги/ціни й з якого джерела породили вітрини (відтворюваність).
//! + демонструє межі впевненості (X.4) на прикладі USD-суми: `config usd=1000000`.
use crate::core::ctx::Ctx;
use crate::core::config::Config;
use crate::core::prices::Prices;
use crate::core::PResult;
use super::Pattern;

pub struct ShowConfig;

impl Pattern for ShowConfig {
    fn name(&self) -> &str { "config" }
    fn about(&self) -> &str { "ефективні пороги/ціни рушія + версія + межі впевненості (X.4)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let c = Config::load(&ctx.dir);
        println!("── КОНФІГ РУШІЯ ─────────────────────────────");
        println!("версія:        {}", c.version);
        println!("джерело:       {}", c.source);
        println!("exchange_tx:   nonce > {} = біржа/сервіс", c.exchange_tx);
        println!("hub_pct:       {} (перцентиль виключення хабів)", c.hub_pct);
        println!("min_cluster:   {} (мін. синхро-кластер)", c.min_cluster);
        println!("window_h:      {} год (вікно синхронності за замовч.)", c.window_h);
        println!("grab_emission: {} (частка холдерів = викуп емісії)", c.grab_emission);
        println!("price_eth_usd: ${}", c.price_eth_usd);
        println!("price_matic:   ${}", c.price_matic_usd);
        println!("невизначеність ціни: ±{}% (для меж впевненості X.4)", (c.price_uncertainty * 100.0) as i64);
        // Фаза 1.2: реальні ціни з артефакту (token-prices.json) з відкатом на словник
        let p = Prices::load(&ctx.dir);
        println!("── ЦІНИ (Фаза 1.2) ──────────────────────────");
        println!("джерело цін:   {}", p.source);
        println!("ETH:           ${:.2}", p.eth_usd);
        println!("токенів з реальною ціною: {} {}", p.priced_count(),
            if p.priced_count() == 0 { "(відкат на константи — запусти tokprices)" } else { "" });
        // Фаза 0.1: реєстр формальних визначень L2 (defs=1)
        if args.get("defs").map(|s| s == "1").unwrap_or(false) {
            use crate::core::dict::definitions;
            println!("── СЛОВНИК ВИЗНАЧЕНЬ (L2, Фаза 0.1) ─────────");
            for d in definitions() {
                println!("• {} = {}", d.name, d.formula);
                println!("    параметри: {} · v{} · від {}", d.params, d.version, d.since);
            }
        } else {
            println!("(дай defs=1 щоб побачити реєстр визначень L2)");
        }
        // демонстрація меж впевненості на USD-сумі
        if let Some(u) = args.get("usd").and_then(|s| s.parse::<f64>().ok()) {
            let (lo, hi) = c.interval(u);
            println!("── МЕЖІ ВПЕВНЕНОСТІ (X.4) ───────────────────");
            println!("оцінка ${:.0}  →  [{:.0} … {:.0}]  (±{}%)", u, lo, hi, (c.price_uncertainty * 100.0) as i64);
        } else {
            println!("(дай usd=<сума> щоб побачити інтервал впевненості)");
        }
        Ok(())
    }
}
