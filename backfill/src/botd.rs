//! botd — двосторонній Telegram-бот як ОКРЕМИЙ демон.
//!
//! ЧОМУ ОКРЕМО, А НЕ В rust-server. Спершу він був модулем веб-сервера — і тут-таки
//! виявилось, чому це погана ідея: дерево `rust-server` не збиралося цілу добу через
//! незавершену правку в `portfolio.rs`, і бот, якого там ще немає, вже був заблокований.
//! Веб-сервер переписують постійно; бот, прив'язаний до його збірки, успадковує кожну
//! його поломку. А цього тижня ми вже бачили, чим коштує мовчазно померлий монітор.
//!
//! Тепер бот залежить лише від власного бінарника. Асистент `/api/ask` доступний по
//! HTTP, дані — просто файли; нічого з того, що бот уміє, не потребувало жити всередині
//! сервера.
//!
//! ─────────────────────────── ДОСТУП ───────────────────────────
//! Бот виконує команди на ВМ, зокрема довільні shell-команди. Це віддалене виконання
//! коду, тож впускаємо рівно одного: `TELEGRAM_USERNAME` з `.env`. Повідомлення від
//! будь-кого іншого відкидається Й ЗАПИСУЄТЬСЯ — мовчки ігнорувати спробу гірше, ніж
//! знати про неї. Порожній `TELEGRAM_USERNAME` = команди вимкнено повністю.
//!
//! Довільний shell вимагає префікса `!`. Не заради безпеки (доступ той самий), а щоб
//! випадкове речення не пішло в `sh -c`. Усе виконане лягає в `deploy/bot-commands.log`.
//! ──────────────────────────────────────────────────────────────
//!
//! Вільний текст (не команда) йде спершу в `/api/ask` — детермінований асистент, який
//! відповідає з даних і не вигадує чисел. Якщо він не впізнав питання, підхоплює
//! локальна qwen2.5 на GPU, і її відповідь помічається окремо. Порядок принциповий:
//! пустиш модель першою — вона впевнено назве неправильну суму, і відрізнити її від
//! правильної буде нічим.
//!
//! Args: ROOT. Env: BOT_MODEL(qwen2.5:14b) API_PORT(3000)

use serde_json::{json, Value};
use std::io::Write;
use std::sync::OnceLock;

/// Скільки секунд тримати довге опитування. Telegram віддає відповідь одразу, щойно
/// приходить повідомлення, тож це не затримка, а економія запитів.
const POLL_SECS: u64 = 25;

/// Ліміт Telegram на одне повідомлення — 4096. Ріжемо з запасом на розмітку.
const TG_LIMIT: usize = 3900;

/// Що дозволено запускати командою `/запусти`. Довільний бінарник сюди не потрапляє —
/// для цього є `!`, і хай це буде свідомим вибором, а не одруком у назві.
const RUNNABLE: [&str; 6] = ["cexsignal", "campwatch", "wakeup", "tokprices", "tokliq", "relay-monitor"];

struct Cfg {
    root: String,
    token: String,
    user: String,
    admin_pass: String,
    port: String,
    model: String,
}

fn cfg() -> &'static Cfg {
    static C: OnceLock<Cfg> = OnceLock::new();
    C.get_or_init(|| {
        let root = std::env::args().nth(1).unwrap_or_else(|| "/opt/wallet-tracker".into());
        let env = std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default();
        let get = |k: &str| -> String {
            env.lines()
                .find(|l| l.starts_with(&format!("{k}=")))
                .and_then(|l| l.splitn(2, '=').nth(1))
                .unwrap_or("")
                .trim()
                .to_string()
        };
        Cfg {
            token: get("TELEGRAM_BOT_TOKEN"),
            user: get("TELEGRAM_USERNAME").trim_start_matches('@').to_lowercase(),
            admin_pass: get("ADMIN_PASSWORD"),
            port: std::env::var("API_PORT").unwrap_or_else(|_| "3000".into()),
            model: std::env::var("BOT_MODEL").unwrap_or_else(|_| "qwen2.5:14b".into()),
            root,
        }
    })
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            // більше за POLL_SECS, інакше довге опитування рвалося б щоразу
            .timeout(std::time::Duration::from_secs(POLL_SECS + 35))
            .build()
            .expect("reqwest client будується зі сталої конфігурації")
    })
}

/// Екранування під Telegram HTML. Вивід команд містить `<` і `&` регулярно, і без цього
/// Telegram відповідає 400, а користувач бачить тишу замість результату.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn read_json(path: &str) -> Option<Value> {
    let f = std::fs::File::open(path).ok()?;
    serde_json::from_reader(std::io::BufReader::new(f)).ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn human_usd(v: f64) -> String {
    if v >= 1e9 { format!("${:.2}млрд", v / 1e9) }
    else if v >= 1e6 { format!("${:.2}млн", v / 1e6) }
    else if v >= 1e3 { format!("${:.1}тис", v / 1e3) }
    else { format!("${v:.0}") }
}

