//! ПАТЕРН dossier (Фаза 2.1 + X.4 плану): ЗВІТ-ЯК-ПОСЛУГА — разове розслідування ГРУПИ у файл.
//! «Є що продати завтра, без платформи.» Пакує в один клієнтський markdown усе ядро рушія:
//! реальний vs паперовий капітал, контроль емісії, і ризиковий двигун (вихід/стрес/декомпозиція),
//! скрізь із межами впевненості. Двомовно (uk/en — план: англ. основна для аудиторії).
//! Читає cap-real.json + orderbook.csv + tokenpnl/token-supply; НІЧОГО не мутує.
//! Аргументи: group=<id> lang=en|uk out=<файл> pct=30 days=7 shock=40
use std::collections::HashMap;
use std::io::Write;
use serde_json::Value;
use crate::core::ctx::Ctx;
use crate::core::PResult;
use super::{Pattern, num};

pub struct Dossier;

fn cex_bid(dir: &str) -> HashMap<String, f64> {
    let mut m: HashMap<String, (f64, u64)> = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(format!("{dir}/orderbook.csv")) {
        for line in s.lines().skip(1) {
            let c: Vec<&str> = line.split(',').collect();
            if c.len() < 5 { continue }
            let ts: u64 = c[0].parse().unwrap_or(0);
            let sym = c[1].trim_end_matches("USDT").trim_end_matches("USDC").trim_end_matches("USD").to_uppercase();
            let bid: f64 = c[4].parse().unwrap_or(0.0);
            let e = m.entry(sym).or_insert((0.0, 0));
            if ts >= e.1 { *e = (bid, ts); }
        }
    }
    m.into_iter().map(|(k, (b, _))| (k, b)).collect()
}

fn find(dir: &str, name: &str) -> Option<String> {
    for p in [format!("{dir}/{name}"), format!("{dir}/../{name}"),
              format!("/opt/wallet-tracker/dataset/backfill/{name}"), format!("/opt/wallet-tracker/dataset/{name}")] {
        if std::path::Path::new(&p).exists() { return Some(p) }
    }
    None
}

fn load_supply(dir: &str) -> HashMap<String, f64> {
    let mut supply = HashMap::new();
    if let Some(p) = find(dir, "tokenpnl.jsonl") {
        if let Ok(f) = std::fs::File::open(&p) {
            use std::io::BufRead;
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(d) = serde_json::from_str::<Value>(&line) {
                    let c = d["contract"].as_str().unwrap_or("").to_lowercase();
                    if let Some(s) = d["supply"].as_f64() { if s > 0.0 && !c.is_empty() { supply.entry(c).or_insert(s); } }
                }
            }
        }
    }
    if let Some(v) = find(dir, "token-supply.json").and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        if let Some(o) = v.as_object() {
            for (k, x) in o { if let Some(s) = x["supply"].as_f64() { if s > 0.0 { supply.entry(k.to_lowercase()).or_insert(s); } } }
        }
    }
    supply
}

// невеликий локалізатор
struct L { en: bool }
impl L { fn t<'a>(&self, uk: &'a str, en: &'a str) -> &'a str { if self.en { en } else { uk } } }

impl Pattern for Dossier {
    fn name(&self) -> &str { "dossier" }
    fn about(&self) -> &str { "звіт-як-послуга: клієнтське дос'є групи у файл (Фаза 2.1)" }

