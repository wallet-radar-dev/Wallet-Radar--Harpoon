//! wr-import — переїзд довідкових даних із коду у SQLite.
//!
//!   wr-import extract   .rs → wr-store/seed/*.json   (те, що можна переглянути очима)
//!   wr-import load      seed + dataset → wallet-radar.db
//!   wr-import verify    звірка в ОБИДВА боки + перелік того, що ще лишилось у коді
//!
//! Головний принцип: жодна адреса не переписується руками. Помилка транскрипції
//! в шістнадцятковому рядку тиха — система просто перестане бачити гаманець,
//! і ніхто цього не помітить. Тому все тягнеться регулярками з джерел, а кількість
//! звіряється з розміром масиву, який оголошено в самому Rust (`; 46]`).
//! Не зійшлось — падаємо, а не імпортуємо «скільки вийшло».

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    if let Ok(p) = std::env::var("WR_REPO") {
        return PathBuf::from(p);
    }
    // біна лежить у wr-store/target/...; корінь репозиторію — два рівні вище крейта
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if here.join("wr-store").is_dir() {
        return here;
    }
    here.parent().map(|p| p.to_path_buf()).unwrap_or(here)
}

/// Куди кладуться видобуті таблиці. `WR_SEED` навмисно винесений назовні:
/// ці файли містять ТІ САМІ адреси, що ми прибираємо з коду, тож найкраще
/// їм узагалі не з'являтися в теці репозиторію — не покладатись на .gitignore.
fn seed_dir() -> PathBuf {
    if let Ok(p) = std::env::var("WR_SEED") {
        return PathBuf::from(p);
    }
    repo_root().join("wr-store").join("seed")
}

fn read(root: &Path, rel: &str) -> Result<String> {
    let p = root.join(rel);
    fs::read_to_string(&p).with_context(|| format!("не читається {}", p.display()))
}

// ─────────────────────────── видобування ───────────────────────────

/// Джерела для видобування, у порядку довіри: робоча копія, потім git HEAD.
///
/// Потрібно тому, що міграція ЗАБИРАЄ таблиці з коду. Після етапу 3 у робочій
/// копії `wr-dict/src/lib.rs` уже немає ні порогів, ні real_money — але
/// `extract` мусить лишатись відтворюваним, інакше імпорт можна виконати
/// рівно один раз у житті, а потім лише сподіватись, що seed не загубився.
fn sources(root: &Path, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(s) = read(root, rel) {
        out.push(s);
    }
    if let Ok(o) = std::process::Command::new("git")
        .current_dir(root)
        .args(["show", &format!("HEAD:{rel}")])
        .output()
    {
        if o.status.success() {
            out.push(String::from_utf8_lossy(&o.stdout).into_owned());
        }
    }
    // Архівні дерева — теж джерело, і не запасне: git у цьому репозиторії неповний
    // (частину файлів ніколи не комітили). Кожен екстрактор мусить бачити всі три,
    // інакше після чистки коду він мовчки перестає знаходити свою таблицю.
    for a in std::env::var("WR_ARCHIVE_ROOTS").unwrap_or_default().split([';', ',']) {
        let a = a.trim();
        if a.is_empty() {
            continue;
        }
        if let Ok(s) = read(Path::new(a), rel) {
            out.push(s);
        }
    }
    out
}

/// Останній рубіж: уже видобутий seed. Тримає імпорт ідемпотентним навіть тоді,
/// коли ні робоча копія, ні git більше не містять таблиці.
fn from_seed(name: &str, want: Option<usize>) -> Option<Vec<Value>> {
    let v = seed(name).ok()?;
    let rows = arr(&v).clone();
    match want {
        Some(n) if rows.len() != n => None,
        _ if rows.is_empty() => None,
        _ => Some(rows),
    }
}


/// Розмір масиву, оголошений у Rust: `const ROWS: [(&str, &str, &str); 46] = [`.
/// Це наш незалежний свідок — якщо регулярка знайде інше число, ми це побачимо.
fn declared_len(src: &str, name: &str) -> Result<usize> {
    let re = Regex::new(&format!(r"const\s+{name}\s*:\s*\[[^\]]*;\s*(\d+)\s*\]"))?;
    re.captures(src)
        .and_then(|c| c.get(1))
        .ok_or_else(|| anyhow!("у джерелі немає оголошення довжини для {name}"))?
        .as_str()
        .parse::<usize>()
        .map_err(Into::into)
}

