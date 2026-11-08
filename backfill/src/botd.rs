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
//! Бот багатокористувацький: підключитися може кожен, хто надіслає `/start`, і дані
//! проєкту (гаманці, токени, групи, монітори) бачать усі підключені.
//!
//! Але «доступ до даних» і «доступ до машини» — різні речі. Бот виконує команди на ВМ,
//! зокрема довільний shell: це віддалене виконання коду, і воно лишається за власником.
//! Тому ролі дві:
//! - `admin`  — власник (`TELEGRAM_USERNAME`, плюс `TELEGRAM_ADMINS` через кому):
//!   довільний shell, запуск бінів, зміна порогів, журнал команд, керування користувачами;
//! - `viewer` — усі інші підключені: лише читання даних і асистент.
//!
//! ─────────────────────────── ПРАВИЛО РОЗСИЛКИ ───────────────────────────
//! Повідомлення користувача — це ЙОГО особиста справа: відповідь і сповіщення
//! йдуть тільки до нього. Жодне повідомлення (навіть тестове) не спричиняє
//! сповіщень для інших підключених. Автодоставка нових сигналів живе окремим
//! циклом і має КУРСОР НА КОЖНОГО, тож черга кожного — своя.
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

/// Такт автодоставки сигналів. Окремий від опитування Telegram: жодне
/// повідомлення користувача не має спричиняти сповіщення для інших.
const SIGNAL_SECS: u64 = 60;

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
    /// Токен синхронізації реєстру: ним бот пише реєстр у сервер.
    sync_token: String,
    /// Адреса сервера (API). На Windows-машині це ноутбук у локальній мережі.
    api_host: String,
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
            sync_token: get("TELEGRAM_SYNC_TOKEN"),
            api_host: std::env::var("API_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
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

/// Курсори автодоставки сигналів — ПО ОДНОМУ НА КОРИСТУВАЧА.
///
/// Глобальний курсор був неправильним: активність одного користувача (будь-яке
/// повідомлення) запускала розсилку й просувала чергу для всіх одразу. Тепер
/// кожен має свій курсор і отримує саме ті сигнали, яких ще не бачив.
fn signal_cursors_path() -> String {
    format!("{}/deploy/telegram-signal-cursors.json", cfg().root)
}

fn load_cursors() -> Value {
    let mut v = read_json(&signal_cursors_path()).unwrap_or_else(|| json!({}));
    if !v.is_object() {
        v = json!({});
    }
    v
}

/// Атомарний запис курсорів: файл малий, але він — «памʼять» того, хто що бачив.
fn save_cursors(v: &Value) -> bool {
    let path = signal_cursors_path();
    let tmp = format!("{path}.tmp.{}", std::process::id());
    let Ok(body) = serde_json::to_string_pretty(v) else { return false };
    if std::fs::write(&tmp, body).is_err() {
        return false;
    }
    let ok = std::fs::rename(&tmp, &path).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
    ok
}

/// Спільне місце — реєстр на сервері.
///
/// Бот пише його сам, сервер читає звідти й розсилає свої алерти тим самим
/// людям, що й команди бота. Токен синхронізації — окремий від пароля адміна:
/// у бота й сервера різні паролі, і ділитися адмінським з ботом не треба.
async fn push_registry(users: &Value) -> bool {
    let c = cfg();
    if c.sync_token.is_empty() {
        return false;
    }
    let url = format!("http://{}:{}/api/telegram-users", c.api_host, c.port);
    let Ok(body) = serde_json::to_string(users) else { return false };
    match http()
        .post(&url)
        .header("x-telegram-sync-token", &c.sync_token)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => true,
        Ok(r) => {
            eprintln!("[botd] сервер не прийняв реєстр: HTTP {}", r.status().as_u16());
            false
        }
        Err(e) => {
            eprintln!("[botd] реєстр не долетів до сервера: {e}");
            false
        }
    }
}