fn ago(ts: u64) -> String {
    let now = now_secs();
    if ts == 0 || ts > now { return "невідомо".to_string(); }
    let d = now - ts;
    if d < 3600 { format!("{} хв тому", d / 60) }
    else if d < 86400 { format!("{} год тому", d / 3600) }
    else { format!("{} дн тому", d / 86400) }
}

fn mtime(path: &str) -> u64 {
    std::fs::metadata(path).ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Журнал виконаного. Пишеться ДО виконання: якщо команда підвісить процес, у журналі
/// однаково лишиться, що саме його підвісило.
fn log_cmd(who: &str, text: &str) {
    let path = format!("{}/deploy/bot-commands.log", cfg().root);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let stamp = chrono::DateTime::from_timestamp(now_secs() as i64, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| now_secs().to_string());
        let _ = writeln!(f, "{stamp}\t{who}\t{text}");
    }
}

// ─────────────────────────── маршрутизація ───────────────────────────

async fn handle(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() { return "порожнє повідомлення".to_string(); }

    // Довільна команда — свідомий префікс, не випадкове речення
    if let Some(cmd) = t.strip_prefix('!') {
        return run_shell(cmd.trim()).await;
    }

    let (head, rest) = match t.split_once(char::is_whitespace) {
        Some((h, r)) => (h.to_lowercase(), r.trim().to_string()),
        None => (t.to_lowercase(), String::new()),
    };

    match head.as_str() {
        "/start" | "/help" | "/довідка" | "/допомога" => help(),
        "/стан" | "/status" => status(),
        "/гаманець" | "/wallet" => wallet(&rest),
        "/токен" | "/token" => token_info(&rest),
        "/група" | "/group" => group(&rest),
        "/запусти" | "/run" => run_bin(&rest).await,
        "/поріг" | "/env" => env_cmd(&rest),
        "/лог" | "/log" => tail_log(&rest),
        "/діагностика" | "/diag" => diag().await,
        _ => ask(t).await,
    }
}

fn help() -> String {
    "<b>Wallet Radar — бот</b>\n\n\
     <b>Дані</b>\n\
     /гаманець 0x… — роль, баланс, мітка, активність\n\
     /токен ONDO — ціна, ліквідність, обіг, капіталізація\n\
     /група N — склад групи (без аргументу — список)\n\
     /стан — коли востаннє відпрацював кожен монітор\n\
     /лог cexsignal — хвіст логу\n\
     /діагностика — перевірка всіх ланок відповіді\n\n\
     <b>Дії</b>\n\
     /запусти cexsignal MULT=2 — запустити бін із параметрами\n\
     /поріг — показати збережені параметри\n\
     /поріг cexsignal MIN_USD=500000 — змінити й запам'ятати\n\
     !df -h — довільна команда на ВМ\n\n\
     <b>Питання</b>\n\
     Напиши питання без слеша — піде в асистента, а якщо той\n\
     не впізнає, відповість локальна модель (буде позначено).\n\n\
     Запускати можна: cexsignal, campwatch, wakeup, tokprices, tokliq, relay-monitor"
        .to_string()
}

/// Здоров'я моніторів. Головне питання не «чи є файл», а «коли він востаннє оновлювався»:
/// саме мовчазне застигання стану на сім днів і лишилось непоміченим у серпні.
fn status() -> String {
    let r = &cfg().root;
    let items: [(&str, String); 4] = [
        ("cexsignal", format!("{r}/deploy/cexsignal.state.json")),
        ("campwatch", format!("{r}/dataset/backfill/campwatch-state.json")),
        ("wakeup", format!("{r}/deploy/wakeup.state.json")),
        ("ціни (tokprices)", format!("{r}/dataset/backfill/token-prices.json")),
    ];
    let mut out = String::from("<b>СТАН МОНІТОРІВ</b>\n");
    for (name, path) in items.iter() {
        let m = mtime(path);
        let size = std::fs::metadata(path).map(|x| x.len()).unwrap_or(0);
        let (mark, note) = if m == 0 { ("❌", "файлу нема".to_string()) }
            else if size == 0 { ("⚠️", "файл ПОРОЖНІЙ".to_string()) }
            else { ("✅", ago(m)) };
        out.push_str(&format!("{mark} <b>{name}</b> — {note}\n"));
    }
    let liq = format!("{r}/dataset/backfill/token-liquidity.json");
    out.push_str(&format!("\nринковий контекст: {}\n", ago(mtime(&liq))));
    out
}