/// entities.rs — 46 перевірених біржових/інституційних гаманців.
fn extract_entities(root: &Path) -> Result<Vec<Value>> {
    let re = Regex::new(r#"\(\s*"(0x[0-9a-fA-F]{40})"\s*,\s*"([^"]*)"\s*,\s*"([^"]*)"\s*\)"#)?;
    // Той самий триступеневий пошук, що й у решти: робоча копія → git → seed.
    // Етап 4 ЗАБРАВ цю таблицю з entities.rs, тож читання лише робочої копії
    // зламало б `extract` назавжди.
    for src in sources(root, "rust-server/src/entities.rs") {
        let Ok(want) = declared_len(&src, "ROWS") else { continue };
        let rows: Vec<Value> = re
            .captures_iter(&src)
            .map(|c| {
                json!({
                    "address": c[1].to_lowercase(),
                    "label":   c[2].to_string(),
                    "kind":    c[3].to_string(),
                })
            })
            .collect();
        if rows.len() != want {
            bail!("entities.rs: оголошено {want} рядків, видобуто {} — розбіжність", rows.len());
        }
        return Ok(rows);
    }
    if let Some(rows) = from_seed("entities.json", None) {
        return Ok(rows);
    }
    bail!("entities: таблиці немає ні в робочій копії, ні в git, ні в seed")
}

/// wr-dict::real_money() — «справжні гроші» для P&L. Мережа береться з коментарів-секцій.
fn extract_real_money(root: &Path) -> Result<Vec<Value>> {
    let arm = Regex::new(
        r#""(0x[0-9a-fA-F]{40})"\s*=>\s*\(Money::(\w+)\s*,\s*(\d+)\)\s*,?\s*(?://\s*(.*))?"#,
    )?;
    let parse = |src: &str| -> Vec<Value> {
        let Some(body) = src.split("pub fn real_money").nth(1) else { return Vec::new() };
        let body = body.split("\n}").next().unwrap_or(body);
        let mut chain = "eth".to_string();
        let mut out = Vec::new();
        for line in body.lines() {
            let t = line.trim();
            // секційний коментар перемикає мережу: `// arbitrum`
            if let Some(rest) = t.strip_prefix("//") {
                let w = rest.trim();
                if matches!(w, "eth" | "arbitrum" | "polygon" | "base" | "optimism") {
                    chain = w.to_string();
                    continue;
                }
            }
            if let Some(c) = arm.captures(t) {
                out.push(json!({
                    "chain":      chain,
                    "contract":   c[1].to_lowercase(),
                    "money_kind": c[2].to_lowercase(),
                    "decimals":   c[3].parse::<i64>().unwrap_or_default(),
                    "symbol":     c.get(4).map(|m| m.as_str().trim().to_string())
                                    .unwrap_or_else(|| "?".into()),
                }));
            }
        }
        out
    };
    for src in sources(root, "wr-dict/src/lib.rs") {
        let rows = parse(&src);
        if !rows.is_empty() {
            return Ok(rows);
        }
    }
    if let Some(rows) = from_seed("real-money.json", None) {
        return Ok(rows);
    }
    bail!("real_money: таблиці немає ні в робочій копії, ні в git, ні в seed")
}

/// wr-dict `pub const …` — пороги рушія.
fn extract_consts(root: &Path) -> Result<Vec<Value>> {
    // Тип мусить ловити і `f64`, і `&str`, і `&'static str`. Перша версія брала лише
    // `\w+` і мовчки губила DICT_VERSION — саме тому нижче стоїть звірка з незалежним
    // підрахунком `^pub const`, а не довіра до самої регулярки.
    let re = Regex::new(
        r"(?m)^pub const\s+(\w+)\s*:\s*(&(?:'static\s+)?\w+|\w+)\s*=\s*([^;]+);\s*(?://\s*(.*))?$",
    )?;
    for src in sources(root, "wr-dict/src/lib.rs") {
        let declared = src.lines().filter(|l| l.starts_with("pub const ")).count();
        if declared == 0 {
            continue; // у цьому джерелі констант уже немає — пробуємо наступне
        }
        let mut out = Vec::new();
        for c in re.captures_iter(&src) {
            let name = c[1].to_string();
            let ty = c[2].to_string();
            let raw = c[3].trim().trim_matches('"').replace('_', "");
            let kind = match ty.as_str() {
                "f64" | "f32" => "float",
                "u8" | "u32" | "u64" | "usize" | "i64" => "int",
                _ => "text",
            };
            out.push(json!({
                "key": name,
                "value": raw,
                "kind": kind,
                "description": c.get(4).map(|m| m.as_str().trim().to_string())
                                .unwrap_or_else(|| format!("перенесено з wr-dict ({ty})")),
            }));
        }
        if out.len() != declared {
            let got: BTreeSet<&str> = out.iter().filter_map(|v| v["key"].as_str()).collect();
            let lost: Vec<&str> = src
                .lines()
                .filter_map(|l| l.strip_prefix("pub const "))
                .filter_map(|l| l.split(':').next())
                .map(str::trim)
                .filter(|k| !got.contains(k))
                .collect();
            bail!(
                "wr-dict: у джерелі {declared} `pub const`, видобуто {} — загублено: {:?}",
                out.len(),
                lost
            );
        }
        return Ok(out);
    }
    if let Some(rows) = from_seed("constants.json", None) {
        return Ok(rows);
    }
    bail!("константи: немає ні в робочій копії, ні в git, ні в seed")
}

/// universe.rs — BASE_JSON (30 контрактів) і CG_IDS.
fn extract_universe(root: &Path) -> Result<(Vec<Value>, Vec<String>)> {
    let cg_re = Regex::new(r#"const CG_IDS\s*:\s*&str\s*=\s*"([^"]*)""#)?;
    for src in sources(root, "rust-server/src/universe.rs") {
        let Some(start) = src.find("const BASE_JSON") else { continue };
        let raw = &src[start..];
        let Some(o) = raw.find("r#\"") else { continue };
        let open = o + 3;
        let Some(close) = raw[open..].find("\"#") else { continue };
        let base: Vec<Value> = serde_json::from_str(&raw[open..open + close])
            .context("universe.rs: BASE_JSON не парситься як JSON")?;
        let cg = cg_re
            .captures(&src)
            .map(|c| c[1].split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        return Ok((base, cg));
    }
    // після чистки таблиця живе лише в базі/seed
    if let Some(rows) = from_seed("universe.json", None) {
        return Ok((rows, Vec::new()));
    }
    if let Ok(v) = seed("universe.json") {
        let base = v.get("base").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let cg = v
            .get("cg_ids")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if !base.is_empty() {
            return Ok((base, cg));
        }
    }
    bail!("universe: BASE_JSON немає ні в робочій копії, ні в git, ні в архіві, ні в seed")
}

/// cexsignal.rs — інфраструктура мережі під наглядом і пара, що утворює її центр.
fn extract_watchlist(root: &Path) -> Result<(Vec<Value>, Vec<String>)> {
    let pair = Regex::new(r#"\(\s*"(0x[0-9a-fA-F]{40})"\s*,\s*"([^"]*)"\s*\)"#)?;
    let one = Regex::new(r#""(0x[0-9a-fA-F]{40})""#)?;
    for src in sources(root, "backfill/src/cexsignal.rs") {
        let Ok(want) = declared_len(&src, "INFRA") else { continue };
        let Some(ib) = src.split("const INFRA").nth(1).and_then(|s| s.split("];").next()) else {
            continue;
        };
        let infra: Vec<Value> = pair
            .captures_iter(ib)
            .map(|c| json!({ "address": c[1].to_lowercase(), "label": c[2].to_string() }))
            .collect();
        if infra.len() != want {
            bail!("cexsignal.rs INFRA: оголошено {want}, видобуто {}", infra.len());
        }
        let tw = declared_len(&src, "TREASURY")?;
        let tb = src
            .split("const TREASURY")
            .nth(1)
            .and_then(|s| s.split("];").next())
            .ok_or_else(|| anyhow!("cexsignal.rs: TREASURY не знайдено"))?;
        let treasury: Vec<String> =
            one.captures_iter(tb).map(|c| c[1].to_lowercase()).collect();
        if treasury.len() != tw {
            bail!("cexsignal.rs TREASURY: оголошено {tw}, видобуто {}", treasury.len());
        }
        return Ok((infra, treasury));
    }
    // після чистки — з наявного seed
    if let Ok(v) = seed("watchlist.json") {
        let infra = v.get("infra").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let treasury: Vec<String> = v
            .get("treasury")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if !infra.is_empty() {
            return Ok((infra, treasury));
        }
    }
    bail!("watchlist: немає ні в робочій копії, ні в git, ні в архіві, ні в seed")
}
/// wr-dict::known_exchange() — 7 біржових адрес, яких НЕ було в entities.rs.
///
/// Мало не загубились: етап 3 прибрав цю функцію з коду, і лише звірка показала,
/// що три адреси (Binance 0xd551…, Poloniex, Bitfinex) не мали другого джерела.
/// Тому екстрактор читає її і з робочої копії, і — коли її там уже немає —
/// з git. Дані лишаються відтворюваними, а адреси не повертаються в .rs.
fn extract_known_exchange(root: &Path) -> Result<Vec<Value>> {
    const WANT: usize = 7;
    let arm = Regex::new(r#""(0x[0-9a-fA-F]{40})"\s*=>\s*"([^"]+)""#)?;

    let parse = |src: &str| -> Vec<Value> {
        let Some(body) = src.split("pub fn known_exchange").nth(1) else { return Vec::new() };
        let body = body.split("\n}").next().unwrap_or(body);
        arm.captures_iter(body)
            .map(|c| json!({ "address": c[1].to_lowercase(), "label": c[2].to_string() }))
            .collect()
    };

    for src in sources(root, "wr-dict/src/lib.rs") {
        let rows = parse(&src);
        if rows.len() == WANT {
            return Ok(rows);
        }
    }
    if let Some(rows) = from_seed("known-exchange.json", Some(WANT)) {
        return Ok(rows);
    }
    bail!(
        "known_exchange: не знайдено {WANT} адрес ні в робочій копії, ні в git, ні в seed. \
         Ці адреси не мають іншого джерела — імпортувати неповний перелік не можна."
    )
}


/// Усі .rs крейта — і з робочої копії, і з git HEAD, без тестів і тек `_backup*`.
///
/// Git-частина ОБОВ'ЯЗКОВА, і це коштувало втрати даних. Перша версія читала лише
/// робочу копію. Щойно етап 4 прибрав `const STABLES` із dexswap.rs, повторний
/// `extract` зі знищеним seed недорахував **8 стейблів** (BUSD, FRAX, LUSD, PYUSD,
/// TUSD, USDP, USDS, crvUSD) — і жодної помилки при цьому не було: імпорт просто
/// «знайшов менше». Саме тому кожен екстрактор мусить бачити й історію.
///
/// Екстрактори дедуплікують за адресою, тож додавання старих версій нічого
/// не псує — лише повертає те, що код уже забув.
fn rs_files(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();

    // git HEAD — спершу, щоб робоча копія (свіжіша) перекривала за потреби
    if let Ok(o) = std::process::Command::new("git")
        .current_dir(root)
        .args(["ls-tree", "-r", "HEAD", "--name-only"])
        .output()
    {
        if o.status.success() {
            for path in String::from_utf8_lossy(&o.stdout).lines() {
                if !path.ends_with(".rs") || path.contains("_backup") {
                    continue;
                }
                if let Ok(f) = std::process::Command::new("git")
                    .current_dir(root)
                    .args(["show", &format!("HEAD:{path}")])
                    .output()
                {
                    if f.status.success() {
                        let t = String::from_utf8_lossy(&f.stdout);
                        out.push((format!("git:{path}"), strip_tests(&t)));
                    }
                }
            }
        }
    }

    // Архівні дерева (`WR_ARCHIVE_ROOTS`) — головне джерело історії в ЦЬОМУ репозиторії.
    //
    // Git тут ненадійний: чимало файлів ніколи не комітили. `backfill/src/dexswap.rs`
    // узагалі відсутній у HEAD — і саме тому git-фолбек не врятував 8 стейблів,
    // коли етап 4 прибрав з нього таблицю. Прод-бекап на зовнішньому диску знає
    // більше за git, тож він рівноправне джерело, а не запасний варіант.
    let archives: Vec<PathBuf> = std::env::var("WR_ARCHIVE_ROOTS")
        .unwrap_or_default()
        .split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();

    let mut roots: Vec<PathBuf> = vec![root.to_path_buf()];
    roots.extend(archives);

    for base_root in roots {
    for crate_dir in ["rust-server", "backfill", "patterns", "nextaction", "wr-dict", "features"] {
        let mut stack = vec![base_root.join(crate_dir).join("src")];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("_backup")) {
                        stack.push(p);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") {
                    if let Ok(t) = fs::read_to_string(&p) {
                        let rel = p.display().to_string();
                        out.push((rel, strip_tests(&t)));
                    }
                }
            }
        }
    }
    }
    out
}

/// `const NAME: &str = "0x…40hex";` — адреси-константи, розсипані по крейтах.
///
/// Сюди потрапляє й нульова адреса, яку інакше довелось би набрати в цьому файлі
/// руками — тобто просто переселити з одного .rs в інший. Ключ = ім'я константи,
/// тож `ZERO` з backtest.rs і `WETH` з dexswap.rs стають іменованими записами.
fn extract_addr_consts(root: &Path) -> Result<Vec<Value>> {
    let re = Regex::new(r#"(?m)^\s*(?:pub\s+)?const\s+(\w+)\s*:\s*&(?:'static\s+)?str\s*=\s*"(0x[0-9a-fA-F]{40})"\s*;"#)?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for (rel, src) in rs_files(root) {
        for c in re.captures_iter(&src) {
            let key = format!("ADDR_{}", &c[1]);
            if !seen.insert(key.clone()) {
                continue;
            }
            out.push(json!({
                "key": key,
                "value": c[2].to_lowercase(),
                "kind": "hex",
                "description": format!("адреса-константа, перенесена з {rel}"),
            }));
        }
    }
    Ok(out)
}

/// Таблиці виду `"0x…" => ("Мітка", "вид"),` — concentration.rs та подібні.
fn extract_label_maps(root: &Path) -> Result<Vec<Value>> {
    let re = Regex::new(
        r#""(0x[0-9a-fA-F]{40})"\s*=>\s*\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\)"#,
    )?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for (rel, src) in rs_files(root) {
        for c in re.captures_iter(&src) {
            let addr = c[1].to_lowercase();
            if !seen.insert(addr.clone()) {
                continue;
            }
            out.push(json!({
                "address": addr,
                "label": c[2].to_string(),
                "kind": c[3].to_string(),
                "source": rel,
            }));
        }
    }
    Ok(out)
}

/// Пари `("0x…", "ПІДПИС")` у масивах — dexswap STABLES, known.rs ARTIFACT_ADDRS,
/// selltape-таблиці тощо. Підпис може бути символом токена або причиною-нотаткою.
fn extract_labelled_pairs(root: &Path) -> Result<Vec<Value>> {
    let re = Regex::new(r#"\(\s*"(0x[0-9a-fA-F]{40})"\s*,\s*"([^"]{1,200})"\s*\)"#)?;
    let sym = Regex::new(r#"\(\s*"([A-Za-z0-9._-]{1,12})"\s*,\s*"(0x[0-9a-fA-F]{40})"\s*\)"#)?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for (rel, src) in rs_files(root) {
        // (адреса, підпис)
        for c in re.captures_iter(&src) {
            let addr = c[1].to_lowercase();
            if !seen.insert(addr.clone()) {
                continue;
            }
            out.push(json!({ "address": addr, "label": c[2].to_string(), "source": rel }));
        }
        // (СИМВОЛ, адреса) — зворотний порядок, як у selltape
        for c in sym.captures_iter(&src) {
            let addr = c[2].to_lowercase();
            if !seen.insert(addr.clone()) {
                continue;
            }
            out.push(json!({ "address": addr, "label": c[1].to_string(), "source": rel }));
        }
    }
    Ok(out)
}

/// Списки стейблів: `const STABLES: &[&str] = &[ "0x…", … ];`
/// Секційні коментарі (`// USDC`) дають символ.
fn extract_stable_lists(root: &Path) -> Result<Vec<Value>> {
    let addr = Regex::new(r#""(0x[0-9a-fA-F]{40})""#)?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for (rel, src) in rs_files(root) {
        let Some(body) = src.split("const STABLES").nth(1) else { continue };
        let Some(body) = body.split("];").next() else { continue };
        let mut sym = String::from("STABLE");
        for line in body.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("//") {
                let w = rest.trim();
                if !w.is_empty() && w.len() <= 24 {
                    sym = w.to_string();
                }
                continue;
            }
            for c in addr.captures_iter(t) {
                let a = c[1].to_lowercase();
                if seen.insert(a.clone()) {
                    out.push(json!({ "contract": a, "symbol": sym, "source": rel }));
                }
            }
        }
    }
    Ok(out)
}


/// `const NAME: [&str; N] = ["0x…", …];` — іменовані переліки адрес.
///
/// Це остання форма, у якій адреси сиділи в коді: не таблиця з мітками, а просто
/// множина з іменем (KNOWN_CEX, PARIBU, ETH_TOKENS, DISPENSERS…). Ім'я масиву —
/// це і є тег; довжина, оголошена в типі, знову служить незалежним свідком.
fn extract_addr_arrays(root: &Path) -> Result<Vec<Value>> {
    let decl = Regex::new(
        r#"(?s)const\s+(\w+)\s*:\s*\[\s*&(?:'static\s+)?str\s*;\s*(\d+)\s*\]\s*=\s*\[(.*?)\]\s*;"#,
    )?;
    let addr = Regex::new(r#""(0x[0-9a-fA-F]{40})""#)?;
    // Форма-ЗРІЗ: `const NAME: &[&str] = &[ … ];` — довжини в типі немає, тож
    // незалежного свідка теж немає. Приймаємо, але вимагаємо непорожнечу:
    // мовчки імпортований порожній перелік — це вимкнена перевірка в бою.
    let slice = Regex::new(
        r#"(?s)const\s+(\w+)\s*:\s*&\[\s*&(?:'static\s+)?str\s*\]\s*=\s*&\[(.*?)\]\s*;"#,
    )?;
    let mut out: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (rel, src) in rs_files(root) {
        for c in slice.captures_iter(&src) {
            let name = c[1].to_string();
            let addrs: Vec<String> =
                addr.captures_iter(&c[2]).map(|m| m[1].to_lowercase()).collect();
            if addrs.is_empty() {
                continue;
            }
            let key = format!("{name}:{}", addrs.join(","));
            if seen.insert(key) {
                out.push(json!({ "name": name, "source": rel, "addresses": addrs }));
            }
        }
        for c in decl.captures_iter(&src) {
            let name = c[1].to_string();
            let want: usize = c[2].parse().unwrap_or(0);
            let addrs: Vec<String> =
                addr.captures_iter(&c[3]).map(|m| m[1].to_lowercase()).collect();
            if addrs.is_empty() {
                continue; // перелік не з адрес (символи спам-токенів тощо)
            }
            if addrs.len() != want {
                bail!(
                    "{rel}: `{name}` оголошено на {want} елементів, видобуто {} — \
                     перелік неповний, імпортувати не можна",
                    addrs.len()
                );
            }
            // один і той самий масив зустрічається і в архіві, і в робочій копії
            let key = format!("{name}:{}", addrs.join(","));
            if !seen.insert(key) {
                continue;
            }
            out.push(json!({ "name": name, "source": rel, "addresses": addrs }));
        }
    }
    Ok(out)
}

/// Перелік спалених адрес із тіла `fn is_burn`.
fn extract_burn(root: &Path) -> Result<Vec<String>> {
    let addr = Regex::new(r#""(0x[0-9a-fA-F]{40})""#)?;
    let mut out: BTreeSet<String> = BTreeSet::new();
    for (_rel, src) in rs_files(root) {
        let Some(body) = src.split("fn is_burn").nth(1) else { continue };
        let Some(body) = body.split("\n}").next() else { continue };
        for c in addr.captures_iter(body) {
            out.insert(c[1].to_lowercase());
        }
    }
    Ok(out.into_iter().collect())
}

/// Стартові адреси, з яких розгортається граф, коли фронтир порожній (expand.rs).
fn extract_bootstrap(root: &Path) -> Result<Vec<String>> {
    let re = Regex::new(
        r#"(?s)frontier\.is_empty\(\).{0,200}?\[(\s*"0x[0-9a-fA-F]{40}".*?)\]"#,
    )?;
    let addr = Regex::new(r#""(0x[0-9a-fA-F]{40})""#)?;
    let mut out: BTreeSet<String> = BTreeSet::new();
    for (_rel, src) in rs_files(root) {
        if let Some(c) = re.captures(&src) {
            for m in addr.captures_iter(&c[1]) {
                out.insert(m[1].to_lowercase());
            }
        }
    }
    Ok(out.into_iter().collect())
}


/// Набори адрес УСЕРЕДИНІ функцій: `matches!(a, "0x…" | "0x…")` та
/// `match a { "0x…" => ("Binance","cex"), … }`.
///
/// Остання форма, у якій дані сиділи в коді. Шість модулів rust-server мали
/// власну копію набору `KNOWN` з index.js — і копії РОЗІЙШЛИСЬ: спільних адрес
/// 9, у трьох модулях є ще FTX, у двох інших — ще одна біржа, а forensics не має
/// жодної з двох. Тому тег прив'язаний до файлу І функції: злиття в один
/// перелік мовчки змінило б поведінку трьох модулів.
fn extract_fn_sets(root: &Path) -> Result<Vec<Value>> {
    let fnre = Regex::new(r"(?m)^\s*(?:pub\s+)?fn\s+(\w+)\s*\(")?;
    let addr = Regex::new(r#""(0x[0-9a-fA-F]{40})"(?:\s*=>\s*\(\s*"([^"]*)"\s*,\s*"([^"]*)"\s*\))?"#)?;
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (rel, src) in rs_files(root) {
        let stem = rel
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".rs")
            .to_string();
        for m in fnre.captures_iter(&src) {
            let fname = m[1].to_string();
            let start = m.get(0).map(|x| x.end()).unwrap_or(0);
            // тіло функції — до першого рядка, що починається з `}` у стовпці 0
            let body = match src[start..].find("\n}") {
                Some(e) => &src[start..start + e],
                None => &src[start..],
            };
            let rows: Vec<Value> = addr
                .captures_iter(body)
                .map(|c| {
                    json!({
                        "address": c[1].to_lowercase(),
                        "label": c.get(2).map(|x| x.as_str().to_string()),
                        "kind": c.get(3).map(|x| x.as_str().to_string()),
                    })
                })
                .collect();
            // менш як 3 адреси — це не перелік, а поодинока згадка
            if rows.len() < 3 {
                continue;
            }
            let tag = format!("{stem}:{}", fname.to_lowercase());
            let key = format!(
                "{tag}|{}",
                rows.iter().filter_map(|r| r["address"].as_str()).collect::<Vec<_>>().join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(json!({ "tag": tag, "source": rel, "rows": rows }));
        }
    }
    Ok(out)
}


/// Кортежні масиви: `const N: [(&str, &str); K]` та `[(&str, &str, bool); K]`.
///
/// Остання форма в коді: `STRAT_UNIVERSE` (символ, контракт), `MAJORS` те саме,
/// `RELAYS` (адреса, мітка, чи біржо-суміжне). Порядок полів різний, тож
/// визначаємо його за тим, у якому з них лежить 0x-адреса.
fn extract_tuple_arrays(root: &Path) -> Result<Vec<Value>> {
    let decl = Regex::new(
        r#"(?s)const\s+(\w+)\s*:\s*\[\s*\((?:[^)]*)\)\s*;\s*(\d+)\s*\]\s*=\s*\[(.*?)\]\s*;"#,
    )?;
    let two = Regex::new(r#"\(\s*"([^"]*)"\s*,\s*"([^"]*)"\s*\)"#)?;
    let three = Regex::new(r#"\(\s*"([^"]*)"\s*,\s*"([^"]*)"\s*,\s*(true|false)\s*\)"#)?;
    let isaddr = Regex::new(r#"^0x[0-9a-fA-F]{40}$"#)?;
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (rel, src) in rs_files(root) {
        let stem = rel
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".rs")
            .to_string();
        for c in decl.captures_iter(&src) {
            let name = c[1].to_string();
            let want: usize = c[2].parse().unwrap_or(0);
            let body = &c[3];
            let mut rows = Vec::new();
            for t in three.captures_iter(body) {
                rows.push(json!({
                    "address": t[1].to_lowercase(),
                    "label": t[2].to_string(),
                    "flag": &t[3] == "true",
                }));
            }
            if rows.is_empty() {
                for t in two.captures_iter(body) {
                    let (a, b) = (t[1].to_string(), t[2].to_string());
                    let (addr, label) = if isaddr.is_match(&a) {
                        (a, b)
                    } else if isaddr.is_match(&b) {
                        (b, a)
                    } else {
                        continue; // пара без адреси — не наш випадок
                    };
                    rows.push(json!({ "address": addr.to_lowercase(), "label": label }));
                }
            }
            if rows.is_empty() {
                continue;
            }
            // К-сть може бути меншою за оголошену: у MAJORS є ("ETH", "") без адреси.
            // Це не втрата — просто не кожен рядок несе адресу.
            if rows.len() > want {
                bail!("{rel}: `{name}` оголошено на {want}, видобуто {}", rows.len());
            }
            let tag = format!("{stem}:{}", name.to_lowercase());
            let key = format!(
                "{tag}|{}",
                rows.iter().filter_map(|r| r["address"].as_str()).collect::<Vec<_>>().join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(json!({ "tag": tag, "source": rel, "declared": want, "rows": rows }));
        }
    }
    Ok(out)
}

fn cmd_extract() -> Result<()> {
    let root = repo_root();
    let dir = seed_dir();
    fs::create_dir_all(&dir)?;

    let entities = extract_entities(&root)?;
    let money = extract_real_money(&root)?;
    let mut consts = extract_consts(&root)?;
    let (base, cg) = extract_universe(&root)?;
    let (infra, treasury) = extract_watchlist(&root)?;
    let kex = extract_known_exchange(&root)?;
    let addr_consts = extract_addr_consts(&root)?;
    let label_maps = extract_label_maps(&root)?;
    let pairs = extract_labelled_pairs(&root)?;
    let stables = extract_stable_lists(&root)?;
    let arrays = extract_addr_arrays(&root)?;
    let burn = extract_burn(&root)?;
    let boot = extract_bootstrap(&root)?;
    let fnsets = extract_fn_sets(&root)?;
    let tuples = extract_tuple_arrays(&root)?;

    // CG_IDS — теж константа, і її місце тут, у seed. Синтез на кроці `load`
    // розсинхронізував би звірку seed↔база й зробив перевірку марною.
    consts.push(json!({
        "key": "UNIVERSE_CG_IDS",
        "value": cg.join(","),
        "kind": "text",
        "description": "Список id для запиту до CoinGecko. НЕ відповідає порядку токенів: \
                        відповідь зіставляється за символом (27 id проти 30 токенів).",
    }));

    let write = |name: &str, v: &Value| -> Result<()> {
        let p = dir.join(name);
        fs::write(&p, serde_json::to_string_pretty(v)? + "\n")?;
        Ok(())
    };
    write("entities.json", &json!(entities))?;
    write("real-money.json", &json!(money))?;
    write("constants.json", &json!(consts))?;
    write("universe.json", &json!({ "base": base, "cg_ids": cg }))?;
    write("watchlist.json", &json!({ "infra": infra, "treasury": treasury }))?;
    write("known-exchange.json", &json!(kex))?;
    write("addr-consts.json", &json!(addr_consts))?;
    write("label-maps.json", &json!(label_maps))?;
    write("labelled-pairs.json", &json!(pairs))?;
    write("stable-lists.json", &json!(stables))?;
    write("addr-arrays.json", &json!(arrays))?;
    write("burn.json", &json!(burn))?;
    write("bootstrap.json", &json!(boot))?;
    write("fn-sets.json", &json!(fnsets))?;
    write("tuple-arrays.json", &json!(tuples))?;

    println!("видобуто у {}", dir.display());
    println!("  entities.rs      {:>4} гаманців", entities.len());
    println!("  wr-dict money    {:>4} контрактів", money.len());
    println!("  wr-dict consts   {:>4} порогів", consts.len());
    println!("  universe BASE    {:>4} токенів ({} cg-id)", base.len(), cg.len());
    println!("  cexsignal INFRA  {:>4} (+{} казна)", infra.len(), treasury.len());
    println!("  known_exchange   {:>4} (таблиця, якої вже немає в коді)", kex.len());
    println!("  адреси-константи {:>4}", addr_consts.len());
    println!("  мапи з мітками   {:>4}", label_maps.len());
    println!("  пари з підписом  {:>4}", pairs.len());
    println!("  списки стейблів  {:>4}", stables.len());
    println!("  іменовані масиви {:>4}", arrays.len());
    println!("  спалені адреси   {:>4}", burn.len());
    println!("  bootstrap-сіди   {:>4}", boot.len());
    println!("  набори у функціях{:>4}", fnsets.len());
    println!("  кортежні масиви {:>4}", tuples.len());
    Ok(())
}

// ─────────────────────────── завантаження ───────────────────────────

const CHAINS: &[(&str, &str, &str)] = &[
    ("eth", "Ethereum", "ETH"),
    ("arbitrum", "Arbitrum One", "ETH"),
    ("polygon", "Polygon", "MATIC"),
    ("base", "Base", "ETH"),
    ("optimism", "OP Mainnet", "ETH"),
    ("linea", "Linea", "ETH"),
    ("blast", "Blast", "ETH"),
    ("mantle", "Mantle", "MNT"),
    ("sonic", "Sonic", "S"),
    ("zora", "Zora", "ETH"),
    ("zksync", "zkSync Era", "ETH"),
    // Не мережа, а явна позначка «класифікація не прив'язана до мережі».
    // Потрібна для списків на кшталт rgprice::STABLES, які в коді документовані
    // як «mainnet+L2»: покласти контракт Base на 'eth' означало б збрехати.
    ("any", "(поза мережею: класифікаційний список)", "-"),
];

fn seed(name: &str) -> Result<Value> {
    let p = seed_dir().join(name);
    let t = fs::read_to_string(&p)
        .with_context(|| format!("немає {} — спершу `wr-import extract`", p.display()))?;
    Ok(serde_json::from_str(&t)?)
}

fn arr(v: &Value) -> &Vec<Value> {
    static EMPTY: Vec<Value> = Vec::new();
    v.as_array().unwrap_or(&EMPTY)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

fn cmd_load() -> Result<()> {
    let path = wr_store::db_path();
    let mut c = wr_store::open_or_create(&path)?;
    let tx = c.transaction()?;

    for (id, name, sym) in CHAINS {
        tx.execute(
            "INSERT INTO chains(id,name,native_symbol) VALUES (?1,?2,?3)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name",
            rusqlite::params![id, name, sym],
        )?;
    }

    // --- константи ---
    let consts = seed("constants.json")?;
    for v in arr(&consts) {
        tx.execute(
            "INSERT INTO constants(key,value,kind,description) VALUES (?1,?2,?3,?4)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, kind=excluded.kind",
            rusqlite::params![s(v, "key"), s(v, "value"), s(v, "kind"), s(v, "description")],
        )?;
    }
    let n_consts = arr(&consts).len();

    // --- токени: спершу «справжні гроші» (там є достовірні decimals) ---
    let money = seed("real-money.json")?;
    for v in arr(&money) {
        tx.execute(
            "INSERT INTO tokens(chain,contract,symbol,decimals,money_kind,source,verified)
             VALUES (?1,?2,?3,?4,?5,'wr-dict',1)
             ON CONFLICT(chain,contract) DO UPDATE SET
               decimals=excluded.decimals, money_kind=excluded.money_kind, verified=1",
            rusqlite::params![
                s(v, "chain"),
                s(v, "contract"),
                s(v, "symbol"),
                v.get("decimals").and_then(|x| x.as_i64()),
                s(v, "money_kind"),
            ],
        )?;
    }
    let n_money = arr(&money).len();

    // --- universe: decimals НЕ вигадуємо, лишаємо NULL ---
    let uni = seed("universe.json")?;
    let base = uni.get("base").cloned().unwrap_or(json!([]));
    let cg: Vec<String> = uni
        .get("cg_ids")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    // CG_IDS НЕ розкладається по токенах. Коментар у universe.rs каже «in BASE order»,
    // але це неправда: 27 id проти 30 токенів, SHIB і PEPE у кінці списку. Позиційний zip
    // почав би брехати з четвертого елемента (SHIB↔uniswap). Коду це не шкодить — там
    // список іде цілим у запит до CoinGecko, а відповідь зіставляється ЗА СИМВОЛОМ.
    // Тому він лежить однією константою (її кладе крок `extract`).
    let _ = &cg;

    let mut pos = 0i64;
    for v in arr(&base) {
        let contract = s(v, "c").to_lowercase();
        let sym = s(v, "sym");
        tx.execute(
            "INSERT INTO tokens(chain,contract,symbol,sector,source,verified)
             VALUES ('eth',?1,?2,?3,'universe.rs',1)
             ON CONFLICT(chain,contract) DO UPDATE SET
               symbol=excluded.symbol, sector=COALESCE(excluded.sector,tokens.sector)",
            rusqlite::params![contract, sym, s(v, "sector")],
        )?;
        tx.execute(
            "INSERT INTO universe(chain,contract,size,position) VALUES ('eth',?1,?2,?3)
             ON CONFLICT(chain,contract) DO UPDATE SET size=excluded.size",
            rusqlite::params![contract, v.get("size").and_then(|x| x.as_f64()).unwrap_or(0.0), pos],
        )?;
        pos += 1;
    }
    let n_uni = arr(&base).len();

    // --- гаманці: мітки бірж ---
    let ents = seed("entities.json")?;
    for v in arr(&ents) {
        let kind = match s(v, "kind").as_str() {
            "" => "unknown".to_string(),
            k => k.to_string(),
        };
        let entity = s(v, "label").split(':').next().unwrap_or("").trim().to_string();
        let entity = entity.split_whitespace().next().unwrap_or("").to_string();
        tx.execute(
            "INSERT INTO wallets(chain,address,kind,label,entity,source)
             VALUES ('eth',?1,?2,?3,?4,'entities.rs')
             ON CONFLICT(chain,address) DO UPDATE SET
               kind=excluded.kind, label=excluded.label, entity=excluded.entity",
            rusqlite::params![s(v, "address"), kind, s(v, "label"), entity],
        )?;
    }
    let n_ents = arr(&ents).len();

    // known_exchange: доливаємо ті, кого не було в entities.rs. НЕ перетираємо
    // наявну мітку — у entities.rs вона точніша ("Binance 15" проти "Binance").
    let kex = seed("known-exchange.json")?;
    for v in arr(&kex) {
        tx.execute(
            "INSERT INTO wallets(chain,address,kind,label,entity,source)
             VALUES ('eth',?1,'cex',?2,?2,'wr-dict:known_exchange')
             ON CONFLICT(chain,address) DO NOTHING",
            rusqlite::params![s(v, "address"), s(v, "label")],
        )?;
    }
    let n_kex = arr(&kex).len();

    // --- гаманці під наглядом + їхні ролі ---
    let wl = seed("watchlist.json")?;
    let infra = wl.get("infra").cloned().unwrap_or(json!([]));
    let treasury: Vec<String> = wl
        .get("treasury")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    tx.execute(
        "INSERT INTO groups(id,name,kind,monitored,notes)
         VALUES ('NET','Мережа під наглядом (cexsignal)','investigation',1,
                 'Перенесено з backfill/src/cexsignal.rs')
         ON CONFLICT(id) DO NOTHING",
        [],
    )?;

    for v in arr(&infra) {
        let addr = s(v, "address");
        let label = s(v, "label");
        tx.execute(
            "INSERT INTO wallets(chain,address,kind,label,source,notes)
             VALUES ('eth',?1,'eoa',?2,'cexsignal.rs','інфраструктура мережі під наглядом')
             ON CONFLICT(chain,address) DO UPDATE SET label=excluded.label",
            rusqlite::params![addr, label],
        )?;
        let role = if treasury.contains(&addr) {
            "treasury"
        } else if label.contains("КОНЦЕНТРАТОР") {
            "concentrator"
        } else if label.contains("ДИСТРИБ") {
            "distributor"
        } else if label.contains("хаб") {
            "hub"
        } else {
            "core"
        };
        tx.execute(
            "INSERT INTO wallet_roles(chain,address,role,ctx,notes)
             VALUES ('eth',?1,?2,'NET',?3) ON CONFLICT DO NOTHING",
            rusqlite::params![addr, role, label],
        )?;
        tx.execute(
            "INSERT INTO group_members(group_id,chain,address,role,confidence,joined_via)
             VALUES ('NET','eth',?1,?2,1.0,'cexsignal.rs') ON CONFLICT DO NOTHING",
            rusqlite::params![addr, role],
        )?;
    }
    let n_infra = arr(&infra).len();

    // --- реєстр секретів (значення тут НЕ зберігаються) ---
    let secrets: &[(&str, bool, &str)] = &[
        ("ETHERSCAN_API_KEY", true, "краул ланцюга"),
        ("TELEGRAM_BOT_TOKEN", true, "алерти й бот"),
        ("TELEGRAM_CHAT_ID", true, "куди слати алерти"),
        ("APP_PASSWORD", true, "вхід до вебінтерфейсу"),
        ("ADMIN_PASSWORD", true, "адмінський вхід"),
        ("MORALIS_API_KEY", false, "баланси токенів"),
        ("MORALIS_STREAM_SECRET", false, "підпис вебхука Moralis"),
        ("LUNARCRUSH_API_KEY", false, "соціальні метрики"),
        ("COINGECKO_API_KEY", false, "ціни/капіталізації"),
        ("ETHPLORER_API_KEY", false, "баланси"),
        ("LLM_API_KEY", false, "асистент"),
        ("SENTRY_DSN", false, "телеметрія помилок"),
    ];
    for (name, req, desc) in secrets {
        tx.execute(
            "INSERT INTO secrets(name,service,account,required,env_fallback,description)
             VALUES (?1,'wallet-radar',?1,?2,?1,?3)
             ON CONFLICT(name) DO UPDATE SET required=excluded.required",
            rusqlite::params![name, *req as i64, desc],
        )?;
    }


    // адреси-константи (нульова, burn, WETH-квота тощо)
    let ac = seed("addr-consts.json")?;
    for v in arr(&ac) {
        tx.execute(
            "INSERT INTO constants(key,value,kind,description) VALUES (?1,?2,'hex',?3)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![s(v, "key"), s(v, "value"), s(v, "description")],
        )?;
    }
    let n_ac = arr(&ac).len();

    // мапи «адреса => (мітка, вид)»
    let lm = seed("label-maps.json")?;
    let mut n_lm = 0usize;
    for v in arr(&lm) {
        let kind = match s(v, "kind").as_str() {
            k @ ("cex" | "bridge" | "contract" | "pool" | "router" | "eoa" | "defi" | "issuer"
                | "burn") => k.to_string(),
            _ => "unknown".to_string(),
        };
        let label = s(v, "label");
        let entity = label.split(&[':', ' '][..]).next().unwrap_or("").to_string();
        tx.execute(
            "INSERT INTO wallets(chain,address,kind,label,entity,source)
             VALUES ('eth',?1,?2,?3,?4,?5)
             ON CONFLICT(chain,address) DO NOTHING",
            rusqlite::params![s(v, "address"), kind, label, entity, s(v, "source")],
        )?;
        n_lm += 1;
    }

    // списки стейблів -> токени (усі вони 'usd'); мережу тут не знаємо,
    // тож кладемо на 'eth' лише ті, яких ще немає, щоб не вигадувати мережу
    let st = seed("stable-lists.json")?;
    let mut n_st = 0usize;
    for v in arr(&st) {
        let src = s(v, "source");
        // dexswap перелічує САМЕ мейннет-контракти; rgprice документує свій список
        // як «mainnet+L2» одним плоским масивом без мережі — його не можна
        // видавати за ethereum.
        let chain = if src.contains("dexswap") { "eth" } else { "any" };
        tx.execute(
            "INSERT INTO tokens(chain,contract,symbol,money_kind,source,verified)
             VALUES (?4,?1,?2,'usd',?3,0)
             ON CONFLICT(chain,contract) DO NOTHING",
            rusqlite::params![s(v, "contract"), s(v, "symbol"), src, chain],
        )?;
        n_st += 1;
    }

    // пари (адреса, підпис): токени розпізнаємо за коротким символом,
    // решта — гаманці з нотаткою. Вид не вигадуємо: 'unknown' чесніше.
    let lp = seed("labelled-pairs.json")?;
    let mut n_tok = 0usize;
    let mut n_wal = 0usize;
    for v in arr(&lp) {
        let label = s(v, "label");
        let looks_like_symbol = label.len() <= 12
            && !label.contains(' ')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if looks_like_symbol {
            tx.execute(
                "INSERT INTO tokens(chain,contract,symbol,source,verified)
                 VALUES ('eth',?1,?2,?3,0) ON CONFLICT(chain,contract) DO NOTHING",
                rusqlite::params![s(v, "address"), label, s(v, "source")],
            )?;
            n_tok += 1;
        } else {
            tx.execute(
                "INSERT INTO wallets(chain,address,kind,label,source,notes)
                 VALUES ('eth',?1,'unknown',?2,?3,?2) ON CONFLICT(chain,address) DO NOTHING",
                rusqlite::params![s(v, "address"), label, s(v, "source")],
            )?;
            n_wal += 1;
        }
    }


    // ── іменовані переліки → wallets/tokens + тег ──
    // Тег = ім'я масиву в коді, у нижньому регістрі. Вид гаманця виводимо з імені:
    // вгадувати не треба, назви однозначні.
    let arrays = seed("addr-arrays.json")?;
    let mut n_tag = 0usize;
    for a in arr(&arrays) {
        let name = s(a, "name");
        let src = s(a, "source");
        // Тег прив'язаний до ФАЙЛУ, а не лише до імені масиву. `KNOWN_CEX` існує
        // і у feed.rs (6 адрес), і в exchange_netflow.rs (9) — спільний тег зробив би
        // кожен із них ширшим, ніж був. Це виглядало б як покращення й було б
        // тихою зміною поведінки обох модулів.
        let stem = src
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".rs")
            .to_string();
        let tag = format!("{stem}:{}", name.to_lowercase());
        let list: Vec<String> = a["addresses"]
            .as_array()
            .map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();

        // ETH_TOKENS — це контракти, решта переліків — гаманці
        let is_tokens = name.contains("TOKEN");
        for addr in list {
            if is_tokens {
                tx.execute(
                    "INSERT INTO tokens(chain,contract,symbol,source,verified)
                     VALUES ('eth',?1,'?',?2,0) ON CONFLICT(chain,contract) DO NOTHING",
                    rusqlite::params![addr, src],
                )?;
                tx.execute(
                    "INSERT INTO token_tags(chain,contract,tag,source)
                     VALUES ('eth',?1,?2,?3) ON CONFLICT DO NOTHING",
                    rusqlite::params![addr, tag, src],
                )?;
            } else {
                let kind =
                    if tag.contains("cex") || name == "PARIBU" { "cex" } else { "eoa" };
                tx.execute(
                    "INSERT INTO wallets(chain,address,kind,source)
                     VALUES ('eth',?1,?2,?3) ON CONFLICT(chain,address) DO NOTHING",
                    rusqlite::params![addr, kind, src],
                )?;
                tx.execute(
                    "INSERT INTO wallet_tags(chain,address,tag,source)
                     VALUES ('eth',?1,?2,?3) ON CONFLICT DO NOTHING",
                    rusqlite::params![addr, tag, src],
                )?;
                // TREASURY/DISPENSERS/COLLECTORS/RELAYS — це ролі, не просто теги
                let role = match name.as_str() {
                    "TREASURY" => Some("treasury"),
                    "DISPENSERS" => Some("distributor"),
                    "COLLECTORS" => Some("core"),
                    "RELAYS" => Some("core"),
                    _ => None,
                };
                if let Some(r) = role {
                    tx.execute(
                        "INSERT INTO wallet_roles(chain,address,role,ctx,notes)
                         VALUES ('eth',?1,?2,'NET',?3) ON CONFLICT DO NOTHING",
                        rusqlite::params![addr, r, src],
                    )?;
                }
            }
            n_tag += 1;
        }
    }

    // ── спалені адреси ──
    let burn = seed("burn.json")?;
    let mut n_burn = 0usize;
    for v in arr(&burn) {
        let addr = v.as_str().unwrap_or_default().to_string();
        if addr.is_empty() {
            continue;
        }
        tx.execute(
            "INSERT INTO wallets(chain,address,kind,label,source)
             VALUES ('eth',?1,'burn','Burn','is_burn()')
             ON CONFLICT(chain,address) DO UPDATE SET kind='burn'",
            rusqlite::params![addr],
        )?;
        tx.execute(
            "INSERT INTO wallet_tags(chain,address,tag,source)
             VALUES ('eth',?1,'burn','is_burn()') ON CONFLICT DO NOTHING",
            rusqlite::params![addr],
        )?;
        n_burn += 1;
    }

    // ── bootstrap-сіди розгортання графа ──
    let boot = seed("bootstrap.json")?;
    let mut n_boot = 0usize;
    for v in arr(&boot) {
        let addr = v.as_str().unwrap_or_default().to_string();
        if addr.is_empty() {
            continue;
        }
        tx.execute(
            "INSERT INTO wallets(chain,address,kind,source,notes)
             VALUES ('eth',?1,'eoa','expand.rs','стартова точка розгортання графа')
             ON CONFLICT(chain,address) DO NOTHING",
            rusqlite::params![addr],
        )?;
        tx.execute(
            "INSERT INTO wallet_tags(chain,address,tag,source)
             VALUES ('eth',?1,'expand_bootstrap','expand.rs') ON CONFLICT DO NOTHING",
            rusqlite::params![addr],
        )?;
        n_boot += 1;
    }


    // ── набори адрес усередині функцій ──
    let fnsets = seed("fn-sets.json")?;
    let mut n_fs = 0usize;
    for g in arr(&fnsets) {
        let tag = s(g, "tag");
        let src = s(g, "source");
        for r in g["rows"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let addr = s(r, "address");
            let label = r["label"].as_str().unwrap_or("");
            let kind = match r["kind"].as_str() {
                Some(k @ ("cex" | "burn" | "bridge" | "defi" | "issuer" | "pool" | "contract")) => k,
                _ => "unknown",
            };
            tx.execute(
                "INSERT INTO wallets(chain,address,kind,label,source)
                 VALUES ('eth',?1,?2,NULLIF(?3,''),?4) ON CONFLICT(chain,address) DO NOTHING",
                rusqlite::params![addr, kind, label, src],
            )?;
            tx.execute(
                "INSERT INTO wallet_tags(chain,address,tag,source)
                 VALUES ('eth',?1,?2,?3) ON CONFLICT DO NOTHING",
                rusqlite::params![addr, tag, src],
            )?;
            n_fs += 1;
        }
    }


    // ── кортежні масиви: символ+контракт або адреса+мітка+прапорець ──
    let tuples = seed("tuple-arrays.json")?;
    let mut n_tu = 0usize;
    for g in arr(&tuples) {
        let tag = s(g, "tag");
        let src = s(g, "source");
        for r in g["rows"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let addr = s(r, "address");
            let label = s(r, "label");
            let has_flag = r.get("flag").is_some();
            if has_flag {
                // (адреса, мітка, прапорець) — це гаманці
                tx.execute(
                    "INSERT INTO wallets(chain,address,kind,label,source,notes)
                     VALUES ('eth',?1,'eoa',?2,?3,?4) ON CONFLICT(chain,address) DO NOTHING",
                    rusqlite::params![
                        addr,
                        label,
                        src,
                        if r["flag"].as_bool().unwrap_or(false) { "біржо-суміжне" } else { "" }
                    ],
                )?;
                tx.execute(
                    "INSERT INTO wallet_tags(chain,address,tag,source,notes)
                     VALUES ('eth',?1,?2,?3,?4) ON CONFLICT DO NOTHING",
                    rusqlite::params![
                        addr,
                        tag,
                        src,
                        if r["flag"].as_bool().unwrap_or(false) { "exchange_adjacent" } else { "" }
                    ],
                )?;
            } else {
                // (символ, контракт) — це токени
                tx.execute(
                    "INSERT INTO tokens(chain,contract,symbol,source,verified)
                     VALUES ('eth',?1,?2,?3,0) ON CONFLICT(chain,contract) DO NOTHING",
                    rusqlite::params![addr, label, src],
                )?;
                tx.execute(
                    "INSERT INTO token_tags(chain,contract,tag,source)
                     VALUES ('eth',?1,?2,?3) ON CONFLICT DO NOTHING",
                    rusqlite::params![addr, tag, src],
                )?;
            }
            n_tu += 1;
        }
    }


    // ── ручний вотчліст (manual-watch.json) ──
    // Для адрес, які ЖОДЕН екстрактор не дістане: вони сиділи всередині рядкових
    // літералів як значення env за замовчуванням (campwatch: TOKENS/HOLDERS).
    // Файл ведеться руками й лежить у WR_SEED, тобто поза репозиторієм.
    if let Ok(mw) = seed("manual-watch.json") {
        for g in arr(&mw) {
            let tag = s(g, "tag");
            let src = s(g, "source");
            let is_token = s(g, "kind") == "token";
            for r in g["rows"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                let addr = s(r, "address");
                let label = s(r, "label");
                if is_token {
                    tx.execute(
                        "INSERT INTO tokens(chain,contract,symbol,source,verified)
                         VALUES ('eth',?1,?2,?3,0) ON CONFLICT(chain,contract) DO NOTHING",
                        rusqlite::params![addr, label, src],
                    )?;
                    tx.execute(
                        "INSERT INTO token_tags(chain,contract,tag,source)
                         VALUES ('eth',?1,?2,?3) ON CONFLICT DO NOTHING",
                        rusqlite::params![addr, tag, src],
                    )?;
                } else {
                    tx.execute(
                        "INSERT INTO wallets(chain,address,kind,label,source)
                         VALUES ('eth',?1,'eoa',?2,?3) ON CONFLICT(chain,address) DO NOTHING",
                        rusqlite::params![addr, label, src],
                    )?;
                    tx.execute(
                        "INSERT INTO wallet_tags(chain,address,tag,source)
                         VALUES ('eth',?1,?2,?3) ON CONFLICT DO NOTHING",
                        rusqlite::params![addr, tag, src],
                    )?;
                }
            }
        }
    }

    tx.commit()?;
    println!("завантажено у {}", path.display());
    println!("  мережі      {:>4}", CHAINS.len());
    println!("  константи   {:>4}", n_consts);
    println!("  гроші       {:>4}", n_money);
    println!("  universe    {:>4}", n_uni);
    println!("  мітки бірж  {:>4} (+{} із wr-dict)", n_ents, n_kex);
    println!("  під наглядом{:>4}", n_infra);
    println!("  секрети     {:>4} (лише реєстр, значень немає)", secrets.len());
    println!("  адр-константи{:>4}", n_ac);
    println!("  мапи міток  {:>4}", n_lm);
    println!("  стейбли     {:>4}", n_st);
    println!("  пари: токенів {:>2}, гаманців {:>2}", n_tok, n_wal);
    println!("  теги        {:>4}", n_tag);
    println!("  спалені     {:>4}", n_burn);
    println!("  bootstrap   {:>4}", n_boot);
    println!("  набори-фн   {:>4}", n_fs);
    println!("  кортежі     {:>4}", n_tu);
    Ok(())
}

// ─────────────────────────── звірка ───────────────────────────

fn count(c: &Connection, sql: &str) -> Result<i64> {
    Ok(c.query_row(sql, [], |r| r.get(0))?)
}

/// Відрізає `#[cfg(test)] mod … { … }`.
///
/// Лічильник «скільки адрес лишилось у коді» рахував і тестові фікстури —
/// `0x1111…`, `0x2222…`, vitalik.eth у тестах `bs.rs`. Це не дані, їх нікуди
/// не переносять, і вони завищували обсяг роботи на чверть. Число, яке
/// показує більше за реальність, так само шкідливе, як і те, що показує менше:
/// на нього перестають дивитись.
fn strip_tests(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        // знаходимо `{` після атрибута й пропускаємо збалансований блок
        let Some(open) = tail.find('{') else {
            break;
        };
        let mut depth = 0i32;
        let mut end = None;
        for (k, ch) in tail[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + k + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => rest = &tail[e..],
            None => break, // незбалансовано — краще лишити як є, ніж з'їсти пів файла
        }
    }
    out.push_str(rest);
    out
}

fn cmd_verify() -> Result<()> {
    let c = wr_store::open()?;
    let root = repo_root();

    println!("── у базі ───────────────────────────────");
    for (label, sql) in [
        ("мережі", "SELECT COUNT(*) FROM chains"),
        ("константи", "SELECT COUNT(*) FROM constants"),
        ("токени", "SELECT COUNT(*) FROM tokens"),
        ("  з них «гроші»", "SELECT COUNT(*) FROM tokens WHERE money_kind IS NOT NULL"),
        ("  без decimals", "SELECT COUNT(*) FROM tokens WHERE decimals IS NULL"),
        ("universe", "SELECT COUNT(*) FROM universe"),
        ("гаманці", "SELECT COUNT(*) FROM wallets"),
        ("ролі", "SELECT COUNT(*) FROM wallet_roles"),
        ("групи", "SELECT COUNT(*) FROM groups"),
        ("члени груп", "SELECT COUNT(*) FROM group_members"),
        ("секрети (реєстр)", "SELECT COUNT(*) FROM secrets"),
    ] {
        println!("{label:<18} {:>6}", count(&c, sql)?);
    }

    // Зворотна звірка: КОЖЕН рядок seed мусить знайтись у базі.
    //
    // Раніше тут порівнювались кількості — і перевірка почала брехати, щойно в
    // одну таблицю пішли дані з кількох seed (константи 17 у файлі проти 23 у базі).
    // Кількості до того ж можуть випадково зійтись при різному вмісті. Тому множини:
    // питання не «скільки», а «чи все на місці».
    println!("\n── seed проти бази (порядково) ─────────");
    let mut bad = 0;

    let mut check = |file: &str, key: &str, sql: &str| -> Result<()> {
        let v = seed(file)?;
        // деякі seed — об'єкти ({infra, treasury}), а не масиви
        let rows = match v.as_array() {
            Some(a) => a.clone(),
            None => v
                .as_object()
                .map(|o| o.values().filter_map(|x| x.as_array()).flatten().cloned().collect())
                .unwrap_or_default(),
        };
        let mut have: BTreeSet<String> = BTreeSet::new();
        let mut q = c.prepare(sql)?;
        for r in q.query_map([], |r| r.get::<_, String>(0))? {
            have.insert(r?.to_lowercase());
        }
        let missing: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get(key).and_then(|x| x.as_str()))
            .map(|x| x.to_lowercase())
            .filter(|x| !have.contains(x))
            .collect();
        if missing.is_empty() {
            println!("{file:<22} {:>4} рядків — усі в базі", rows.len());
        } else {
            bad += 1;
            println!(
                "{file:<22} {:>4} рядків — НЕМАЄ {}: {:?}",
                rows.len(),
                missing.len(),
                missing.iter().take(5).collect::<Vec<_>>()
            );
        }
        Ok(())
    };

    check("entities.json", "address", "SELECT address FROM wallets")?;
    check("known-exchange.json", "address", "SELECT address FROM wallets")?;
    check("watchlist.json", "address", "SELECT address FROM wallets")?;
    check("label-maps.json", "address", "SELECT address FROM wallets")?;
    check("real-money.json", "contract", "SELECT contract FROM tokens WHERE money_kind IS NOT NULL")?;
    check("stable-lists.json", "contract", "SELECT contract FROM tokens")?;
    check("constants.json", "key", "SELECT key FROM constants")?;
    check("addr-consts.json", "key", "SELECT key FROM constants")?;

    // що ЩЕ лишилось у коді — це план на етапи 3–5
    println!("\n── адреси, які ще сидять у .rs ──────────");
    let re = Regex::new(r#""(0x[0-9a-fA-F]{40})""#)?;
    let mut known: BTreeSet<String> = BTreeSet::new();
    {
        let mut q = c.prepare("SELECT address FROM wallets")?;
        for r in q.query_map([], |r| r.get::<_, String>(0))? {
            known.insert(r?);
        }
        let mut q = c.prepare("SELECT contract FROM tokens")?;
        for r in q.query_map([], |r| r.get::<_, String>(0))? {
            known.insert(r?);
        }
        // Адреси-константи (нульова, burn) живуть у `constants`, а не в `wallets`.
        // Без цього лічильник показував їх як «ще не перенесено» у 40 файлах —
        // хоча вони вже в базі. Число, яке завищує роботу, так само марне, як і те,
        // що її занижує.
        let mut q = c.prepare("SELECT value FROM constants WHERE kind='hex'")?;
        for r in q.query_map([], |r| r.get::<_, String>(0))? {
            known.insert(r?);
        }
    }
    let mut left: Vec<(String, usize, usize)> = Vec::new();
    for crate_dir in ["rust-server", "backfill", "patterns", "nextaction", "wr-dict", "features"] {
        let dir = root.join(crate_dir).join("src");
        let mut n = 0usize;
        let mut total = 0usize;
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    // `_backup*` — мертві копії старого коду. Рахувати їх означає
                    // показувати роботу, якої немає: 38 із 39 «залишків» у backfill
                    // виявились саме там.
                    if !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("_backup")) {
                        stack.push(p);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let Ok(t) = fs::read_to_string(&p) else { continue };
                    for cap in re.captures_iter(strip_tests(&t).as_str()) {
                        total += 1;
                        if !known.contains(&cap[1].to_lowercase()) {
                            n += 1;
                        }
                    }
                }
            }
        }
        if total > 0 {
            left.push((crate_dir.to_string(), n, total));
        }
    }
    let sum_missing: usize = left.iter().map(|(_, n, _)| n).sum();
    let sum_total: usize = left.iter().map(|(_, _, t)| t).sum();
    // Два різні питання. «Не в базі» — це РИЗИК: прибереш такий рядок із коду
    // і дані зникнуть. «Усього в коді» — це ОБСЯГ ЧИСТКИ: скільки ще переписати
    // на читання з бази. Перше має бути нулем завжди, друге падає до нуля в кінці.
    println!("{:<18} {:>8} {:>10}", "", "не в базі", "усього в .rs");
    for (d, n, t) in &left {
        println!("{d:<18} {n:>8} {t:>10}");
    }
    println!("{:<18} {sum_missing:>8} {sum_total:>10}", "РАЗОМ");

    if bad > 0 {
        bail!("{bad} розбіжностей між seed і базою");
    }
    Ok(())
}

fn cmd_mirror() -> Result<()> {
    let c = wr_store::open()?;
    let targets = wr_store::mirror::mirror_targets();
    if targets.is_empty() {
        bail!("WR_DB_MIRRORS не задано — дзеркалити нікуди");
    }
    println!("джерело: {}", wr_store::db_path().display());
    let mut failed = 0;
    for o in wr_store::mirror::mirror_all(&c) {
        match &o.error {
            None => println!("  ok   {:>9} Б  {}", o.bytes, o.dest.display()),
            Some(e) => {
                failed += 1;
                println!("  ЗБІЙ            {} — {e}", o.dest.display());
            }
        }
    }
    // seed теж дублюємо — це JSON, тут звичайне копіювання безпечне
    let src_seed = seed_dir();
    if src_seed.is_dir() {
        for t in &targets {
            let dst = t.parent().unwrap_or(Path::new(".")).join("seed");
            if let Err(e) = copy_dir(&src_seed, &dst) {
                failed += 1;
                println!("  ЗБІЙ seed       {} — {e}", dst.display());
            } else {
                println!("  ok   seed       {}", dst.display());
            }
        }
    }

    // Перевіряємо КОЖНУ копію відкриттям, а не наявністю файла: зіпсована база
    // теж має ненульовий розмір і теж лежить на місці.
    for t in &targets {
        match Connection::open(t).and_then(|c| {
            c.query_row("SELECT COUNT(*) FROM wallets", [], |r| r.get::<_, i64>(0))
        }) {
            Ok(n) => println!("  читається       {} ({n} гаманців)", t.display()),
            Err(e) => {
                failed += 1;
                println!("  НЕ ЧИТАЄТЬСЯ     {} — {e}", t.display());
            }
        }
    }

    // Часткова відмова — це відмова: копія, якої немає, гірша за відсутність ілюзій.
    if failed > 0 {
        bail!("{failed} проблем при дзеркаленні");
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if p.is_file() {
            fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "extract" => cmd_extract(),
        "load" => cmd_load(),
        "verify" => cmd_verify(),
        "mirror" => cmd_mirror(),
        _ => {
            eprintln!("вжиток: wr-import extract | load | verify | mirror");
            std::process::exit(2);
        }
    }
}