/// Цикл синхронізації реєстру — ОКРЕМИМ від опитування Telegram.
///
/// Якщо зробити push усередині опитування, то зникнення Telegram (або помилка
/// опитування) зупиняло б і синхронізацію — а реєстр має бути актуальним
/// саме тоді, коли бот мовчить. Крім того, «остання активність» підтягується
/// сюди раз на хвилину, тож список підключених не застаріває.
async fn registry_sync_loop() {
    loop {
        let users = load_users();
        if !users_list(&users).is_empty() {
            if push_registry(&users).await {
                eprintln!("[botd] реєстр синхронізовано із сервером");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// Сигнали стрічки, відсортовані за часом.
fn feed_items() -> Vec<Value> {
    let r = &cfg().root;
    let Some(feed) = read_json(&format!("{r}/dataset/alpha-feed.json")) else {
        return vec![];
    };
    let empty = vec![];
    let mut items: Vec<Value> = feed.as_array().unwrap_or(&empty).clone();
    items.sort_by_key(|i| i["ts"].as_u64().unwrap_or(0));
    items
}

/// Що надіслати користувачеві, у якого курсор `cursor`.
///
/// Повертає (новий курсор, сигнали до надсилання). Курсор `0` — це «цей
/// користувач ще нічого не отримував»: тоді стартуємо від найновішого сигналу
/// й нічого не шлемо, інакше перше ж доставлення залило б його історією.
fn signals_for(items: &[Value], cursor: u64) -> (u64, Vec<Value>) {
    let newest = items.last().map(|i| i["ts"].as_u64().unwrap_or(0)).unwrap_or(0);
    if cursor == 0 {
        return (newest, vec![]);
    }
    let pending: Vec<Value> = items
        .iter()
        .filter(|i| i["ts"].as_u64().unwrap_or(0) > cursor)
        .cloned()
        .collect();
    let upto: Vec<Value> = pending.into_iter().take(10).collect();
    let new_cursor = upto
        .last()
        .map(|i| i["ts"].as_u64().unwrap_or(0))
        .unwrap_or(cursor);
    (new_cursor, upto)
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

/// Авто-доставка нових сигналів — КОЖНОМУ користувачеві те, чого він ще не бачив.
///
/// Повертає кількість надісланих повідомлень. Користувачі з тишею не отримують
/// нічого; для інших сигнал не дублюється.
async fn broadcast_signals_once() -> usize {
    let items = feed_items();
    if items.is_empty() {
        return 0;
    }
    let users = load_users();
    let mut cursors = load_cursors();
    let mut sent = 0usize;
    let mut changed = false;
    for u in users_list(&users) {
        let chat = u["chat"].as_str().unwrap_or("").to_string();
        if chat.is_empty() || u["muted"].as_bool().unwrap_or(false) {
            continue;
        }
        let cursor = cursors.get(&chat).and_then(|v| v.as_u64()).unwrap_or(0);
        let (new_cursor, pending) = signals_for(&items, cursor);
        if cursor == 0 {
            // Перший контакт: курсор стає найновішим сигналом, історію не лишемо.
            cursors[&chat] = json!(new_cursor);
            changed = true;
            continue;
        }
        for it in pending {
            let ts = it["ts"].as_u64().unwrap_or(0);
            let text = format!(
                "{}\n\n{}",
                signal_text(&it),
                "Дані проєкту Wallet Radar · /сигнали — історія"
            );
            if !send_to(&chat, &text).await {
                // Нікому не дійшло — не рухаємо курсор, спробуємо наступного разу.
                break;
            }
            cursors[&chat] = json!(ts);
            changed = true;
            sent += 1;
        }
    }
    // Курсори зберігаємо, навіть якщо нічого не надіслано: «перший контакт»
    // теж рухає курсор, і без цього авто-доставка не почалася б узагалі.
    if changed {
        let _ = save_cursors(&cursors);
    }
    sent
}

/// Окремий цикл автодоставки — НЕ від запиту користувача.
///
/// Раніше розсилка запускалася з обробки будь-якого повідомлення: тож тестове
/// повідомлення одного користувача спричиняло сповіщення в усіх інших. Тепер
/// автодоставка живе окремо: ніхто своїм повідомленням нікого не «розбуджує».
async fn signal_broadcast_loop() {
    loop {
        let n = broadcast_signals_once().await;
        if n > 0 {
            eprintln!("[botd] розіслано сигналів: {n}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(SIGNAL_SECS)).await;
    }
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

/// Відмова, що відповідає на запит про нашу внутрішню базу гаманців.
///
/// Тут важлива не сама відмова, а те, що вона не пропонує обхід: глядач не має
/// жодного шляху дістати склад груп чи дані про адреси — ні командою, ні
/// асистентом, ні через модель з інструментами.
fn denied_data(what: &str) -> String {
    format!(
        "{what} — це внутрішня база проєкту, глядачам вона недоступна. \
         Доступні лише транзакції та сигнали: /сигнали"
    )
}

/// Розбір команди: перше слово — команда, решта — аргументи.
///
/// Окремий випадок — «/в правах <chat_id>»: у назві команди є пробіл, тож наївно
/// розбір віддає «/в» і «правах …», а назва з пробілом ніколи не збігається —
/// команда мовчки тонула у запиті до асистента. Тут це розбирається явно.
fn parse_cmd(t: &str) -> (String, String) {
    let t = t.trim();
    let (head, rest) = match t.split_once(char::is_whitespace) {
        Some((h, r)) => (h.to_lowercase(), r.trim().to_string()),
        None => (t.to_lowercase(), String::new()),
    };
    if head == "/в" {
        let lower = rest.to_lowercase();
        if lower.starts_with("правах") {
            // skip(6) — саме символи, а не байти: «ПРАВАХ» у кодуванні UTF-8 це
            // 12 байт, і зрізання по байтах могло б розрубати символ.
            let tail = rest.chars().skip(6).collect::<String>().trim().to_string();
            return ("/в правах".to_string(), tail);
        }
    }
    (head, rest)
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

    let (head, rest) = parse_cmd(t);
    let is_admin = caller.role == Role::Admin;

    match head.as_str() {
        "/start" => greet(caller.role),
        "/help" | "/довідка" | "/допомога" => help(caller.role),
        "/стан" | "/status" => status(),
        // Наша база гаманців — власна внутрішня база проєкту. Глядач її не бачить.
        "/гаманець" | "/wallet" => {
            if is_admin { wallet(&rest) } else { denied_data("дані про гаманця") }
        }
        "/токен" | "/token" => token_info(&rest),
        "/група" | "/group" => {
            if is_admin { group(&rest) } else { denied_data("склад груп капіталу") }
        }
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
        "/тиша" | "/mute" => mute_cmd(caller).await,
        "/користувачі" | "/users" => {
            if is_admin { users_cmd() } else { denied("список користувачів") }
        }
        "/відключити" | "/revoke" => {
            if is_admin { revoke_cmd(&rest).await } else { denied("відключення користувачів") }
        }
        "/права" | "/promote" => {
            if is_admin { role_cmd(&rest, true).await } else { denied("зміна прав") }
        }
        "/в правах" | "/вправах" | "/demote" => {
            if is_admin { role_cmd(&rest, false).await } else { denied("зміна прав") }
        }
        _ => ask(t, caller.role).await,
    }
}

/// Привітання після підключення. Роль видно одразу — щоб не гадати, що тобі доступно.
fn greet(role: Role) -> String {
    let extra = match role {
        Role::Admin => "\nТи — власник: доступні довільні команди, запуск бінів і керування користувачами.",
        Role::Viewer => "\nТи підключений як глядач: транзакції, сигнали, ринок і стан моніторів.",
    };
    format!("<b>Wallet Radar — бот</b>\nПідключено.{extra}\n\n{}", help(role))
}

/// Довідка. Для глядача приховуємо і привілейовані команди, і ті, що читають
/// нашу внутрішню базу гаманців — щоб не було спокуси натискати те, що все одно
/// відхилиться.
fn help(role: Role) -> String {
    if role != Role::Admin {
        return "<b>Wallet Radar — бот</b>\n\n\
     <b>Дані</b>\n\
     /токен ONDO — ціна, ліквідність, обіг, капіталізація\n\
     /сигнали [N] — останні сигнали проєкту (за замовчуванням 5)\n\
     /стан — коли востаннє відпрацював кожен монітор\n\
     /діагностика — перевірка всіх ланок відповіді\n\
     /хтоя — мій чат, роль і ліміт\n\n\
     <b>Питання</b>\n\
     Напиши питання без слеша — відповім по ринку, моніторах і сигналах.\n\n\
     /тиша — не присилати тобі сигнали й розсилки (лишаєшся підключеним)\n\n\
     Внутрішня база гаманців глядачам недоступна.".to_string();
    }
    "<b>Wallet Radar — бот</b>\n\n\
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
     /тиша — не присилати тобі сигнали й розсилки (лишаєшся підключеним)\n\n\
     <b>Дії (лише власник)</b>\n\
     /рассылка <текст> — розіслати повідомлення всім підключеним\n\
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
     Запускати можна: cexsignal, campwatch, wakeup, tokprices, tokliq, relay-monitor".to_string()
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
async fn mute_cmd(caller: &Caller) -> String {
    let mut users = load_users();
    let Some(rec) = users["users"].get(&caller.chat).cloned() else {
        return "ти не підключений — напиши /start.".to_string();
    };
    let muted = !rec["muted"].as_bool().unwrap_or(false);
    users["users"][&caller.chat]["muted"] = json!(muted);
    if save_users(&users) {
        let _ = push_registry(&users).await;
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
async fn revoke_cmd(arg: &str) -> String {
    let chat = arg.trim();
    if chat.is_empty() {
        return "кого відключити? /відключити <chat_id>".to_string();
    }
    let mut users = load_users();
    if remove_user(&mut users, chat) {
        if save_users(&users) {
            let _ = push_registry(&users).await;
            format!("користувача {chat} відключено.")
        } else {
            "не зміг зберегти реєстр — спробуй ще раз.".to_string()
        }
    } else {
        format!("користувача {chat} у реєстрі немає.")
    }
}

/// Надати або забрати права адміна.
async fn role_cmd(arg: &str, admin: bool) -> String {
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
        let _ = push_registry(&users).await;
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

/// Одна група капіталу в нормалізованому вигляді.
struct Grp {
    /// Ідентифікатор, за яким групу шукають: «/група 3».
    id: String,
    /// Людська назва: спільні активи групи або її стара назва.
    label: String,
    /// Адреси учасників.
    members: Vec<String>,
}

/// Групи капіталу — у нормалізованому вигляді.
///
/// Формат даних змінювався, і бот має розуміти обидва:
/// - старий — `dataset/capital-groups/wallets.json`: словник «назва → масив адрес»;
/// - новий — `dataset/capital-groups.json`: список груп, де в кожній є учасники
///   зі свіжими сумами та спільні активи.
///
/// Без цього команда `/група` мовчки не працювала б на копії, де лежить лише
/// новіший файл (або навпаки).
fn load_groups(r: &str) -> Vec<Grp> {
    if let Some(v) = read_json(&format!("{r}/dataset/capital-groups/wallets.json")) {
        if let Some(obj) = v.as_object() {
            return obj
                .iter()
                .map(|(k, val)| Grp {
                    id: k.clone(),
                    label: k.clone(),
                    members: val
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| {
                                    x.as_str()
                                        .map(|s| s.to_string())
                                        .or_else(|| x["address"].as_str().map(|s| s.to_string()))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect();
        }
    }

    let Some(v) = read_json(&format!("{r}/dataset/capital-groups.json")) else {
        return vec![];
    };
    let Some(list) = v["groups"].as_array() else {
        return vec![];
    };
    list.iter()
        .enumerate()
        .map(|(i, g)| {
            let assets: Vec<String> = g["commonAssets"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let members: Vec<String> = g["members"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x["address"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let label = if assets.is_empty() {
                format!("група {}", i + 1)
            } else {
                assets.join(", ")
            };
            Grp { id: (i + 1).to_string(), label, members }
        })
        .collect()
}

fn group(arg: &str) -> String {
    let q = arg.trim().to_lowercase();
    let r = &cfg().root;
    let groups = load_groups(r);
    if groups.is_empty() {
        return "груп капіталу в даних нема".to_string();
    }

    if q.is_empty() {
        let mut out = String::from("<b>Групи капіталу</b>\n");
        let mut rows: Vec<&Grp> = groups.iter().collect();
        rows.sort_by(|a, b| b.members.len().cmp(&a.members.len()));
        for g in rows.iter().take(40) {
            out.push_str(&format!(
                "• {} <b>{}</b> — {} гаманців\n",
                esc(&g.id),
                esc(&g.label),
                g.members.len()
            ));
        }
        if rows.len() > 40 {
            out.push_str(&format!("…і ще {}\n", rows.len() - 40));
        }
        return out;
    }

    match groups.iter().find(|g| g.id == q || g.label.to_lowercase().contains(&q)) {
        Some(g) => {
            let mut out = format!(
                "<b>Група {}</b>\n{} — гаманців: {}\n\nПерші 10:\n",
                esc(&g.id),
                esc(&g.label),
                g.members.len()
            );
            for w in g.members.iter().take(10) {
                out.push_str(&format!(
                    "• <a href=\"https://etherscan.io/address/{w}\">{}</a>\n",
                    esc(&short(w))
                ));
            }
            out
        }
        None => format!(
            "групи «{}» нема. /група без аргументу — список усіх",
            esc(&q)
        ),
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

    // асистент — від твоєї ролі, бо саме так він викликається в роботі.
    // Адреса сервера — не завжди localhost (на Windows-машині сервер на ноутбуку),
    // тож беремо її з API_HOST, як в решті місць.
    let url = format!("http://{}:{}/api/ask?q=%D1%82%D0%B5%D1%81%D1%82", c.api_host, c.port);
    let (login, pass) = api_creds(caller.role);
    let a = match http().get(&url).basic_auth(login, Some(pass)).send().await {
        Ok(r) if r.status().is_success() => "✅ відповідає".to_string(),
        Ok(r) => format!("⚠️ HTTP {}", r.status().as_u16()),
        Err(e) => format!("❌ {}", e),
    };
    out.push_str(&format!("асистент /api/ask ({login}): {}\n", esc(&a)));

    // Локальний витяг із даних — працює без асистента й без моделі.
    out.push_str(&format!(
        "питання без асистента: {} (гаманець, токен, групи, монітори, сигнали)\n",
        if a.starts_with("\u{2705}") { "\u{2705} \u{0447}\u{0438}\u{0442}\u{0430}\u{044e}\u{0442}\u{044c}\u{0441}\u{044f} \u{0437} \u{0434}\u{0430}\u{043d}\u{0438}\u{0445}" } else { "\u{26a0}\u{fe0f} \u{043d}\u{0435} \u{0432}\u{0438}\u{0437}\u{043d}\u{0430}\u{0447}\u{0435}\u{043d}\u{043e}" }
    ));

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
    let t = tool_exec("monitors_status", &json!({}), caller.role);
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
    if role == Role::Admin {
        // 1. Детермінований асистент сервера — лише власнику: він уміє піднімати
        //    дані про наші групи й гаманці, а це внутрішня база проєкту.
        if let Some(a) = ask_assistant(q, role).await { return a; }
        // 2. Модель З ДОСТУПОМ ДО ДАНИХ: сама вирішує, що подивитись, ми виконуємо.
        if let Some(a) = ask_ollama_tools(q, role).await { return a; }
        // 3. Те саме, але БЕЗ моделі: питання про конкретний гаманець, токен, групу,
        //    монітори чи сигнали читається з тих самих файлів, які бот уже читає.
        if let Some(a) = ask_local(q, role) { return a; }
        // 4. Модель без даних — останній засіб, і він чесно помічений.
        return match ask_ollama(q).await {
            Some(a) => format!("{}\n\n<i>⚠️ модель відповідала БЕЗ доступу до даних проєкту — числа з неї не бери</i>", esc(&a)),
            None => "не зрозумів питання, і модель не відповіла. /довідка — що я вмію".to_string(),
        };
    }

    // Глядач: тільки транзакції (сигнали/алерти), ринок і стан моніторів.
    // Жодного доступу до нашої бази гаманців — ні через команди, ні через асистента,
    // ні через «розумну» модель, яка сама спитала б інструмент.
    if let Some(a) = ask_local(q, role) { return a; }
    "На питання про нашу базу гаманців я не відповідаю: глядачам доступні лише транзакції та сигнали (/сигнали). Запитайте власника.".to_string()
}

/// Питання без асистента й без моделі — читається з локальних даних.
///
/// Не всі питання потребують моделі: «хто такий 0x…», «яка ціна ONDO»,
/// «які є групи», «що з моніторами» — це пошук у тих самих файлах, які бот уже
/// читає для команд `/гаманець`, `/токен`, `/група`, `/стан`, `/сигнали`.
/// Тому навіть без `/api/ask` і без локальної моделі бот лишається корисним,
/// а не відповідає «не зрозумів питання» наохочу.
fn ask_local(q: &str, role: Role) -> Option<String> {
    let raw = q.trim();
    if raw.is_empty() { return None; }
    let low = raw.to_lowercase();

    // гаманець — за повною адресою: це наша внутрішня база, лише власнику.
    if role == Role::Admin {
        if let Some(addr) = find_address(raw) {
            return Some(strip_tags(&wallet(&addr)));
        }
    }

    // токен — ринкові дані, вони публічні й нічого про наші гаманці не кажуть.
    if let Some(sym) = find_ticker(raw) {
        if let Some(ans) = token_found(&sym) {
            return Some(strip_tags(&ans));
        }
    }

    // групи капіталу — це склад НАШИХ гаманців: лише власник.
    if role == Role::Admin && has_word(&low, &["групи", "група", "груп"]) {
        return Some(strip_tags(&group("")));
    }
    // монітори — стан роботи, без нашої бази.
    if has_word(&low, &["монітор", "монітори", "стан", "працює", "живі", "застряг"]) {
        return Some(strip_tags(&status()));
    }
    // сигнали — транзакції: ось що глядач має бачити.
    if has_word(&low, &["сигнал", "сигнали", "алерт", "транзакц"]) {
        return Some(strip_tags(&signals("5")));
    }
    None
}

/// Повна адреса 0x… у тексті.
fn find_address(s: &str) -> Option<String> {
    let l = s.to_lowercase();
    // Идемо по МЕЖАХ СИМВОЛІВ, а не по байтах: кирилиця в UTF-8 — це два байти,
    // і зрізання посередині символу кинуло б панік (byte index not a char boundary).
    for (i, _) in l.char_indices() {
        let tail = &l[i..];
        if tail.len() < 42 { continue; }
        if !tail.starts_with("0x") { continue; }
        if tail.as_bytes()[2..42].iter().all(|b| b.is_ascii_hexdigit()) {
            return Some(tail[0..42].to_string());
        }
    }
    None
}

/// Символ токена: слово з 2..=10 великих літер/цифр. Тільки у ВЕРХНЬОМУ регістрі
/// — інакше «яка ціна ondo» та «зроби ondo» підходили б під «ONDO».
fn find_ticker(s: &str) -> Option<String> {
    for w in s.split(|c: char| !c.is_alphanumeric()) {
        if w.len() >= 2 && w.len() <= 10 && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && w.chars().any(|c| c.is_ascii_uppercase()) {
            return Some(w.to_string());
        }
    }
    None
}

/// Чи є у тексті хоч одне зі слів ( збіг по межах слова).
fn has_word(low: &str, words: &[&str]) -> bool {
    words.iter().any(|w| {
        low.split(|c: char| !c.is_alphanumeric())
            .any(|t| t == *w || (w.len() > 3 && t.starts_with(w)))
    })
}

/// Токен знайдено в ринкових даних? `None` — такого токена в даних немає.
fn token_found(sym: &str) -> Option<String> {
    let r = &cfg().root;
    let q = sym.to_lowercase();
    let liq = read_json(&format!("{r}/dataset/backfill/token-liquidity.json"))?;
    let obj = liq["tokens"].as_object()?;
    for (k, v) in obj {
        let s = v["symbol"].as_str().unwrap_or("").to_lowercase();
        if k.to_lowercase() == q || s == q {
            return Some(token_info(sym));
        }
    }
    None
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
/// Повний набір інструментів — тільки для власника.
fn all_tool_defs() -> Value {
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

/// Інструменти, доступні моделі. Для глядача — БЕЗ тих, що читають нашу базу
/// гаманців: ролі, баланси, групи, пошук за мітками.
fn tool_defs(role: Role) -> Value {
    if role == Role::Admin {
        return all_tool_defs();
    }
    let arr = all_tool_defs().as_array().cloned().unwrap_or_default();
    Value::Array(
        arr.into_iter()
            .filter(|t| {
                let n = t["function"]["name"].as_str().unwrap_or("");
                !matches!(n, "wallet_info" | "group_info" | "find_by_label")
            })
            .collect(),
    )
}

/// Виконання інструмента. Повертає ПЛОСКИЙ текст — те саме, що побачив би користувач,
/// лише без розмітки.
fn tool_exec(name: &str, args: &Value, role: Role) -> String {
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    // Глядач не дістає нашу базу гаманців — навіть якщо модель сама попросила
    // такий інструмент. Обмеження на рівні інструмента, а не лише на рівні списку.
    if role != Role::Admin && matches!(name, "wallet_info" | "group_info" | "find_by_label") {
        return "інструмент доступний лише власнику".to_string();
    }
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
async fn ollama_chat(messages: &[Value], with_tools: bool, role: Role) -> Option<Value> {
    let mut body = json!({
        "model": cfg().model,
        "messages": messages,
        "stream": false,
        // нижча температура: нам потрібні факти з інструментів, а не творчість
        "options": {"temperature": 0.2},
    });
    if with_tools { body["tools"] = tool_defs(role); }
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
async fn ask_ollama_tools(q: &str, role: Role) -> Option<String> {
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
        let msg = ollama_chat(&messages, true, role).await?;
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
            let result = tool_exec(&name, &args, role);
            eprintln!("[botd] інструмент {name}({args}) → {} символів", result.len());
            used.push(name.clone());
            messages.push(json!({"role": "tool", "name": name, "content": result}));
        }
    }
    // Витратили всі кола на інструменти — просимо підсумувати вже без них
    let msg = ollama_chat(&messages, false, role).await?;
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
    // Адреса сервера — не завжди localhost: на Windows-машині сервер Wallet Radar
    // живе на ноутбукці, тож запит інакше пішов би в порожнечу.
    let url = format!("http://{}:{}/api/ask?q={}", c.api_host, c.port, urlencode(q));
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
                    // Одразу кажемо серверу: новий підключений має отримувати алерти.
                    let _ = push_registry(&users).await;
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
    // Тут НІКОЛИ не розсилається нічого іншим: повідомлення користувача —
    // це його особиста справа, відповідь іде тільки йому. Автодоставка
    // сигналів живе окремим циклом (signal_broadcast_loop), щоб тестове
    // повідомлення не «розбуджувало» решту.
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

    // Реєстр живе у спільному місці — на сервері. Синхронізація окремим циклом,
    // щоб не залежала від того, чи вдалося опитування Telegram.
    tokio::spawn(registry_sync_loop());

    // Автодоставка сигналів — окремим циклом, щоб не привʼязуватися до
    // повідомлень: користувач пише — і лише йому відповідають.
    tokio::spawn(signal_broadcast_loop());

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

    #[test]
    fn first_run_does_not_flood_with_history() {
        let _guard = registry_lock();
        ensure_root();
        let dir = std::env::var("APP_ROOT").unwrap();
        std::fs::create_dir_all(format!("{dir}/dataset")).unwrap();
        let feed = json!([
            {"address": "0xaa", "tokens": ["AAA"], "ts": 1000},
            {"address": "0xbb", "tokens": ["BBB"], "ts": 2000}
        ]);
        std::fs::write(format!("{dir}/dataset/alpha-feed.json"), feed.to_string()).unwrap();
        let items = feed_items();

        // Перший контакт: історія НЕ надсилається, лише фіксується курсор.
        let (cursor, pending) = signals_for(&items, 0);
        assert!(pending.is_empty(), "перший запуск не має лити історію");
        assert_eq!(cursor, 2000, "курсор стає найновішим сигналом");

        // Новий сигнал — ось тепер має дійти.
        let feed = json!([
            {"address": "0xaa", "tokens": ["AAA"], "ts": 1000},
            {"address": "0xbb", "tokens": ["BBB"], "ts": 2000},
            {"address": "0xcc", "tokens": ["CCC"], "ts": 3000}
        ]);
        std::fs::write(format!("{dir}/dataset/alpha-feed.json"), feed.to_string()).unwrap();
        let items = feed_items();
        let (cursor, pending) = signals_for(&items, 2000);
        assert_eq!(pending.len(), 1, "лише новий сигнал");
        assert_eq!(pending[0]["ts"].as_u64(), Some(3000));
        assert_eq!(cursor, 3000);
    }

    #[test]
    fn every_user_has_own_cursor() {
        // Черга кожного — своя: активність одного не просуває курсор інших.
        let items = vec![
            json!({"address": "0xaa", "tokens": ["AAA"], "ts": 1000}),
            json!({"address": "0xbb", "tokens": ["BBB"], "ts": 2000}),
            json!({"address": "0xcc", "tokens": ["CCC"], "ts": 3000}),
        ];
        // «А» бачив усе до 2000, «Б» — тільки до 1000.
        let (_ca, pa) = signals_for(&items, 2000);
        let (_cb, pb) = signals_for(&items, 1000);
        assert_eq!(pa.len(), 1);
        assert_eq!(pa[0]["ts"].as_u64(), Some(3000));
        assert_eq!(pb.len(), 2, "«Б» отримує і те, що «А» вже бачив");
        assert_eq!(pb[0]["ts"].as_u64(), Some(2000));

        // Новий користувач (курсор 0) не отримує нічого — історію не литимо.
        let (_cc, pc) = signals_for(&items, 0);
        assert!(pc.is_empty());
    }

    #[test]
    fn one_signal_is_not_delivered_twice() {
        let items = vec![
            json!({"address": "0xaa", "tokens": ["AAA"], "ts": 1000}),
            json!({"address": "0xbb", "tokens": ["BBB"], "ts": 2000}),
        ];
        let (cursor, pending) = signals_for(&items, 1000);
        assert_eq!(pending.len(), 1);
        // Наступного разу той самий сигнал уже не дублюється.
        let (_cursor2, pending2) = signals_for(&items, cursor);
        assert!(pending2.is_empty());
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

    #[test]
    fn command_whose_name_has_a_space_is_still_parsed() {
        // "/в правах 123" — у назві команди є пробіл. Наївний розбір за першим
        // пробілом дав би голову "/в" і аргумент "правах 123", тож назва з
        // пробілом ніколи не збігалася б — команда мовчки тонула у запиті
        // до асистента.
        let (head, rest) = parse_cmd("/\u{0432} \u{043f}\u{0440}\u{0430}\u{0432}\u{0430}\u{0445} 123");
        assert_eq!(head, "/\u{0432} \u{043f}\u{0440}\u{0430}\u{0432}\u{0430}\u{0445}");
        assert_eq!(rest, "123");

        // альтернативне написання без пробілу
        let (head, rest) = parse_cmd("/\u{0432}\u{043f}\u{0440}\u{0430}\u{0432}\u{0430}\u{0445} 456");
        assert_eq!(head, "/\u{0432}\u{043f}\u{0440}\u{0430}\u{0432}\u{0430}\u{0445}");
        assert_eq!(rest, "456");

        // звичайні команди не ламаються
        let (head, rest) = parse_cmd("/\u{0433}\u{0430}\u{043c}\u{0430}\u{043d}\u{0435}\u{0446}\u{044c} 0x1234");
        assert_eq!(head, "/\u{0433}\u{0430}\u{043c}\u{0430}\u{043d}\u{0435}\u{0446}\u{044c}");
        assert_eq!(rest, "0x1234");

        // команда без аргументів
        let (head, rest) = parse_cmd("/\u{0441}\u{0442}\u{0430}\u{0442}");
        assert_eq!(head, "/\u{0441}\u{0442}\u{0430}\u{0442}");
        assert_eq!(rest, "");
    }

    #[test]
    fn local_fallback_finds_address_and_ticker() {
        assert_eq!(
            find_address("who is 0x4524d2f91f71ee172fcc1fb534b84ad4b2a63f7c?").unwrap(),
            "0x4524d2f91f71ee172fcc1fb534b84ad4b2a63f7c"
        );
        assert_eq!(find_ticker("price of ONDO now").unwrap(), "ONDO");
        // lower case is not a ticker: "ondo" is a word, not a symbol
        assert!(find_ticker("price of ondo now").is_none());
        assert!(find_address("short 0xdeadbeef").is_none());
        // a multi-byte char BEFORE the address: byte-wise slicing used to panic
        // with "start byte index is not a char boundary"
        assert_eq!(
            find_address("\u{0447}\u{0442}\u{043e} 0x4524d2f91f71ee172fcc1fb534b84ad4b2a63f7c").unwrap(),
            "0x4524d2f91f71ee172fcc1fb534b84ad4b2a63f7c"
        );
    }

    #[test]
    #[ignore = "needs a live APP_ROOT with real data"]
    fn local_fallback_answers_from_live_data() {
        std::env::set_var("APP_ROOT", "/opt/wallet-tracker");
        let _ = cfg();
        for q in ["\u{044f}\u{043a}\u{0430} \u{0446}\u{0456}\u{043d}\u{0430} ONDO",
                  "\u{0445}\u{0442}\u{043e} \u{0442}\u{0430}\u{043a}\u{0438}\u{0439} 0x4524d2f91f71ee172fcc1fb534b84ad4b2a63f7c",
                  "\u{044f}\u{043a}\u{0456} \u{0454} \u{0433}\u{0440}\u{0443}\u{043f}\u{0438} \u{0454}",
                  "\u{044f}\u{043a}\u{0456} \u{0441}\u{0442}\u{0430}\u{043d} \u{043c}\u{043e}\u{043d}\u{0456}\u{0442}\u{043e}\u{0440}\u{0456}\u{0432}"] {
            let a = ask_local(q, Role::Admin);
            println!("Q={q:?} -> {}", a.as_deref().unwrap_or("<none>").chars().take(90).collect::<String>());
            assert!(a.is_some(), "no local answer for {q:?}");
        }
    }

    // ───────────────── ролі: глядач не бачить нашу базу гаманців ─────────────────

    #[test]
    fn viewer_tool_list_excludes_our_base() {
        let names: Vec<String> = tool_defs(Role::Viewer).as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|f| f["function"]["name"].as_str().unwrap_or("").to_string())
            .collect();
        for closed in ["wallet_info", "group_info", "find_by_label"] {
            assert!(!names.iter().any(|n| n == closed), "{closed} мусить бути закритий для глядача");
        }
        // а от ринкове й монітори лишаються
        for open in ["token_info", "monitors_status", "cex_flows"] {
            assert!(names.iter().any(|n| n == open), "{open} має лишатися глядачеві");
        }
        // власнику — увесь набір
        assert_eq!(tool_defs(Role::Admin).as_array().unwrap().len(), 6);
    }

    #[test]
    fn viewer_cannot_execute_our_base_tools() {
        // навіть якщо модель сама попросила закритий інструмент
        let r = tool_exec("wallet_info", &json!({"address": "0x4524d2f91f71ee172fcc1fb534b84ad4b2a63f7c"}), Role::Viewer);
        assert!(r.contains("лише власнику"), "wallet_info для глядача: {r}");
        let r = tool_exec("group_info", &json!({"name": ""}), Role::Viewer);
        assert!(r.contains("лише власнику"), "group_info для глядача: {r}");
        let r = tool_exec("find_by_label", &json!({"query": "binance"}), Role::Viewer);
        assert!(r.contains("лише власнику"), "find_by_label для глядача: {r}");
    }

    #[test]
    fn viewer_gets_no_wallet_or_group_data_in_answers() {
        // питання про гаманця: глядач не отримує картку гаманця
        let addr = "0x4524d2f91f71ee172fcc1fb534b84ad4b2a63f7c";
        assert!(ask_local(&format!("хто такий {addr}"), Role::Viewer).is_none(),
            "глядач не мав отримати дані про гаманця");
        // питання про групи: теж ні
        assert!(ask_local("які є групи", Role::Viewer).is_none(),
            "глядач не мав отримати склад груп");
        // а власник — отримує
        assert!(ask_local(&format!("хто такий {addr}"), Role::Admin).is_some());
    }

    #[test]
    fn viewer_still_gets_market_and_signals() {
        // глядач мусить лишатись корисним: ринок, монітори, сигнали — можна
        let s = ask_local("останні сигнали", Role::Viewer);
        assert!(s.is_some(), "глядач мав отримати сигнали");
    }

    #[test]
    fn denial_message_names_what_is_closed() {
        let d = denied_data("дані про гаманця");
        assert!(d.contains("внутрішня база"));
        assert!(d.contains("/сигнали"), "відмова має показати доступну заміну");
    }
}