fn wallet(arg: &str) -> String {
    let a = arg.trim().to_lowercase();
    if a.len() != 42 || !a.starts_with("0x") {
        return "потрібна адреса: /гаманець 0x…".to_string();
    }
    let r = &cfg().root;
    let mut out = format!(
        "<b>{}</b>\n<a href=\"https://etherscan.io/address/{a}\">дивитись у Etherscan</a>\n",
        esc(&a));

    if let Some(l) = read_json(&format!("{r}/dataset/labels-cache.json")) {
        if let Some(name) = l[&a]["name"].as_str() {
            if !name.is_empty() { out.push_str(&format!("Мітка: <b>{}</b>\n", esc(name))); }
        }
    }
    if let Some(k) = read_json(&format!("{r}/dataset/backfill/wallet-kind.json")) {
        if let Some(v) = k[&a].as_str() {
            out.push_str(&format!("Тип: {}\n", if v == "contract" { "контракт" } else { v }));
        }
    }
    let mut found = false;
    if let Some(roles) = read_json(&format!("{r}/dataset/backfill/wallet-roles.json")) {
        for x in roles["wallets"].as_array().cloned().unwrap_or_default() {
            if x["w"].as_str().map(|s| s.to_lowercase()).as_deref() != Some(a.as_str()) { continue; }
            found = true;
            out.push_str(&format!("Роль: <b>{}</b>\n", esc(x["role"].as_str().unwrap_or("?"))));
            out.push_str(&format!("Баланс: {:.2} ETH\n", x["balEth"].as_f64().unwrap_or(0.0)));
            let last = x["lastTs"].as_u64().unwrap_or(0);
            if last > 0 { out.push_str(&format!("Останній рух ETH: {}\n", ago(last))); }
            break;
        }
    }
    if !found { out.push_str("\nУ рольовій базі не значиться (walletrole її не бачив).\n"); }
    out
}

fn token_info(arg: &str) -> String {
    let q = arg.trim().to_lowercase();
    if q.is_empty() { return "потрібен символ або контракт: /токен ONDO".to_string(); }
    let r = &cfg().root;
    let liq = read_json(&format!("{r}/dataset/backfill/token-liquidity.json"));
    let px = read_json(&format!("{r}/dataset/backfill/token-prices.json"));

    // шукаємо і за контрактом, і за символом
    let mut hit: Option<(String, Value)> = None;
    if let Some(l) = &liq {
        if let Some(obj) = l["tokens"].as_object() {
            for (k, v) in obj {
                let sym = v["symbol"].as_str().unwrap_or("").to_lowercase();
                if k.to_lowercase() == q || sym == q { hit = Some((k.clone(), v.clone())); break; }
            }
        }
    }
    let Some((addr, v)) = hit else {
        return format!("токен «{}» не знайдено в ринкових даних", esc(&q));
    };
    let sym = v["symbol"].as_str().unwrap_or("?");
    let mut out = format!(
        "<b>{}</b>\n<a href=\"https://etherscan.io/token/{addr}\">{}</a>\n\n",
        esc(sym), esc(&addr));
    out.push_str(&format!("Ціна: ${}\n", v["price"].as_f64().unwrap_or(0.0)));
    out.push_str(&format!("Ліквідність пулів: {}\n", human_usd(v["reserveUsd"].as_f64().unwrap_or(0.0))));
    out.push_str(&format!("Обіг за добу: {}\n", human_usd(v["vol24"].as_f64().unwrap_or(0.0))));
    out.push_str(&format!("Капіталізація: {}\n", human_usd(v["mcap"].as_f64().unwrap_or(0.0))));
    if let Some(p) = px {
        // ДВІ ціни навмисно: розбіжність між DefiLlama і пулами сама по собі
        // діагностична — саме вона колись дала «COMP по $41046».
        if let Some(u) = p["tokens"][&addr]["usd"].as_f64() {
            out.push_str(&format!("\nЦіна за DefiLlama: ${u}\n"));
        }
        if let Some(t) = p["asOf"].as_u64() { out.push_str(&format!("ціни оновлені {}\n", ago(t))); }
    }
    out
}

