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
//! Бот багатокористувацький: підключитися може кожен, хто наді��лає `/start`, і дані
//! проєкту (гаманці, токени, групи, монітори) бачать усі підключені.
//!
//! Але «доступ до даних» і «доступ до машини» — різні речі. Бот виконує команди на ВМ,
//! зокрема довільний shell: це віддалене виконання коду, і воно лишається за власником.
//! Тому ролі дві:
//! - `admin`  — власник (`TELEGRAM_USERNAME`, плюс `TELEGRAM_ADMINS` через кому):
//!   довільний shell, запуск бінів, зміна порогів, журнал команд, керування користувачами;
//! - `viewer` — усі інші підключені: лише читання даних і асистент.
//!
//! Реєстр підключених — `deploy/telegram-users.json`, тож підключення переживає
//! перезапуск демона. Незнайомому чату бот відповідає «напиши /start» і нічого не
//! виконує; спроба без реєстрації пишеться в журнал — мовчки ігнорувати спробу гірше,
//! ніж знати про неї.
//!
//! Якщо заданий `TELEGRAM_ACCESS_CODE`, підключення вимагає коду (`/start <код>`) —
//! тоді бот не є відкритим для всіх, хто його знайде. Якщо код не заданий — підключитися
//! може кожен, хто надішле `/start`.
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
use std::sync::{Mutex, OnceLock};

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
    /// Список адмінів (власник + додаткові). Порожній = немає жодного адміна, тоді
    /// привілейовані команди недоступні взагалі, а бот працює лише на читання.
    admins: Vec<String>,
    /// Необов'язковий код підключення: без нього /start не працює.
    access_code: String,
    admin_pass: String,
    /// Пароль ролі «глядач» у веб-API. Саме ним бот ходить у /api/ask від імені
    /// підключеного, хто не є власником: viewer не може випадково отримати admin.
    viewer_pass: String,
    port: String,
    model: String,
}