    fn run(&self, ctx: &Ctx, args: &std::collections::BTreeMap<String, String>) -> PResult {
        let l = L { en: args.get("lang").map(|s| s == "en").unwrap_or(false) };
        let pct = num(args, "pct", 30.0);
        let days = num(args, "days", 7.0).max(1.0);
        let shock = num(args, "shock", 40.0) / 100.0;

        let path = find(&ctx.dir, "cap-real.json").ok_or("нема cap-real.json (спершу capreal)")?;
        let cr: Value = serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let groups = cr["groups"].as_array().ok_or("нема groups")?;
        let bid = cex_bid(&ctx.dir);
        let supply = load_supply(&ctx.dir);

        let want = args.get("group").cloned().unwrap_or_else(||
            groups.iter().max_by(|a, b| a["realTotalUsd"].as_f64().unwrap_or(0.0)
                .partial_cmp(&b["realTotalUsd"].as_f64().unwrap_or(0.0)).unwrap())
                .and_then(|g| g["id"].as_str()).unwrap_or("?").to_string());
        let g = groups.iter().find(|g| g["id"].as_str() == Some(&want))
            .ok_or(format!("групу {want} не знайдено"))?;

        let out = args.get("out").cloned().unwrap_or_else(|| format!("/opt/wallet-tracker/reports/dossier-{}-{}.md", want, if l.en {"en"} else {"uk"}));
        let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;

        let gv = |k: &str| g[k].as_f64().unwrap_or(0.0);
        let (paper, native, realiz, real_total) = (gv("paperUsd"), gv("nativeEthUsd"), gv("realizableUsd"), gv("realTotalUsd"));
        let size = g["size"].as_u64().unwrap_or(0);
        let members = g["membersUsed"].as_u64().unwrap_or(0);

        // активи + похідні (реалізовне з біржею, % емісії, ризик)
        struct A { sym: String, paper: f64, reserve: f64, realiz_cex: f64, emis: Option<f64>, dd_daily: f64, score: f64 }
        let mut assets: Vec<A> = vec![];
        let (mut stuck, mut realiz_cex_tot) = (0.0f64, 0.0f64);
        if let Some(arr) = g["assets"].as_array() {
            for a in arr {
                let p = a["paperUsd"].as_f64().unwrap_or(0.0);
                if p < 1000.0 { continue }
                let reserve = a["reserveUsd"].as_f64().unwrap_or(0.0);
                let sym = a["symbol"].as_str().unwrap_or("").to_uppercase();
                let c = a["token"].as_str().unwrap_or("").to_lowercase();
                let amount = a["amount"].as_f64().unwrap_or(0.0);
                let price = if amount > 0.0 { p / amount } else { 0.0 };
                let cexb = if sym.is_empty() { 0.0 } else { bid.get(&sym).copied().unwrap_or(0.0) };
                let depth = wr_dict::dex_depth_at_ref(reserve) + cexb;
                let rc = wr_dict::realizable_with_cex(p, reserve, 0.10, cexb);
                realiz_cex_tot += rc;
                let mcap = supply.get(&c).copied().unwrap_or(0.0) * price;
                let emis = if mcap > 0.0 { Some((p / mcap).min(1.0)) } else { None };
                let dd = wr_dict::price_impact_pct(pct / 100.0 * p / days, depth);
                if dd.is_infinite() { stuck += pct / 100.0 * p; }
                let conc = emis.unwrap_or(1.0);
                let illiq = (1.0 - (depth / p).min(1.0)).max(0.0);
                assets.push(A { sym: if sym.is_empty() { c[..8.min(c.len())].to_string() } else { sym }, paper: p, reserve, realiz_cex: rc, emis, dd_daily: dd, score: 100.0 * conc * illiq });
            }
        }
        assets.sort_by(|a, b| b.paper.partial_cmp(&a.paper).unwrap());

        let real_cex = native + realiz_cex_tot; // реальний капітал з біржовою глибиною
        let d = |x: f64| format!("${:.0}", x);

        // ── ДОКУМЕНТ ──
        writeln!(f, "# {} {}\n", l.t("РОЗСЛІДУВАННЯ ГРУПИ КАПІТАЛУ", "CAPITAL GROUP INVESTIGATION"), want).map_err(e2s)?;
        writeln!(f, "*Wallet Radar · dict {} · {}*\n", wr_dict::dict_version(),
            l.t("детермінований аналіз ончейн-даних, кожен висновок відтворюваний",
                "deterministic on-chain analysis, every figure reproducible")).map_err(e2s)?;

        writeln!(f, "## {}\n", l.t("Стисло", "Executive summary")).map_err(e2s)?;
        writeln!(f, "- {}: **{}** {} ({} {})", l.t("Розмір групи", "Group size"), size, l.t("гаманців", "wallets"), members, l.t("з капіталом", "with capital")).map_err(e2s)?;
        writeln!(f, "- {}: **{}** ({} {} + {} {})", l.t("Реальний капітал", "Real capital"), d(real_cex),
            d(native), l.t("нативний ETH, глибокий", "native ETH, deep"), d(realiz_cex_tot), l.t("реалізовне в токенах", "realizable in tokens")).map_err(e2s)?;
        let tok_ratio = if paper > 0.0 { 100.0 * realiz_cex_tot / paper } else { 0.0 };
        writeln!(f, "- {}: **{}** {} **{}** (**{:.0}%**) — {}", l.t("Токенні позиції (папір)", "Token positions (paper)"), d(paper),
            l.t("→ реалізовно лише", "→ realizable only"), d(realiz_cex_tot), tok_ratio, l.t("решта неліквідна", "the rest is illiquid")).map_err(e2s)?;
        if stuck > 0.0 {
            writeln!(f, "- ⚠ {}: **{}** {} {:.0}%{}", l.t("Не вийти", "Cannot exit"), d(stuck),
                l.t("токенних позицій неможливо продати за поточної ліквідності (роздача", "of token positions cannot be sold at current liquidity ("), pct, l.t(")", " distribution)")).map_err(e2s)?;
        }
        let top_emis = assets.iter().filter_map(|a| a.emis.map(|e| (a.sym.clone(), e))).max_by(|x, y| x.1.partial_cmp(&y.1).unwrap());
        if let Some((s, e)) = &top_emis {
            writeln!(f, "- {}: **{}** ({:.0}% {})", l.t("Найбільший контроль емісії", "Largest emission control"), s, e * 100.0, l.t("емісії токена", "of token supply")).map_err(e2s)?;
        }

        // 1. Капітал
        writeln!(f, "\n## 1. {}\n", l.t("Капітал: папір проти реального", "Capital: paper vs real")).map_err(e2s)?;
        writeln!(f, "{}\n", l.t("Паперова оцінка (кількість × ціна) переоцінена: вийняти ці гроші нема куди, бо ліквідність тонка. Реальне = нативний ETH (глибокий) + min(папір, 10% пулів DEX + біржовий bid ±2%).",
            "Paper valuation (quantity × price) is overstated: the money cannot be extracted because liquidity is thin. Real = native ETH (deep) + min(paper, 10% of DEX pools + exchange bid ±2%).")).map_err(e2s)?;
        writeln!(f, "| {} | USD |\n|---|--:|", l.t("метрика", "metric")).map_err(e2s)?;
        writeln!(f, "| {} | {} |", l.t("Паперовий капітал", "Paper capital"), d(paper)).map_err(e2s)?;
        writeln!(f, "| {} | {} |", l.t("Нативний ETH", "Native ETH"), d(native)).map_err(e2s)?;
        writeln!(f, "| {} (DEX) | {} |", l.t("Реалізовне в токенах", "Realizable in tokens"), d(realiz)).map_err(e2s)?;
        writeln!(f, "| {} (DEX+{}) | {} |", l.t("Реалізовне в токенах", "Realizable in tokens"), l.t("біржа", "exchange"), d(realiz_cex_tot)).map_err(e2s)?;
        writeln!(f, "| **{}** | **{}** |", l.t("РЕАЛЬНИЙ ПІДСУМОК", "REAL TOTAL"), d(real_cex)).map_err(e2s)?;

        // 2. Холдинги + контроль емісії
        writeln!(f, "\n## 2. {}\n", l.t("Холдинги та контроль емісії", "Holdings & emission control")).map_err(e2s)?;
        writeln!(f, "| {} | {} | {} | {} | {} |\n|---|--:|--:|--:|--:|",
            l.t("токен", "token"), l.t("папір$", "paper$"), l.t("% емісії", "% supply"), l.t("реалізовне$", "realizable$"), l.t("DEX-резерв$", "DEX reserve$")).map_err(e2s)?;
        for a in assets.iter().take(15) {
            let em = a.emis.map(|e| format!("{:.1}%", e * 100.0)).unwrap_or_else(|| "?".into());
            writeln!(f, "| {} | {} | {} | {} | {} |", a.sym, d(a.paper), em, d(a.realiz_cex), d(a.reserve)).map_err(e2s)?;
        }

        // 3. Ризиковий двигун
        writeln!(f, "\n## 3. {}\n", l.t("Ризиковий аналіз", "Risk analysis")).map_err(e2s)?;
        writeln!(f, "### 3.1 {} ({}% / {} {})\n", l.t("Сценарій виходу", "Exit scenario"), pct, days, l.t("днів", "days")).map_err(e2s)?;
        let mut worst: Vec<&A> = assets.iter().filter(|a| a.dd_daily.is_finite() && a.dd_daily > 0.5).collect();
        worst.sort_by(|a, b| b.dd_daily.partial_cmp(&a.dd_daily).unwrap());
        writeln!(f, "{}\n", l.t("Якщо група роздає позиції — просадка/день (нижня межа, реальний стакан тоншає глибше):",
            "If the group distributes positions — drawdown/day (lower bound; real books thin out further):")).map_err(e2s)?;
        writeln!(f, "| {} | {} | {} |\n|---|--:|--:|", l.t("токен", "token"), l.t("продати$", "sell$"), l.t("просадка/день", "drawdown/day")).map_err(e2s)?;
        for a in worst.iter().take(8) {
            writeln!(f, "| {} | {} | {:.1}% … {:.1}% |", a.sym, d(pct / 100.0 * a.paper), a.dd_daily, a.dd_daily * 2.0).map_err(e2s)?;
        }
        writeln!(f, "\n### 3.2 {} (−{:.0}%)\n", l.t("Стрес ринку", "Market stress"), shock * 100.0).map_err(e2s)?;
        let exposed: f64 = assets.iter().filter(|a| a.score > 50.0 || a.dd_daily.is_infinite()).map(|a| a.paper * (1.0 - shock)).sum();
        writeln!(f, "{} **{}** {} {:.0}% {}.",
            l.t("За шоку ринку", "Under a market shock"), d(exposed), l.t("капіталу групи —", "of group capital is in"), if real_total > 0.0 { 100.0 * exposed / (paper * (1.0 - shock)).max(1.0) } else { 0.0 },
            l.t("у каскадних/неліквідних активах (падають сильніше за ринок)", "cascading/illiquid assets (fall harder than the market)")).map_err(e2s)?;

        writeln!(f, "\n### 3.3 {}\n", l.t("Найризикованіші активи (RiskScore)", "Highest-risk assets (RiskScore)")).map_err(e2s)?;
        let mut byrisk: Vec<&A> = assets.iter().collect();
        byrisk.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        writeln!(f, "| {} | RiskScore | {} |\n|---|--:|---|", l.t("токен", "token"), l.t("чому", "why")).map_err(e2s)?;
        for a in byrisk.iter().take(6) {
            let why = if a.emis.map(|e| e > 0.5).unwrap_or(false) { l.t("висока концентрація + тонка ліквідність", "high concentration + thin liquidity") }
                      else if a.emis.is_none() { l.t("концентрація невідома, тонка ліквідність", "concentration unknown, thin liquidity") }
                      else { l.t("переважно тонка ліквідність", "mostly thin liquidity") };
            writeln!(f, "| {} | {:.0} | {} |", a.sym, a.score, why).map_err(e2s)?;
        }

        // 4. Метод
        writeln!(f, "\n## 4. {}\n", l.t("Метод і межі впевненості", "Method & confidence")).map_err(e2s)?;
        writeln!(f, "- {}", l.t("Кожна цифра детермінована й відтворювана з ончейн-даних (єдиний словник визначень, версія у шапці).",
            "Every figure is deterministic and reproducible from on-chain data (single data dictionary, version in header).")).map_err(e2s)?;
        writeln!(f, "- {}", l.t("Просадки — ЛІНІЙНА нижня межа: реальний стакан тоншає глибше ±2%, тож справжня просадка ≥ вказаної. Діапазон [оцінка … 2×].",
            "Drawdowns are a LINEAR lower bound: real books thin beyond ±2%, so true drawdown ≥ shown. Range [estimate … 2×].")).map_err(e2s)?;
        writeln!(f, "- {}", l.t("«% емісії» рахується лише де є supply (DefiLlama/toksupply); інакше «?».",
            "\"% supply\" is computed only where supply is known (DefiLlama/toksupply); otherwise \"?\".")).map_err(e2s)?;
        writeln!(f, "- {}", l.t("Ліквідність: DEX-пули (GeckoTerminal) + глибина стаканів ±2% (Binance).",
            "Liquidity: DEX pools (GeckoTerminal) + order-book depth ±2% (Binance).")).map_err(e2s)?;

        // Фаза 0.3: журнал перерахунку вітрини (коли/яким кодом/на яких даних)
        crate::core::journal::record(&ctx.dir, "dossier", &out,
            &[&path, &format!("{}/orderbook.csv", ctx.dir)]);
        println!("[dossier] {} {} -> {out} ({} {}, {})",
            l.t("звіт групи", "group report"), want, assets.len(), l.t("активів", "assets"), if l.en {"en"} else {"uk"});
        Ok(())
    }
}

fn e2s<E: std::fmt::Display>(e: E) -> String { e.to_string() }