fn group(arg: &str) -> String {
    let id = arg.trim().to_lowercase();
    let r = &cfg().root;
    let path = format!("{r}/dataset/capital-groups/wallets.json");
    let Some(v) = read_json(&path) else { return format!("нема {}", esc(&path)) };
    let Some(obj) = v.as_object() else { return "несподіваний формат wallets.json".to_string() };

    if id.is_empty() {
        let mut out = String::from("<b>Групи капіталу</b>\n");
        let mut rows: Vec<(String, usize)> = obj.iter()
            .map(|(k, val)| (k.clone(), val.as_array().map(|a| a.len()).unwrap_or(0)))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        for (k, n) in rows.iter().take(40) {
            out.push_str(&format!("• {} — {n} гаманців\n", esc(k)));
        }
        if rows.len() > 40 { out.push_str(&format!("…і ще {}\n", rows.len() - 40)); }
        return out;
    }
    match obj.keys().find(|k| k.to_lowercase() == id || k.to_lowercase().contains(&id)) {
        Some(k) => {
            let arr = obj[k].as_array().cloned().unwrap_or_default();
            let mut out = format!("<b>Група {}</b>\nГаманців: {}\n\nПерші 10:\n", esc(k), arr.len());
            for w in arr.iter().take(10) {
                if let Some(s) = w.as_str() {
                    out.push_str(&format!("• <a href=\"https://etherscan.io/address/{s}\">{}</a>\n",
                        esc(&short(s))));
                }
            }
            out
        }
        None => format!("групи «{}» нема. /група без аргументу — список усіх", esc(&id)),
    }
}

/// Самоперевірка ланцюга відповідей.
///
/// Коли бот «мовчить», причин може бути п'ять, і зовні вони не розрізняються: не дійшло
/// повідомлення, не піднявся асистент, не відповідає модель, немає моделі з підтримкою
/// інструментів, зламана розмітка відповіді. Замість здогадів — перевірка кожної ланки
/// окремо, прямо з телефона.
async fn diag() -> String {
    let c = cfg();
    let mut out = String::from("<b>ДІАГНОСТИКА</b>\n");
    out.push_str(&format!("корінь: {}\n", esc(&c.root)));
    out.push_str(&format!("власник: @{}\n", esc(&c.user)));
    out.push_str(&format!("модель: {}\n", esc(&c.model)));
    out.push_str(&format!("зсув Telegram: {}\n\n", load_offset()));

    // асистент
    let url = format!("http://127.0.0.1:{}/api/ask?q=%D1%82%D0%B5%D1%81%D1%82", c.port);
    let a = match http().get(&url).basic_auth("admin", Some(&c.admin_pass)).send().await {
        Ok(r) if r.status().is_success() => "✅ відповідає".to_string(),
        Ok(r) => format!("⚠️ HTTP {}", r.status().as_u16()),
        Err(e) => format!("❌ {}", e),
    };
    out.push_str(&format!("асистент /api/ask: {}\n", esc(&a)));

    // модель
    let o = match http().get("http://127.0.0.1:11434/api/tags").send().await {
        Ok(r) if r.status().is_success() => {
            let v: Value = r.json().await.unwrap_or(Value::Null);
            let names: Vec<String> = v["models"].as_array().cloned().unwrap_or_default().iter()
                .filter_map(|m| m["name"].as_str().map(|s| s.to_string())).collect();
            let has = names.iter().any(|n| n == &c.model);
            format!("{} моделей {}{}", if has { "✅" } else { "⚠️" }, names.len(),
                if has { String::new() } else { format!(", але «{}» серед них НЕМА", c.model) })
        }
        Ok(r) => format!("⚠️ HTTP {}", r.status().as_u16()),
        Err(e) => format!("❌ {}", e),
    };
    out.push_str(&format!("Ollama: {}\n", esc(&o)));

    // інструменти на живих даних
    let t = tool_exec("monitors_status", &json!({}));
    out.push_str(&format!("інструменти: {}\n", if t.contains("МОНІТОР") { "✅ читають дані" } else { "⚠️ порожньо" }));

    let log = format!("{}/deploy/bot-commands.log", c.root);
    let n = std::fs::read_to_string(&log).map(|s| s.lines().count()).unwrap_or(0);
    out.push_str(&format!("команд у журналі: {n}\n"));
    out
}

fn short(a: &str) -> String {
    if a.len() >= 42 { format!("{}…{}", &a[0..8], &a[a.len() - 4..]) } else { a.to_string() }
}

fn tail_log(arg: &str) -> String {
    let name = arg.trim();
    if name.is_empty() { return "який лог? /лог cexsignal".to_string(); }
    // назва йде у шлях, тож обрізаємо все, що могло б із нього вийти
    if name.contains('/') || name.contains("..") { return "у назві логу не може бути шляху".to_string(); }
    let r = &cfg().root;
    for p in [
        format!("{r}/deploy/{name}.cron.log"),
        format!("{r}/backfill/{name}.cron.log"),
        format!("{r}/deploy/{name}.log"),
    ] {
        if let Ok(s) = std::fs::read_to_string(&p) {
            let tail: Vec<&str> = s.lines().rev().take(25).collect();
            let body: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            return format!("<b>{}</b>\n<pre>{}</pre>", esc(name), esc(&body));
        }
    }
    format!("логу для «{}» не знайшов", esc(name))
}

// ─────────────────────────── дії ───────────────────────────