fn cfg() -> &'static Cfg {
    static C: OnceLock<Cfg> = OnceLock::new();
    C.get_or_init(|| {
        // Корінь: спершу APP_ROOT (як у systemd-юніті), потім аргумент. Так одна й та сама
        // копія коду працює і під systemd, і вручну, і в юніт-тестах.
        let root = std::env::var("APP_ROOT").ok().filter(|s| !s.is_empty())
            .or_else(|| std::env::args().nth(1))
            .unwrap_or_else(|| "/opt/wallet-tracker".into());
        let env = std::fs::read_to_string(format!("{root}/.env")).unwrap_or_default();
        let get = |k: &str| -> String {
            env.lines()
                .find(|l| l.starts_with(&format!("{k}=")))
                .and_then(|l| l.splitn(2, '=').nth(1))
                .unwrap_or("")
                .trim()
                .to_string()
        };
        // Адміни — список: TELEGRAM_USERNAME (власник) + TELEGRAM_ADMINS (через кому).
        let mut admins: Vec<String> = get("TELEGRAM_USERNAME")
            .split(',')
            .map(|s| s.trim().trim_start_matches('@').to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        for a in get("TELEGRAM_ADMINS").split(',') {
            let a = a.trim().trim_start_matches('@').to_lowercase();
            if !a.is_empty() && !admins.contains(&a) { admins.push(a); }
        }
        Cfg {
            token: get("TELEGRAM_BOT_TOKEN"),
            admins,
            access_code: get("TELEGRAM_ACCESS_CODE"),
            admin_pass: get("ADMIN_PASSWORD"),
            viewer_pass: get("APP_PASSWORD"),
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

// ─────────────────────────── ролі й реєстр ───────────────────────────

/// Роль підключеного користувача.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Admin,
    Viewer,
}

impl Role {
    fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Viewer => "viewer",
        }
    }
}

/// Хто саме пише в чат — щоб відповідь і журнал знали, чиї це дії.
struct Caller {
    chat: String,
    username: String,
    name: String,
    role: Role,
}

/// Реєстр підключених — `deploy/telegram-users.json`.
fn users_path() -> String {
    users_path_in(&cfg().root)
}

fn users_path_in(root: &str) -> String {
    format!("{root}/deploy/telegram-users.json")
}

/// Читаємо реєстр. Файл може бути відсутній (ще ніхто не підключався) або покоситись —
/// тоді реєстр починається з порожнього, а не з паники.
fn load_users() -> Value {
    load_users_from(&users_path())
}

fn load_users_from(path: &str) -> Value {
    let mut v = read_json(path).unwrap_or_else(|| json!({}));
    if !v["users"].is_object() {
        v["users"] = json!({});
    }
    v
}

/// Атомарний запис: спершу .tmp, потім перейменування. Якщо демон упаде посеред
/// запису, реєстр лишиться цілим — а втрата реєстра = втрата доступу для всіх.
fn save_users(v: &Value) -> bool {
    save_users_to(&users_path(), v)
}

fn save_users_to(path: &str, v: &Value) -> bool {
    // tmp-файл із PID: два процеси (або два тести) не б'ються за одне ім'я, інакше
    // rename одного зникне під other's write — і реєстр не збережеться.
    let tmp = format!("{path}.tmp.{}", std::process::id());
    match serde_json::to_string_pretty(v) {
        Ok(body) => {
            if std::fs::write(&tmp, body).is_err() {
                return false;
            }
            let ok = std::fs::rename(&tmp, path).is_ok();
            if !ok {
                let _ = std::fs::remove_file(&tmp);
            }
            ok
        }
        Err(_) => false,
    }
}

/// Чи цей username — власник (адмін). Ім'я сильніше за реєстр: власник завжди адмін,
/// навіть якщо його записали в реєстр як глядача.
fn is_admin_name(username: &str) -> bool {
    is_admin_in(username, &cfg().admins)
}

fn is_admin_in(username: &str, admins: &[String]) -> bool {
    let u = username.trim().trim_start_matches('@').to_lowercase();
    !u.is_empty() && admins.iter().any(|a| *a == u)
}

/// Роль чату: спершу ім'я (адмін — завжди), потім реєстр. `None` = не підключений.
fn role_of(users: &Value, chat: &str, username: &str) -> Option<Role> {
    role_of_in(users, chat, username, &cfg().admins)
}

fn role_of_in(users: &Value, chat: &str, username: &str, admins: &[String]) -> Option<Role> {
    if is_admin_in(username, admins) {
        return Some(Role::Admin);
    }
    match users["users"].get(chat).and_then(|u| u["role"].as_str()) {
        Some("admin") => Some(Role::Admin),
        Some("viewer") => Some(Role::Viewer),
        _ => None,
    }
}

/// Додає або оновлює запис підключеного.
fn upsert_user(users: &mut Value, chat: &str, username: &str, name: &str, role: Role) {
    let mut u = users["users"].get(chat).cloned().unwrap_or_else(|| json!({}));
    u["chat"] = json!(chat);
    u["username"] = json!(username);
    u["name"] = json!(name);
    u["role"] = json!(role.as_str());
    u["joined"] = json!(u["joined"].as_u64().unwrap_or(now_secs()));
    u["last"] = json!(now_secs());
    users["users"][chat] = u;
}

fn remove_user(users: &mut Value, chat: &str) -> bool {
    match users["users"].as_object_mut() {
        Some(map) => map.remove(chat).is_some(),
        None => false,
    }
}

/// Список підключених — для адмінської `/users`.
fn users_list(users: &Value) -> Vec<Value> {
    users["users"]
        .as_object()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default()
}

/// Кому можна розсилати: усі підключені, крім тих, хто сам попросив тишу.
fn broadcast_targets() -> Vec<String> {
    let users = load_users();
    let mut out: Vec<String> = users_list(&users)
        .iter()
        .filter(|u| !u["muted"].as_bool().unwrap_or(false))
        .map(|u| u["chat"].as_str().unwrap_or("").to_string())
        .filter(|c| !c.is_empty())
        .collect();
    out.sort();
    out.truncate(500); // Telegram не любить сотні спамів за раз
    out
}

/// Розсилає текст усім підключеним. Повертає (доставлено чатів, чатів з помилкою).
///
/// Розсилка йде послідовно, з паузою: Telegram відкидає бурст, і десяток чатів
/// підряд — це нормально, а сотня — відмова.
async fn broadcast(text: &str) -> (usize, usize) {
    let targets = broadcast_targets();
    let mut ok = 0usize;
    let mut err = 0usize;
    for chat in targets {
        let mut good = true;
        for part in chunks(text) {
            if !send_to(&chat, &part).await {
                good = false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        if good { ok += 1; } else { err += 1; }
    }
    (ok, err)
}

/// Курсор авторозсилки сигналів: до якого `ts` ми вже відправили.
fn signal_cursor_path() -> String {
    format!("{}/deploy/telegram-signal-cursor.json", cfg().root)
}

/// Читає курсор. Якщо файлу немає — повертає «ще нічого не надсилали» (0), і тоді
/// pending_signals() ініціалізує курсор по найновішому сигналу: інакше перший же
/// запуск ботa заливить усіх сторима історією.
fn load_signal_cursor() -> u64 {
    read_json(&signal_cursor_path())
        .and_then(|v| v["ts"].as_u64())
        .unwrap_or(0)
}

fn save_signal_cursor(ts: u64) {
    let path = signal_cursor_path();
    let _ = std::fs::write(path, format!("{{\"ts\":{ts}}}"));
}

/// Сигнали, які ще не розіслано.
fn pending_signals() -> Vec<Value> {
    let r = &cfg().root;
    let Some(feed) = read_json(&format!("{r}/dataset/alpha-feed.json")) else {
        return vec![];
    };
    let empty = vec![];
    let mut items: Vec<Value> = feed.as_array().unwrap_or(&empty).clone();
    items.sort_by_key(|i| i["ts"].as_u64().unwrap_or(0));
    let cursor = load_signal_cursor();
    let mut pending: Vec<Value> = items
        .into_iter()
        .filter(|i| i["ts"].as_u64().unwrap_or(0) > cursor)
        .collect();
    if cursor == 0 {
        // Перший запуск: не заливаємо історією — стартуємо від найновішого.
        if let Some(last) = pending.last() {
            let ts = last["ts"].as_u64().unwrap_or(0);
            save_signal_cursor(ts);
            pending.clear();
        }
    }
    pending
}

/// Формат одного сигналу для розсилки.
fn signal_text(it: &Value) -> String {
    let ts = it["ts"].as_u64().unwrap_or(0);
    let addr = it["address"].as_str().unwrap_or("");
    let toks = it["tokens"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    format!(
        "<b>Сигнал</b>\n{} <code>{}</code>\n<i>{}</i>",
        esc(&toks),
        esc(&short(addr)),
        if ts > 0 { ago(ts / 1000) } else { "—".to_string() }
    )
}

/// Автодоставка нових сигналів усім підключеним. Повертає кількість розісланих.
async fn broadcast_signals_once() -> usize {
    let pending = pending_signals();
    if pending.is_empty() {
        return 0;
    }
    let targets = broadcast_targets();
    if targets.is_empty() {
        return 0;
    }
    let mut sent = 0usize;
    for it in pending.into_iter().take(10) {
        let ts = it["ts"].as_u64().unwrap_or(0);
        let text = format!("{}\n\n{}", signal_text(&it), "Дані проєкту Wallet Radar · /сигнали — історія");
        let (ok, _err) = broadcast(&text).await;
        if ok == 0 {
            // Нікому не дійшло — не рухаємо курсор, спробуємо наступного разу.
            break;
        }
        save_signal_cursor(ts);
        sent += 1;
    }
    sent
}

/// Скільки команд на хвилину дозволено одному чату.
///
/// Усі чати блять в один веб-API з одного IP, а сервер лімітує глядачів ПО IP — тож
/// без власного ліміту один гаманець вичерпає квоту для всіх підключених.
fn rate_ok(chat: &str, role: Role) -> bool {
    use std::collections::HashMap;
    static M: OnceLock<Mutex<HashMap<String, (u64, u32)>>> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(HashMap::new()));
    let max = match role {
        Role::Admin => 60,
        Role::Viewer => 15,
    };
    let now = now_secs();
    let mut g = m.lock().unwrap();
    let e = g.entry(chat.to_string()).or_insert((now, 0));
    if now.saturating_sub(e.0) >= 60 {
        e.0 = now;
        e.1 = 0;
    }
    if e.1 >= max {
        return false;
    }
    e.1 += 1;
    true
}

// ─────────────────────────── маршрутизація ───────────────────────────

/// Відмова за роллю. Каже ЩО саме закрите — інакше користувач думає, що бот зламаний.
fn denied(what: &str) -> String {
    format!("{what} — лише для власника бота.")
}

/// Розклад команд за ролями.
///
/// `admin` — власник: усе, включно з довільним shell на ВМ.
/// `viewer` — підключений глядач: лише читання даних і асистент.
async fn handle(text: &str, caller: &Caller) -> String {
    let t = text.trim();
    if t.is_empty() {
        return "порожнє повідомлення".to_string();
    }

    // Довільна команда — свідомий префікс, не випадкове речення. Виконування коду на
    // ВМ лишається за власником: для глядача це не «обмежена функція», а різниця між
    // доступом до даних і доступом до машини.
    if let Some(cmd) = t.strip_prefix('!') {
        if caller.role != Role::Admin {
            return denied("довільні команди на ВМ");
        }
        return run_shell(cmd.trim()).await;
    }

    let (head, rest) = match t.split_once(char::is_whitespace) {
        Some((h, r)) => (h.to_lowercase(), r.trim().to_string()),
        None => (t.to_lowercase(), String::new()),
    };
    let is_admin = caller.role == Role::Admin;

    match head.as_str() {
        "/start" => greet(caller.role),
        "/help" | "/довідка" | "/допомога" => help(caller.role),
        "/стан" | "/status" => status(),
        "/гаманець" | "/wallet" => wallet(&rest),
        "/токен" | "/token" => token_info(&rest),
        "/група" | "/group" => group(&rest),
        "/сигнали" | "/signals" => signals(&rest),
        "/запусти" | "/run" => {
            if is_admin { run_bin(&rest).await } else { denied("запуск службових бінів") }
        }
        // читання порогів — усім, зміна — власнику
        "/поріг" | "/env" => {
            if rest.is_empty() { env_cmd("") }
            else if is_admin { env_cmd(&rest) }
            else { denied("зміна порогів") }
        }
        "/лог" | "/log" => {
            if is_admin { tail_log(&rest) } else { denied("перегляд логів") }
        }
        "/діагностика" | "/diag" => diag(caller).await,
        "/хтоя" | "/whoami" => whoami(caller),
        "/рассылка" | "/розсилка" | "/broadcast" => {
            if is_admin { broadcast_cmd(&rest).await } else { denied("розсилка всім") }
        }
        "/тиша" | "/mute" => mute_cmd(caller),
        "/користувачі" | "/users" => {
            if is_admin { users_cmd() } else { denied("список користувачів") }
        }
        "/відключити" | "/revoke" => {
            if is_admin { revoke_cmd(&rest) } else { denied("відключення користувачів") }
        }
        "/права" | "/promote" => {
            if is_admin { role_cmd(&rest, true) } else { denied("зміна прав") }
        }
        "/в правах" | "/demote" => {
            if is_admin { role_cmd(&rest, false) } else { denied("зміна прав") }
        }
        _ => ask(t, caller.role).await,
    }
}

/// Привітання після підключення. Роль видно одразу — щоб не гадати, що тобі доступно.
fn greet(role: Role) -> String {
    let extra = match role {
        Role::Admin => "\nТи — власник: доступні довільні команди, запуск бінів і керування користувачами.",
        Role::Viewer => "\nТи підключений як глядач: дані проєкту, асистент і монітори.",
    };
    format!("<b>Wallet Radar — бот</b>\nПідключено.{extra}\n\n{}", help(role))
}

/// Довідка. Для глядача приховуємо привілейовані команди — щоб не було спокуси
/// натискати те, що все одно відхилиться.
fn help(role: Role) -> String {
    let base = "<b>Wallet Radar — бот</b>\n\n\
     <b>Дані</b>\n\
     /гаманець 0x… — роль, баланс, мітка, активність\n\
     /токен ONDO — ціна, ліквідність, обіг, капіталізація\n\
     /група N — склад групи (без аргументу — список)\n\
     /сигнали [N] — останні сигнали проєкту (за замовчуванням 5)\n\
     /стан — коли востаннє відпрацював кожен монітор\n\
     /діагностика — перевірка всіх ланок відповіді\n\
     /хтоя — мій чат, роль і ліміт\n\n\
     <b>Питання</b>\n\
     Напиши питання без слеша — піде в асистента, а якщо той\n\
     не впізнає, відповість локальна модель (буде позначено).\n\n\
     /тиша — не присилати тобі сигнали й розсилки (лишаєшся підключеним)";
    if role != Role::Admin {
        return format!("{base}\n\nДані проєкту спільні для всіх підключених.");
    }
    format!("{base}\n\n\
     <b>Дії (лише власник)</b>\n\
     /рассылка <текст> — розіслати повідомлення всім підключеним\n\
     /запусти cexsignal MULT=2 — запустити бін із параметрами\n\
     /запусти cexsignal MULT=2 — запустити бін із параметрами\n\
     /поріг — показати збережені параметри\n\
     /поріг cexsignal MIN_USD=500000 — змінити й запам'ятати\n\
     /лог cexsignal — хвіст логу\n\
     !df -h — довільна команда на ВМ\n\n\
     <b>Користувачі</b>\n\
     /користувачі — хто підключений\n\
     /права &lt;chat_id&gt; — зробити адміном\n\
     /в правах &lt;chat_id&gt; — забрати адміна\n\
     /відключити &lt;chat_id&gt; — від'єднати\n\n\
     Запускати можна: cexsignal, campwatch, wakeup, tokprices, tokliq, relay-monitor")
}

/// Хто я: чат, роль, ліміт. Необхідно, коли бот уже під'єднаний до десятка людей —
/// треба бачити, що саме тобі належить.
fn whoami(c: &Caller) -> String {
    let name_line = if c.name.is_empty() {
        String::new()
    } else {
        format!("ім'я: {}\n", esc(&c.name))
    };
    format!(
        "<b>Ти</b>\nchat_id: <code>{}</code>\n{}ім'я: @{}\nроль: <b>{}</b>\nліміт: {} команд/хв",
        esc(&c.chat),
        name_line,
        esc(&c.username),
        c.role.as_str(),
        match c.role { Role::Admin => 60, Role::Viewer => 15 }
    )
}

/// Розсилка довільного тексту всім підключеним — команда власника.
async fn broadcast_cmd(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "що розсилати? /рассылка <текст>".to_string();
    }
    log_cmd("РОЗСИЛКА", text);
    let (ok, err) = broadcast(text).await;
    if ok == 0 && err == 0 {
        return "підключених немає — розсилати нікому.".to_string();
    }
    format!("доставлено {ok} підключеним{}", if err > 0 { format!(", з помилкою {err}") } else { String::new() })
}

/// Тиша — користувач лишається підключеним, але йому не приходить розсилка.
fn mute_cmd(caller: &Caller) -> String {
    let mut users = load_users();
    let Some(rec) = users["users"].get(&caller.chat).cloned() else {
        return "ти не підключений — напиши /start.".to_string();
    };
    let muted = !rec["muted"].as_bool().unwrap_or(false);
    users["users"][&caller.chat]["muted"] = json!(muted);
    if save_users(&users) {
        if muted {
            "тиха увімкнена: сигнали й розсилки тобі не приходитимуть. /хтоя — перевірити.".to_string()
        } else {
            "тиша вимкнена: сигнали знову приходитимуть.".to_string()
        }
    } else {
        "не зміг зберегти реєстр — спробуй ще раз.".to_string()
    }
}

/// Список підключених — адмінська команда.
fn users_cmd() -> String {
    let users = load_users();
    let list = users_list(&users);
    if list.is_empty() {
        return "підключених ще немає.".to_string();
    }
    let mut out = format!("<b>Підключені: {}</b>\n", list.len());
    let mut sorted = list;
    sorted.sort_by_key(|u| u["joined"].as_u64().unwrap_or(0));
    for u in sorted {
        let last = u["last"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "<code>{}</code> @{} — {} · {}{}\n",
            esc(u["chat"].as_str().unwrap_or("?")),
            esc(u["username"].as_str().unwrap_or("?")),
            esc(u["role"].as_str().unwrap_or("?")),
            esc(u["name"].as_str().unwrap_or("")),
            if last > 0 { format!(" · {}", ago(last)) } else { String::new() }
        ));
    }
    out
}

/// Від'єднати користувача.
fn revoke_cmd(arg: &str) -> String {
    let chat = arg.trim();
    if chat.is_empty() {
        return "кого відключити? /відключити <chat_id>".to_string();
    }
    let mut users = load_users();
    if remove_user(&mut users, chat) {
        if save_users(&users) {
            format!("користувача {chat} відключено.")
        } else {
            "не зміг зберегти реєстр — спробуй ще раз.".to_string()
        }
    } else {
        format!("користувача {chat} у реєстрі немає.")
    }
}

/// Надати або забрати права адміна.
fn role_cmd(arg: &str, admin: bool) -> String {
    let chat = arg.trim();
    if chat.is_empty() {
        return "format: /права <chat_id> або /в правах <chat_id>".to_string();
    }
    let mut users = load_users();
    let Some(rec) = users["users"].get(chat).cloned() else {
        return format!("{chat} не підключений — спершу /start з його боку.")
            .to_string();
    };
    // Власник визначений іменем у .env — прибрати його права командою не можна,
    // бо ім'я сильніше за реєстр. Треба правити TELEGRAM_USERNAME/TELEGRAM_ADMINS.
    if is_admin_name(rec["username"].as_str().unwrap_or("")) {
        return format!(
            "{chat} — власник (список TELEGRAM_USERNAME/TELEGRAM_ADMINS). \
             Прибрати треба звідти, а не командою."
        );
    }
    let role = if admin { "admin" } else { "viewer" };
    users["users"][chat]["role"] = json!(role);
    if save_users(&users) {
        format!("{chat} тепер {role}.")
    } else {
        "не зміг зберегти реєстр — спробуй ще раз.".to_string()
    }
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
/// Останні сигнали з alpha-файду — доступно кожному підключеному.
///
/// Це головне, заради чого бот існує: не довідка з документації, а живі сигнали
/// проєкту, які всі бачать однаково.
fn signals(arg: &str) -> String {
    let n: usize = arg.trim().parse().unwrap_or(5).clamp(1, 20);
    let r = &cfg().root;
    let Some(feed) = read_json(&format!("{r}/dataset/alpha-feed.json")) else {
        return "файлу сигналів немає: dataset/alpha-feed.json".to_string();
    };
    let empty = vec![];
    let items: Vec<&Value> = feed.as_array().unwrap_or(&empty).iter().rev().take(n).collect();
    if items.is_empty() {
        return "сигналів поки немає.".to_string();
    }
    let mut out = format!("<b>Останні сигнали</b>\n");
    for it in items {
        let ts = it["ts"].as_u64().unwrap_or(0);
        let addr = it["address"].as_str().unwrap_or("");
        let toks = it["tokens"]
            .as_array()
            .map(|a| a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        out.push_str(&format!(
            "• <b>{}</b> · {} <code>{}</code>\n",
            esc(&toks),
            if ts > 0 { ago(ts / 1000) } else { "—".to_string() },
            esc(&short(addr))
        ));
    }
    out
}

async fn diag(caller: &Caller) -> String {
    let c = cfg();
    let mut out = String::from("<b>ДІАГНОСТИКА</b>\n");
    out.push_str(&format!("корінь: {}\n", esc(&c.root)));
    out.push_str(&format!(
        "власники: {}\n",
        if c.admins.is_empty() { "<b>немає — привілейовані команди вимкнено</b>".to_string() }
        else { c.admins.iter().map(|a| format!("@{a}")).collect::<Vec<_>>().join(", ") }
    ));
    out.push_str(&format!(
        "підключено: {} · твоя роль: {}\n",
        users_list(&load_users()).len(),
        caller.role.as_str()
    ));
    out.push_str(&format!("модель: {}\n", esc(&c.model)));
    out.push_str(&format!("зсув Telegram: {}\n\n", load_offset()));

    // асистент — від твоєї ролі, бо саме так він викликається в роботі
    let url = format!("http://127.0.0.1:{}/api/ask?q=%D1%82%D0%B5%D1%81%D1%82", c.port);
    let (login, pass) = api_creds(caller.role);
    let a = match http().get(&url).basic_auth(login, Some(pass)).send().await {
        Ok(r) if r.status().is_success() => "✅ відповідає".to_string(),
        Ok(r) => format!("⚠️ HTTP {}", r.status().as_u16()),
        Err(e) => format!("❌ {}", e),
    };
    out.push_str(&format!("асистент /api/ask ({login}): {}\n", esc(&a)));

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

/// Якими кредами бот ходить у веб-API від імені цієї ролі.
///
/// Глядач НІКОЛИ не отримує пароль адміна: «багатокористувацький» не має означати
/// «кожен підключений — адмін». Якщо `APP_PASSWORD` не заданий, у глядача асистента
/// через API немає (він впаде на локальну модель, якій дані не видно).
fn api_creds(role: Role) -> (&'static str, &'static str) {
    let c = cfg();
    match role {
        Role::Admin => ("admin", c.admin_pass.as_str()),
        Role::Viewer => ("viewer", c.viewer_pass.as_str()),
    }
}

async fn ask(q: &str, role: Role) -> String {
    // 1. Детермінований асистент — відповідає з даних і не вигадує чисел.
    if let Some(a) = ask_assistant(q, role).await { return a; }
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

async fn ask_assistant(q: &str, role: Role) -> Option<String> {
    let c = cfg();
    let (login, pass) = api_creds(role);
    if pass.is_empty() {
        return None;
    }
    let url = format!("http://127.0.0.1:{}/api/ask?q={}", c.port, urlencode(q));
    let r = http().get(&url).basic_auth(login, Some(pass)).send().await.ok()?;
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

/// Обробка одного повідомлення Telegram: повертає чат і текст відповіді.
///
/// Винесено ОКРЕМО від надсилання — щоб увесь сценарій доступу (реєстрація, ролі,
/// ліміти, відмови) можна було перевірити тестами без справжнього Telegram.
async fn handle_update(msg: &Value) -> Option<(String, String)> {
    let c = cfg();
    let text = msg["text"].as_str()?;
    let chat = msg["chat"]["id"].as_i64().unwrap_or(0).to_string();
    let username = msg["from"]["username"].as_str().unwrap_or("").trim().to_lowercase();
    let name = msg["from"]["first_name"].as_str().unwrap_or("").trim().to_string();

    let mut users = load_users();
    match role_of(&users, &chat, &username) {
        // ── підключений ──
        Some(role) => {
            log_cmd(&format!("{}[{}] @{}", chat, role.as_str(), username), text);
            if !rate_ok(&chat, role) {
                return Some((chat, "забагато запитів — зачекай хвилину.".to_string()));
            }
            // Час останньої активності — щоб у /users було видно, хто ще користується.
            upsert_user(&mut users, &chat, &username, &name, role);
            let _ = save_users(&users);
            let caller = Caller { chat: chat.clone(), username, name, role };
            let reply = handle(text, &caller).await;
            Some((chat, reply))
        }
        // ── ще не підключений ──
        None => {
            let (head, rest) = match text.trim().split_once(char::is_whitespace) {
                Some((h, r)) => (h.to_lowercase(), r.trim().to_string()),
                None => (text.trim().to_lowercase(), String::new()),
            };
            if head == "/start" || head == "/код" {
                // Код підключення, якщо його задано: без правильного коду не підключитись.
                if !c.access_code.is_empty() && rest.trim() != c.access_code {
                    eprintln!("[botd] ВІДКИНУТО @{username}: /start без коду");
                    log_cmd(&format!("ВІДКИНУТО:@{username}"), text);
                    return Some((chat, "потрібен код підключення: /start <код>".to_string()));
                }
                let role = if is_admin_name(&username) { Role::Admin } else { Role::Viewer };
                upsert_user(&mut users, &chat, &username, &name, role);
                if save_users(&users) {
                    eprintln!("[botd] підключено @{username} ({}) · роль {}", chat, role.as_str());
                    log_cmd(&format!("ПІДКЛЮЧЕНО:{}[{}] @{}", chat, role.as_str(), username), text);
                    Some((chat, greet(role)))
                } else {
                    Some((chat, "не зміг зберегти реєстр — спробуй /start ще раз.".to_string()))
                }
            } else {
                // Не підключений — нічого не виконуємо, але й не мовчимо: спроба
                // записується в журнал, бо мовчазне ігнорування гірше за відомість.
                eprintln!("[botd] ВІДКИНУТО від @{username} (не підключений)");
                log_cmd(&format!("НЕПІДКЛЮЧЕНИЙ:@{username}"), text);
                Some((chat, "Я ще тебе не знаю. Напиши /start — і підключишся.".to_string()))
            }
        }
    }
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
        let Some((chat, reply)) = handle_update(msg).await else { continue };
        for part in chunks(&reply) {
            send_to(&chat, &part).await;
        }
    }
    // Автодоставка нових сигналів усім підключеним. Один цикл опитування — це
    // щонайменше 25 секунд, тож це природній такт: надто часто — шум.
    let n = broadcast_signals_once().await;
    if n > 0 {
        eprintln!("[botd] розіслано сигналів: {n}");
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
    if c.admins.is_empty() {
        // Без власника немає привілейованих команд (довільний shell, запуск бінів,
        // керування користувачами) — але ДАНІ лишаються доступними тим, хто підключився.
        // Раніше тут був вихід: тоді бот не слухав нікого взагалі.
        eprintln!("[botd] ⚠️ TELEGRAM_USERNAME/TELEGRAM_ADMINS порожні — привілейовані команди вимкнено, дані доступні всім підключеним");
    } else {
        eprintln!(
            "[botd] слухаю команди від усіх підключених · адміни: {} · корінь {}",
            c.admins.iter().map(|a| format!("@{a}")).collect::<Vec<_>>().join(", "),
            c.root
        );
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn admins() -> Vec<String> {
        vec!["vitalii_vedana".to_string()]
    }

    fn tmp_registry(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("botd-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn unknown_chat_has_no_role() {
        let users = load_users_from("/nonexistent/telegram-users.json");
        assert_eq!(role_of_in(&users, "111", "someone", &admins()), None);
    }

    #[test]
    fn registered_user_gets_viewer_role() {
        let mut users = load_users_from("/nonexistent/telegram-users.json");
        upsert_user(&mut users, "111", "someone", "Someone", Role::Viewer);
        assert_eq!(role_of_in(&users, "111", "someone", &admins()), Some(Role::Viewer));
    }

    #[test]
    fn owner_is_admin_by_name_even_without_registration() {
        let users = load_users_from("/nonexistent/telegram-users.json");
        assert_eq!(role_of_in(&users, "999", "vitalii_vedana", &admins()), Some(Role::Admin));
        // Регістр не може звузити права власника: ім'я сильніше.
        let mut u = users.clone();
        upsert_user(&mut u, "999", "vitalii_vedana", "Owner", Role::Viewer);
        assert_eq!(role_of_in(&u, "999", "vitalii_vedana", &admins()), Some(Role::Admin));
    }

    #[test]
    fn registry_survives_disk_roundtrip() {
        let path = tmp_registry("roundtrip");
        let p = path.to_string_lossy().to_string();
        let mut users = load_users_from(&p);
        upsert_user(&mut users, "111", "someone", "Someone", Role::Viewer);
        upsert_user(&mut users, "222", "owner", "Owner", Role::Admin);
        assert!(save_users_to(&p, &users));

        let back = load_users_from(&p);
        assert_eq!(role_of_in(&back, "111", "someone", &admins()), Some(Role::Viewer));
        assert_eq!(role_of_in(&back, "222", "owner", &admins()), Some(Role::Admin));
        assert_eq!(users_list(&back).len(), 2);

        // Від'єднання — теж має бути перевірним.
        let mut u = back;
        assert!(remove_user(&mut u, "111"));
        assert!(!remove_user(&mut u, "111"));
        assert_eq!(role_of_in(&u, "111", "someone", &admins()), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_registry_does_not_panic() {
        let path = tmp_registry("corrupt");
        std::fs::write(&path, b"{not json at all").unwrap();
        let users = load_users_from(path.to_str().unwrap());
        assert!(users["users"].is_object());
        assert!(users_list(&users).is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn viewer_is_rate_limited_and_admin_is_not() {
        let chat = format!("rate-test-{}", std::process::id());
        let mut allowed = 0;
        for _ in 0..20 {
            if rate_ok(&chat, Role::Viewer) {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 15, "глядач має бути обмежений 15 командами/хв");

        let admin_chat = format!("rate-admin-{}", std::process::id());
        let mut admin_allowed = 0;
        for _ in 0..70 {
            if rate_ok(&admin_chat, Role::Admin) {
                admin_allowed += 1;
            }
        }
        assert_eq!(admin_allowed, 60, "власник має мати більший ліміт");
    }

    /// Тести, що торкаються СПІЛЬНОГО реєстру, мусять іти послідовно: файл один
    /// на весь процес, і паралельні тести перетирали б один одному записи.
    fn registry_lock() -> std::sync::MutexGuard<'static, ()> {
        static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
        M.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Корінь для тестів: тимчасова копія з мінімальним .env (є власник).
    fn ensure_root() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let dir = std::env::temp_dir().join(format!("botd-root-{}", std::process::id()));
            let _ = std::fs::create_dir_all(dir.join("deploy"));
            let _ = std::fs::write(
                dir.join(".env"),
                "TELEGRAM_USERNAME=owner\nAPP_PASSWORD=viewerpass\nADMIN_PASSWORD=adminpass\n",
            );
            std::env::set_var("APP_ROOT", dir.to_string_lossy().to_string());
            let _ = cfg(); // фіксуємо корінь, поки APP_ROOT ще правильний
        });
    }

    /// Повідомлення у вигляді, який приходить з getUpdates (об'єкт message).
    fn msg(chat: i64, user: &str, name: &str, text: &str) -> Value {
        json!({
            "chat": {"id": chat},
            "from": {"username": user, "first_name": name},
            "text": text
        })
    }

    #[tokio::test]
    async fn many_users_connect_and_only_owner_gets_shell() {
        let _guard = registry_lock();
        ensure_root();

        // Незнайомець просить /start — і підключається.
        let (_, reply) = handle_update(&msg(5001, "alice", "Alice", "/start")).await.unwrap();
        assert!(reply.contains("Підключено"), "відповідь: {reply}");
        assert!(reply.contains("глядач"), "має бути сказано про роль: {reply}");

        let (_, reply) = handle_update(&msg(5002, "bob", "Bob", "/start")).await.unwrap();
        assert!(reply.contains("Підключено"), "відповідь: {reply}");

        let users = load_users();
        assert_eq!(role_of(&users, "5001", "alice"), Some(Role::Viewer));
        assert_eq!(role_of(&users, "5002", "bob"), Some(Role::Viewer));
        // Реєстр спільний для процесу, тож перевіряємо наявність, а не загальний рахунок.
        assert!(users_list(&users).iter().any(|u| u["chat"] == "5001"));
        assert!(users_list(&users).iter().any(|u| u["chat"] == "5002"));

        // Глядач не ганяє довільний shell на ВМ.
        let (_, reply) = handle_update(&msg(5001, "alice", "Alice", "!whoami")).await.unwrap();
        assert!(reply.contains("лише для власника"), "відповідь: {reply}");

        // І не керує користувачами.
        let (_, reply) = handle_update(&msg(5001, "alice", "Alice", "/користувачі")).await.unwrap();
        assert!(reply.contains("лише для власника"), "відповідь: {reply}");

        // Власник (за ім'ям у .env) — адмін: список користувачів доступний.
        let (_, reply) = handle_update(&msg(5003, "owner", "Owner", "/start")).await.unwrap();
        assert!(reply.contains("власник"), "відповідь: {reply}");
        let (_, reply) = handle_update(&msg(5003, "owner", "Owner", "/користувачі")).await.unwrap();
        assert!(reply.contains("Підключені"), "відповідь: {reply}");
        assert!(reply.contains("alice"), "у списку має бути alice: {reply}");
    }

    #[tokio::test]
    async fn unknown_user_is_asked_to_start() {
        let _guard = registry_lock();
        ensure_root();
        let (_, reply) = handle_update(&msg(6001, "stranger", "Stranger", "/стан")).await.unwrap();
        assert!(reply.contains("/start"), "відповідь: {reply}");
        // І в реєстрі його немає.
        let users = load_users();
        assert_eq!(role_of(&users, "6001", "stranger"), None);
    }

    #[tokio::test]
    async fn muted_user_is_skipped_in_broadcast() {
        let _guard = registry_lock();
        ensure_root();
        // Два підключені.
        let _ = handle_update(&msg(7001, "ann", "Ann", "/start")).await.unwrap();
        let _ = handle_update(&msg(7002, "bob2", "Bob", "/start")).await.unwrap();
        assert!(broadcast_targets().contains(&"7001".to_string()));

        // Один просить тишу — лишається підключеним, але розсилка йому не йде.
        let mut users = load_users();
        users["users"]["7001"]["muted"] = json!(true);
        assert!(save_users(&users));
        let targets = broadcast_targets();
        assert!(!targets.contains(&"7001".to_string()), "заглушений не має отримувати розсилку");
        // При цьому він лишається в реєстрі — підключення не зникло.
        let users = load_users();
        assert_eq!(role_of(&users, "7001", "ann"), Some(Role::Viewer));
    }

    #[tokio::test]
    async fn first_run_does_not_flood_with_history() {
        let _guard = registry_lock();
        ensure_root();
        let dir = std::env::var("APP_ROOT").unwrap();
        std::fs::create_dir_all(format!("{dir}/dataset")).unwrap();
        let feed = json!([
            {"address": "0xaa", "tokens": ["AAA"], "ts": 1000},
            {"address": "0xbb", "tokens": ["BBB"], "ts": 2000}
        ]);
        std::fs::write(format!("{dir}/dataset/alpha-feed.json"), feed.to_string()).unwrap();
        let _ = std::fs::remove_file(format!("{dir}/deploy/telegram-signal-cursor.json"));

        // Перший запуск: історія НЕ надсилається, лише фіксується курсор.
        assert!(pending_signals().is_empty(), "перший запуск не має лити історію");
        assert!(std::fs::metadata(format!("{dir}/deploy/telegram-signal-cursor.json")).is_ok());

        // Новий сигнал — ось тепер має розіслатися.
        let feed = json!([
            {"address": "0xaa", "tokens": ["AAA"], "ts": 1000},
            {"address": "0xbb", "tokens": ["BBB"], "ts": 2000},
            {"address": "0xcc", "tokens": ["CCC"], "ts": 3000}
        ]);
        std::fs::write(format!("{dir}/dataset/alpha-feed.json"), feed.to_string()).unwrap();
        let pending = pending_signals();
        assert_eq!(pending.len(), 1, "лише новий сигнал");
        assert_eq!(pending[0]["ts"].as_u64(), Some(3000));
    }

    #[test]
    fn access_code_gates_registration() {
        // Код підключення не має бути в реєстрі й не має з'являтися в ролях:
        // перевірка коду — це умова в poll_once, а не стан користувача.
        let mut users = load_users_from("/nonexistent/telegram-users.json");
        upsert_user(&mut users, "111", "someone", "Someone", Role::Viewer);
        assert!(users["users"]["111"]["role"] == "viewer");
        assert!(users["users"]["111"].get("code").is_none());
    }
}