fn env_cmd(arg: &str) -> String {
    let path = format!("{}/deploy/bot-env.sh", cfg().root);
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if arg.trim().is_empty() {
        return if current.trim().is_empty() {
            format!("Збережених параметрів нема.\nЗадати: /поріг cexsignal MIN_USD=500000\nФайл: {}", esc(&path))
        } else {
            format!("<b>Збережені параметри</b>\n<pre>{}</pre>", esc(&current))
        };
    }
    let mut parts = arg.split_whitespace();
    let Some(bin) = parts.next() else { return "формат: /поріг <бін> KEY=VAL".to_string() };
    let pairs: Vec<&str> = parts.filter(|p| p.contains('=')).collect();
    if pairs.is_empty() { return "потрібна хоча б одна пара KEY=VAL".to_string(); }

    let mut lines: Vec<String> = current.lines().map(|s| s.to_string()).collect();
    for p in pairs {
        let (k, v) = p.split_once('=').unwrap();
        let name = format!("{}_{}", bin.to_uppercase().replace('-', "_"), k.to_uppercase());
        lines.retain(|l| !l.starts_with(&format!("export {name}=")));
        lines.push(format!("export {name}={v}"));
    }
    let body = lines.join("\n") + "\n";
    match std::fs::write(&path, &body) {
        Ok(_) => format!("<b>Збережено</b>\n<pre>{}</pre>\n⚠️ cron підхопить лише якщо його рядок робить <code>source {}</code>",
            esc(&body), esc(&path)),
        Err(e) => format!("не зміг записати {}: {}", esc(&path), esc(&e.to_string())),
    }
}

async fn run_bin(arg: &str) -> String {
    let mut parts = arg.split_whitespace();
    let Some(bin) = parts.next() else {
        return format!("що запустити? дозволені: {}", RUNNABLE.join(", "));
    };
    if !RUNNABLE.contains(&bin) {
        return format!("«{}» не в списку дозволених.\nДозволені: {}\nДля решти є префікс !",
            esc(bin), RUNNABLE.join(", "));
    }
    let r = &cfg().root;
    let path = format!("{r}/backfill/target/release/{bin}");
    if !std::path::Path::new(&path).exists() {
        return format!("бінарника нема: {}", esc(&path));
    }
    let envs: Vec<(String, String)> = parts
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();

    let mut c = tokio::process::Command::new(&path);
    c.arg(r).current_dir(r);
    for (k, v) in &envs { c.env(k, v); }
    match tokio::time::timeout(std::time::Duration::from_secs(900), c.output()).await {
        Ok(Ok(o)) => {
            // біни пишуть діагностику в stderr, тож він і цікавий
            let s = String::from_utf8_lossy(if o.stderr.is_empty() { &o.stdout } else { &o.stderr });
            let tail: Vec<&str> = s.lines().rev().take(30).collect();
            let body: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            format!("<b>{} — код {}</b>\n<pre>{}</pre>", esc(bin), o.status.code().unwrap_or(-1), esc(&body))
        }
        Ok(Err(e)) => format!("не запустився: {}", esc(&e.to_string())),
        Err(_) => format!("{} не вклався у 15 хвилин — перервано", esc(bin)),
    }
}

async fn run_shell(cmd: &str) -> String {
    if cmd.is_empty() { return "порожня команда".to_string(); }
    let mut c = tokio::process::Command::new("sh");
    c.arg("-c").arg(cmd).current_dir(&cfg().root);
    match tokio::time::timeout(std::time::Duration::from_secs(120), c.output()).await {
        Ok(Ok(o)) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr).to_string();
            if !err.trim().is_empty() { s.push_str("\n[stderr]\n"); s.push_str(&err); }
            if s.trim().is_empty() { s = format!("(без виводу, код {})", o.status.code().unwrap_or(-1)); }
            format!("<pre>{}</pre>", esc(s.trim()))
        }
        Ok(Err(e)) => format!("не виконалось: {}", esc(&e.to_string())),
        Err(_) => "перервано: команда йшла довше 2 хвилин".to_string(),
    }
}

// ─────────────────────────── питання ───────────────────────────

async fn ask(q: &str) -> String {
    // 1. Детермінований асистент — відповідає з даних і не вигадує чисел.
    if let Some(a) = ask_assistant(q).await { return a; }
    // 2. Модель З ДОСТУПОМ ДО ДАНИХ: сама вирішує, що подивитись, ми виконуємо.
    if let Some(a) = ask_ollama_tools(q).await { return a; }
    // 3. Модель без даних — останній засіб, і він чесно помічений.
    match ask_ollama(q).await {
        Some(a) => format!("{}\n\n<i>⚠️ модель відповідала БЕЗ доступу до даних проєкту — числа з неї не бери</i>", esc(&a)),
        None => "не зрозумів питання, і модель не відповіла. /довідка — що я вмію".to_string(),
    }
}

/// Прибирає HTML: те саме, що йде в Telegram, згодовується моделі як звичайний текст.
/// Інакше вона починає повторювати теги у відповіді, і Telegram відкидає її з 400.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut skip = false;
    for ch in s.chars() {
        match ch {
            '<' => skip = true,
            '>' => skip = false,
            c if !skip => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
}

/// Інструменти, які модель може викликати. Опис кожного написаний для НЕЇ, а не для
/// людини: від того, наскільки точно тут сказано «коли це доречно», залежить, чи
/// покличе вона потрібну функцію замість того, щоб вигадати відповідь.
fn tool_defs() -> Value {
    json!([
      {"type": "function", "function": {
        "name": "wallet_info",
        "description": "Дані про конкретний гаманець Ethereum: роль у мережі, баланс ETH, мітка, тип (EOA чи контракт), коли востаннє рухав ETH. Викликай ЗАВЖДИ, коли в питанні є адреса 0x...",
        "parameters": {"type": "object", "properties": {
            "address": {"type": "string", "description": "адреса 0x... (42 символи)"}
        }, "required": ["address"]}}},
      {"type": "function", "function": {
        "name": "token_info",
        "description": "Ринкові дані токена: ціна, ліквідність пулів, обіг за добу, капіталізація. Викликай, коли питають про конкретний токен чи монету.",
        "parameters": {"type": "object", "properties": {
            "symbol": {"type": "string", "description": "символ (ONDO, ENA) або адреса контракту"}
        }, "required": ["symbol"]}}},
      {"type": "function", "function": {
        "name": "group_info",
        "description": "Група капіталу: скільки гаманців у ній і які. Без аргументу повертає список усіх груп. Викликай, коли питають про групи.",
        "parameters": {"type": "object", "properties": {
            "name": {"type": "string", "description": "назва або номер групи; порожньо = список усіх"}
        }, "required": []}}},
      {"type": "function", "function": {
        "name": "monitors_status",
        "description": "Стан моніторів: коли кожен востаннє відпрацював, чи не застиг. Викликай, коли питають про моніторинг, алерти, чи все працює.",
        "parameters": {"type": "object", "properties": {}, "required": []}}},
      {"type": "function", "function": {
        "name": "find_by_label",
        "description": "Пошук адрес за міткою: 'binance', 'bitfinex', 'uniswap'. Повертає до 10 адрес із міткою, що містить підрядок.",
        "parameters": {"type": "object", "properties": {
            "query": {"type": "string", "description": "частина назви мітки"}
        }, "required": ["query"]}}},
      {"type": "function", "function": {
        "name": "cex_flows",
        "description": "Скільки мережа заводила на біржі й зливала в пули по днях за останні два тижні, у доларах. Викликай на питання про потоки, заводи на біржі, продажі.",
        "parameters": {"type": "object", "properties": {}, "required": []}}}
    ])
}

/// Виконання інструмента. Повертає ПЛОСКИЙ текст — те саме, що побачив би користувач,
/// лише без розмітки.
fn tool_exec(name: &str, args: &Value) -> String {
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    match name {
        "wallet_info" => strip_tags(&wallet(&s("address"))),
        "token_info" => strip_tags(&token_info(&s("symbol"))),
        "group_info" => strip_tags(&group(&s("name"))),
        "monitors_status" => strip_tags(&status()),
        "find_by_label" => find_by_label(&s("query")),
        "cex_flows" => cex_flows(),
        other => format!("інструмента «{other}» немає"),
    }
}

fn find_by_label(q: &str) -> String {
    let q = q.trim().to_lowercase();
    if q.is_empty() { return "порожній запит".to_string(); }
    let Some(l) = read_json(&format!("{}/dataset/labels-cache.json", cfg().root)) else {
        return "labels-cache недоступний".to_string();
    };
    let Some(obj) = l.as_object() else { return "несподіваний формат labels-cache".to_string() };
    let mut hits: Vec<String> = Vec::new();
    for (addr, v) in obj {
        let name = v["name"].as_str().unwrap_or("");
        if name.to_lowercase().contains(&q) {
            hits.push(format!("{addr} — {name}"));
            if hits.len() >= 10 { break; }
        }
    }
    if hits.is_empty() { format!("адрес із міткою «{q}» не знайдено") } else { hits.join("\n") }
}

/// Потоки з накопиченого стану cexsignal. Це ті самі числа, на яких стоять пороги
/// алертів, тож відповідь бота й алерт не розійдуться.
fn cex_flows() -> String {
    let Some(st) = read_json(&format!("{}/deploy/cexsignal.state.json", cfg().root)) else {
        return "стан cexsignal недоступний".to_string();
    };
    let Some(daily) = st["daily"].as_object() else { return "у стані немає денних сум".to_string() };
    let mut rows: Vec<(i64, f64, f64)> = daily.iter().filter_map(|(d, v)| {
        d.parse::<i64>().ok().map(|day| (day, v["cex"].as_f64().unwrap_or(0.0), v["dex"].as_f64().unwrap_or(0.0)))
    }).collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = String::from("день (UTC) | на біржі | у пули\n");
    for (day, cex, dex) in rows.iter().take(14) {
        let date = chrono::DateTime::from_timestamp(day * 86400, 0)
            .map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| day.to_string());
        out.push_str(&format!("{date} | {} | {}\n", human_usd(*cex), human_usd(*dex)));
    }
    out
}

/// Один виклик Ollama в режимі діалогу (`/api/chat`) — саме він, а не `/api/generate`,
/// уміє інструменти.
async fn ollama_chat(messages: &[Value], with_tools: bool) -> Option<Value> {
    let mut body = json!({
        "model": cfg().model,
        "messages": messages,
        "stream": false,
        // нижча температура: нам потрібні факти з інструментів, а не творчість
        "options": {"temperature": 0.2},
    });
    if with_tools { body["tools"] = tool_defs(); }
    let r = http().post("http://127.0.0.1:11434/api/chat").json(&body).send().await.ok()?;
    if !r.status().is_success() {
        eprintln!("[botd] ollama /api/chat: HTTP {}", r.status().as_u16());
        return None;
    }
    let v: Value = r.json().await.ok()?;
    v.get("message").cloned()
}

/// Модель із доступом до даних: вона вирішує, що подивитись, ми виконуємо і повертаємо.
///
/// Обмеження в 4 кола навмисне: без нього модель, яка не може відповісти, ходить по
/// інструментах нескінченно. Краще чесно сказати, що не вийшло, ніж мовчати хвилинами.
async fn ask_ollama_tools(q: &str) -> Option<String> {
    let system = "Ти помічник у системі аналізу гаманців Ethereum «Wallet Radar».
ГОЛОВНЕ ПРАВИЛО: усі факти й числа бери ВИКЛЮЧНО з інструментів. Якщо для відповіді
потрібні дані — виклич інструмент. Ніколи не вигадуй адрес, сум, цін і дат.
Якщо інструменти не дали потрібного, так і скажи: «даних для цього немає».
Відповідай стисло й українською. Числа наводь так, як їх повернув інструмент.";

    let mut messages = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": q}),
    ];
    let mut used: Vec<String> = Vec::new();

    for _round in 0..4 {
        let msg = ollama_chat(&messages, true).await?;
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        if calls.is_empty() {
            let content = msg["content"].as_str().unwrap_or("").trim().to_string();
            if content.is_empty() { return None; }
            // Без жодного інструмента це звичайна відповідь моделі — не видаємо її
            // за обґрунтовану даними. Хай нею займається останній рівень у ask().
            if used.is_empty() { return None; }
            return Some(format!(
                "{}\n\n<i>дані з: {}</i>",
                esc(&content), esc(&used.join(", "))));
        }
        messages.push(msg.clone());
        for c in calls {
            let name = c["function"]["name"].as_str().unwrap_or("").to_string();
            let args = c["function"]["arguments"].clone();
            let result = tool_exec(&name, &args);
            eprintln!("[botd] інструмент {name}({args}) → {} символів", result.len());
            used.push(name.clone());
            messages.push(json!({"role": "tool", "name": name, "content": result}));
        }
    }
    // Витратили всі кола на інструменти — просимо підсумувати вже без них
    let msg = ollama_chat(&messages, false).await?;
    let content = msg["content"].as_str().unwrap_or("").trim().to_string();
    if content.is_empty() { return None; }
    Some(format!("{}\n\n<i>дані з: {}</i>", esc(&content), esc(&used.join(", "))))
}

async fn ask_assistant(q: &str) -> Option<String> {
    let c = cfg();
    let url = format!("http://127.0.0.1:{}/api/ask?q={}", c.port, urlencode(q));
    let r = http().get(&url).basic_auth("admin", Some(&c.admin_pass)).send().await.ok()?;
    if !r.status().is_success() { return None; }
    let v: Value = r.json().await.ok()?;
    let ans = v["answer"].as_str().unwrap_or("").trim().to_string();
    if ans.is_empty() { return None; }
    let mut out = String::new();
    if let Some(t) = v["topic"].as_str() {
        if !t.is_empty() { out.push_str(&format!("<b>{}</b>\n", esc(t))); }
    }
    out.push_str(&esc(&ans));
    if let Some(s) = v["source"].as_str() {
        if !s.is_empty() { out.push_str(&format!("\n\n<i>джерело: {}</i>", esc(s))); }
    }
    Some(out)
}

async fn ask_ollama(q: &str) -> Option<String> {
    let body = json!({
        "model": cfg().model,
        "prompt": format!("Ти помічник у проєкті аналізу гаманців Ethereum. Відповідай стисло, українською.\n\nПитання: {q}"),
        "stream": false,
    });
    let r = http().post("http://127.0.0.1:11434/api/generate").json(&body).send().await.ok()?;
    let v: Value = r.json().await.ok()?;
    let s = v["response"].as_str()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "+".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

// ─────────────────────────── Telegram ───────────────────────────

async fn send_to(chat: &str, text: &str) -> bool {
    let c = cfg();
    if c.token.is_empty() || chat.is_empty() { return false; }
    let url = format!("https://api.telegram.org/bot{}/sendMessage", c.token);
    let body = json!({
        "chat_id": chat, "text": text,
        "parse_mode": "HTML", "disable_web_page_preview": true,
    });
    match http().post(&url).json(&body).send().await {
        Ok(r) if r.status().is_success() => true,
        Ok(r) => {
            // 400 майже завжди = зламана HTML-розмітка у відповіді. Мовчати не можна:
            // користувач бачив би тишу й вважав бота мертвим.
            let code = r.status().as_u16();
            let why = r.text().await.unwrap_or_default();
            eprintln!("[botd] надіслати не вдалось ({code}): {why}");
            false
        }
        Err(e) => { eprintln!("[botd] мережа: {e}"); false }
    }
}

fn offset_path() -> String { format!("{}/deploy/telegram-offset.txt", cfg().root) }

fn load_offset() -> i64 {
    std::fs::read_to_string(offset_path()).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

fn save_offset(o: i64) { let _ = std::fs::write(offset_path(), o.to_string()); }

/// Розбиття довгої відповіді. Ріжемо по рядках, а не посеред тега — інакше Telegram
/// відкидає повідомлення з незакритим `<pre>`.
fn chunks(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in s.lines() {
        if cur.len() + line.len() + 1 > TG_LIMIT && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if line.len() > TG_LIMIT {
            out.push(line.chars().take(TG_LIMIT).collect());
            continue;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() { out.push(cur); }
    if out.is_empty() { out.push("(порожня відповідь)".to_string()); }
    out
}

async fn poll_once(offset: i64) -> Option<i64> {
    let c = cfg();
    let url = format!("https://api.telegram.org/bot{}/getUpdates?timeout={POLL_SECS}&offset={offset}", c.token);
    let r = http().get(&url).send().await.ok()?;
    if !r.status().is_success() {
        eprintln!("[botd] getUpdates: HTTP {}", r.status().as_u16());
        return None;
    }
    let v: Value = r.json().await.ok()?;
    let ups = v["result"].as_array()?;
    let mut next = offset;
    for u in ups {
        let id = u["update_id"].as_i64().unwrap_or(0);
        if id >= next { next = id + 1; }
        let Some(msg) = u.get("message") else { continue };
        let Some(text) = msg["text"].as_str() else { continue };
        let chat = msg["chat"]["id"].as_i64().unwrap_or(0).to_string();
        let who = msg["from"]["username"].as_str().unwrap_or("").to_lowercase();

        if who != c.user {
            eprintln!("[botd] ВІДКИНУТО від @{who} (дозволено лише @{})", c.user);
            log_cmd(&format!("ВІДКИНУТО:@{who}"), text);
            continue;
        }
        log_cmd(&format!("@{who}"), text);
        let reply = handle(text).await;
        for part in chunks(&reply) {
            send_to(&chat, &part).await;
        }
    }
    Some(next)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let c = cfg();
    if c.token.is_empty() {
        eprintln!("[botd] нема TELEGRAM_BOT_TOKEN у {}/.env — виходжу", c.root);
        std::process::exit(1);
    }
    if c.user.is_empty() {
        // Не «працюємо без обмежень», а саме відмова: бот виконує код на цій машині,
        // тож без явного власника він не слухає нікого.
        eprintln!("[botd] TELEGRAM_USERNAME порожній — команди вимкнено, виходжу");
        std::process::exit(1);
    }
    eprintln!("[botd] слухаю команди від @{} · корінь {}", c.user, c.root);

    let mut offset = load_offset();
    let mut fails = 0u32;
    loop {
        match poll_once(offset).await {
            Some(next) => {
                fails = 0;
                if next != offset { offset = next; save_offset(offset); }
            }
            None => {
                // Мережа падає — відступаємо, але не здаємось: демон має пережити
                // і перезапуск роутера, і тимчасову відмову Telegram.
                fails = (fails + 1).min(6);
                let wait = 5 * (1u64 << fails);
                eprintln!("[botd] опитування не вдалось, чекаю {wait}с");
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
        }
    }
}
