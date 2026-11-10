# Wallet Radar Alerts Bot — архітектура

Окремий продукт. Окремий репозиторій. Wallet Radar — **зовнішній read-only сервіс**
(HTTP + WS, Basic-auth), у його код ми не втручаємося.

Три заповіді, які визначили всю конструкцію:

1. **Алерт — усім, відповідь — лише автору.** Розсилка й приватна відповідь —
   різні конвеєри з різними гарантіями доставки.
2. **Гаманці й транзакції не існують у продукті.** Не «сховати», не
   «замазати в останній момент» — їх немає в типах, якими бот формує текст.
3. **Джерело може зникнути.** Бот має лишатися живим і чесним, а не мовчати
   й не брехати про свіжість даних.

---

## 0. Відповіді на сім питань (коротко, деталі — далі)

| # | Питання | Рішення |
|---|---|---|
| 1 | Як отримувати алерти | **Гібрид із розподілом ролей**: WS — тригер (сирі події, секунди), REST-поллінг — контекст і зріз стану (хвилинні вікна). Алерт = подія з WS + контекст з REST. |
| 2 | «Усім — алерт, автору — відповідь» | Ключ доставки — `chat_id`; особистість — `user_id`. Бот приймає лише приватні чати (`type=private`); у групі/каналі — не відповідає взагалі. Відповідь йде в chat джерела з `reply_to_message_id`. |
| 3 | Заборона гаманців/tx | **Три шари**: типовий (у типах бота немає поля для адреси) → whitelist полів на вході (denylist `address/txHash/counterparty/contract/root`) → регуляркова санітазація тексту на виході + golden-тест «нуль адрес у виході». |
| 4 | Масштабування розсилки | Durable-черга в БД (`deliveries`), глобальний token bucket 25 msg/s, per-chat ≥1.05 с, три пріоритети черги, 429 → `retry_after` без дропу. |
| 5 | Дайджести без розкритчу чужих підписок | Агрегат **один і той самий** для всіх; різниця — лише фільтр за власними підписками. Жодних лічильників «скільки ще хто підписано» у відповідях. |
| 6 | Ідемпотентність | Три рівні: `seen_events(event_id)` → `alert_log.alert_key` (UNIQUE) → `deliveries(chat_id, alert_key)` (PK). Dedup key алерта = `kind+asset+window+hash(evidence)`, **не** `event_id+user_id`. |
| 7 | Падіння Wallet Radar | Режими `healthy → degraded → stale` за лагом і доступністю; кеш останнього зрізу в БД; алерти буферизуються до 30 хв і не дублюються при відновленні; відповіді позначаються віком даних. |

---

## 1. Карта модулів

```
                 ┌──────────────────────┐
  Telegram  ───► │  telegram-gateway    │  long-polling / webhook
                 │  update → IncomingUpdate│  dedup за update_id
                 └──────────┬───────────┘
                            │
              ┌─────────────┴──────────────┐
              │                            │
   ┌──────────▼──────────┐      ┌──────────▼──────────┐
   │  user-session       │      │   command-router    │  /start /assets /groups
   │  ізольований стан   │      │   admin-команди     │  /broadcast /stats /users
   └──────────┬──────────┘      └──────────┬──────────┘
              │                            │
              │                    ┌───────▼────────┐
              │                    │ privacy-filter │  ← обов'язковий перед будь-яким
              │                    │ scrub()        │    виходом у Telegram
              │                    └───────┬────────┘
              │                            │
   ┌──────────▼────────────────────────────▼──────────┐
   │                  broadcast                         │  durable queue,
   │  алерти → усім; idempotency; retry; rate limit   │  per-user delivery
   └──────────┬────────────────────────────────────────┘
              │  Alert (тип без адрес)
   ┌──────────▼──────────┐        ┌───────────────────┐
   │  alert-engine       │◄───────┤  wr-client        │──► Wallet Radar
   │  події → алерти     │  ctx   │  REST + WS        │    (read-only)
   └─────────────────────┘        └───────────────────┘
              │
   ┌──────────▼──────────┐        ┌───────────────────┐
   │  storage (trait)    │───────►│ SQLite / Postgres │
   │  users, deliveries, │        └───────────────────┘
   │  alert_log, audit   │
   └─────────────────────┘
```

Залежності йдуть **вниз** лише. `privacy-filter` не знає ні про Telegram, ні про
Wallet Radar — він приймає `&str` і повертає `ScrubReport`. `alert-engine` не
знає про Telegram. Це і є те, що робить кожен шар тестованим окремо.

### Крейти воркспейсу

| Крейт | Відповідає за | Не містить |
|---|---|---|
| `privacy-filter` | регулярки, allowlist, scrub | нічого доменного |
| `storage` | трейт `Store` + `SqliteStore`/`PostgresStore` | HTTP, Telegram |
| `wr-client` | HTTP/WS до Wallet Radar, реконнект | логіки алертів |
| `alert-engine` | події+контекст → `Alert` | відправки в Telegram |
| `broadcast` | черга, ідемпотентність, rate limit | форматування тексту |
| `user-session` | стан користувача, підписки, GDPR | знань про Wallet Radar |
| `telegram-gateway` | поллінг, роутинг, формати | бізнес-правил |
| `admin` | адмін-команди, аудит | загального стану |
| `bot` (bin) | оркестрація, конфіг, shutdown | логіки |

Крейти — щоб boundary «сирий JSON → типований санітизований домен» був
перевірюваним компілятором, а не домовляістю.

---

## 2. Контракти

### 2.1 Домен Wallet Radar (сире, але типоване)

```rust
// wr-client/src/raw.rs — живе ТІЛЬКИ всередині клієнта.
pub struct RawEvent {
    pub id: String,            // "0x…:eth:nat" — ідентифікатор події
    pub wallet_id: String,     // "wmrg3e7qb" — непрозорий хеш, не адреса
    pub network: Network,      // Eth | Bsc | Sol | Base | …
    pub flow: FlowKind,        // In | Out | Self
    pub asset: AssetRef,       // symbol + chain + contract (contract НЕ експортується)
    pub amount: f64,
    pub value_usd: f64,
    pub ts: i64,               // epoch ms
    pub tx_hash: Option<String>,
}
```

`AssetRef` містить `contract` — він потрібен клієнту для дедуплікації по
`(chain, contract, symbol)`, але **не має публічного шляху в домен**: тип
`SanitizedAsset` його не несе.

### 2.2 Санітизований домен (те, з чого формується текст)

```rust
// alert-engine/src/domain.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction { Accumulation, Distribution }

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    WhaleAccumulation,
    WhaleDistribution,
    PumpPhase,
    LiquidationSpike,
    CapitalGroupChange,   // нова група, злиття, зміна частки емісії
    CrossAssetAnomaly,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity { Info, Medium, High }

/// Агрегати. ПОЛЕЗЯ НЕМАЄ НІ ОДНОГО, ЯКЕ МОЖНА БУЛО Б ЗАПОВНИТИ АДРЕСОЮ.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Metrics {
    pub participants: u32,        // скільки учасників у вікні
    pub total_value_usd: f64,     // сума по вікну (агрегат, не транзакція)
    pub group_share_pct: Option<f64>,  // частка емісії групи
    pub concentration_top10_pct: Option<f64>,
    pub group_size: Option<u32>,  // кількість учасників групи
    pub window: Window,           // 15m | 1h | 24h
}

/// Готовий до рендеру алерт. Єдиний спосіб отримати текст — `render()`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Alert {
    pub key: AlertKey,            // idempotency
    pub kind: AlertKind,
    pub severity: Severity,
    pub asset: AssetLabel,        // symbol + name + chain — БЕЗ контракту
    pub direction: Option<Direction>,
    pub metrics: Metrics,
    pub archetype: Option<&'static str>,  // "accumulator", "distributor", …
    pub degraded: bool,           // джерело було неповне — показуємо позначку
    pub created_at: i64,
}
```

**Чому `Metrics` без адрес — це не «обережність», а конструкція.** Якщо
потрібні адреси для дедуплікації, вони живуть у `RawEvent` і вмирають у
клієнті. Тому `render()` фізично не може скласти речення «гаманець 0x… купив».

### 2.3 Трейти

```rust
// storage/src/lib.rs
pub trait Store {
    fn upsert_user(&self, u: NewUser) -> impl Future<Output = Result<User>>;
    fn get_user(&self, chat_id: i64) -> impl Future<Output = Result<Option<User>>>;
    fn set_settings(&self, chat_id: i64, s: &Settings) -> impl Future<Output = Result<()>>;
    fn subscriptions(&self, chat_id: i64) -> impl Future<Output = Result<Vec<Subscription>>>;
    fn subscribe(&self, chat_id: i64, sub: Subscription) -> impl Future<Output = Result<()>>;

    /// Ідемпотентна реєстрація алерта. `Ok(false)` — такий alert_key уже є.
    fn register_alert(&self, a: &Alert) -> impl Future<Output = Result<bool>>;
    /// Поставити в чергу доставку до одного чату.
    fn enqueue_delivery(&self, d: NewDelivery) -> impl Future<Output = Result<()>>;
    fn due_deliveries(&self, now: i64, limit: u32) -> impl Future<Output = Result<Vec<Delivery>>>;
    fn mark_delivery(&self, d: DeliveryUpdate) -> impl Future<Output = Result<()>>;

    fn seen_event(&self, event_id: &str, ts: i64) -> impl Future<Output = Result<bool>>;
    fn seen_update(&self, update_id: i64) -> impl Future<Output = Result<bool>>;
    fn cursor(&self, name: &str) -> impl Future<Output = Result<Option<String>>>;
    fn set_cursor(&self, name: &str, v: &str) -> impl Future<Output = Result<()>>;

    fn audit(&self, a: AuditEntry) -> impl Future<Output = Result<()>>;
    fn purge_user(&self, user_id: i64) -> impl Future<Output = Result<Purged>>;  // GDPR
}
```

Трейт — **-generic**, не `dyn`: `bot` інстанціює `App<S: Store, C: WalletRadar,
N: Notifier>`, тож мономорфізація, нульова динаміка, і два бекенди БД без
`dyn`. Вибір конкретного `S` — на етапі компіляції (feature `postgres`),
для SQLite це типовий випадок.

```rust
// wr-client/src/lib.rs
pub trait WalletRadar {
    fn snapshot(&self) -> impl Future<Output = Result<Snapshot>>;   // агрегати, вже санітизовані
    fn events(&self, limit: u32) -> impl Future<Output = Result<Vec<RawEvent>>>;
    fn connect(&self) -> impl Future<Output = Result<WsStream>>;     // Stream<Item = Result<ServerMsg>>
}

/// Зріз агрегатів. ПОЛЯ — ЦЕ WHITELIST. Усе інше з відповіді API відкидається.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Snapshot {
    pub as_of: i64,
    pub signals: Vec<FamilySignal>,        // /api/signals
    pub whale_assets: Vec<WhaleAsset>,      // /api/whale-assets
    pub groups: Vec<CapitalGroup>,          // /api/capital-groups (агреговано!)
    pub cross_asset: Vec<CrossAssetRow>,    // /api/cross-asset
    pub liquidations: Option<LiqSnapshot>,  // /api/liquidations
    pub archetypes: Vec<Archetype>,         // /api/meta-subnets
    pub track_record: Option<TrackRecord>,  // /api/track-record
}
```

```rust
// privacy-filter/src/lib.rs
pub struct ScrubReport {
    pub text: String,
    pub redactions: Vec<Redaction>,   // kind + позиція — у лог без самого тексту
}
pub trait Sanitizer {
    fn scrub(&self, text: &str) -> ScrubReport;
}
```

```rust
// broadcast/src/lib.rs
pub trait Notifier {
    fn send(&self, chat_id: i64, msg: &Outbound) -> impl Future<Output = Result<(), SendError>>;
}
pub trait Broadcaster {
    fn publish(&self, alert: Alert) -> impl Future<Output = Result<PublishOutcome>>;
}
```

```rust
// telegram-gateway/src/lib.rs
#[derive(Clone, Debug)]
pub struct IncomingUpdate {
    pub update_id: i64,
    pub chat_id: i64,
    pub chat_type: ChatType,     // Private | Group | Channel | Supergroup
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub text: String,
    pub reply_to_message_id: Option<i64>,
}
```

### 2.4 Обробка оновлень (канонічний шлях)

```
update_id ──► seen_updates? ──► так → ігнорувати (Telegram ретранслює)
                   │ні
                   ▼
        chat_type == Private? ──► ні → звичайний текст, відповідь «лише в приватних чатах»
                   │так
                   ▼
        rate limit користувача ──► перевищено → «ти занадто часто», НЕ виконуємо команду
                   │
                   ▼
        команда? ──► command-router ──► handler ──► Reply (privacy-filter) ──► send
                   │ні
                   ▼
        NLU/fallback ──► підказка з командами
```

Кожна відповідь проходить `privacy-filter` **навіть якщо створена адміністратором
вручну**.

---

## 3. Правило 2 у деталях: «усім — алерт, автору — відповідь»

**Джерело істини — `chat_id`.** Саме він визначає, кому саме Telegram доставить
повідомлення. `user_id` — особистість (для GDPR-видалення «все, що знаю про
цю людину»).

| Ситуація | Поведінка |
|---|---|
| Приватний чат | Нормальна робота. Відповідь → у цей chat. |
| Група / супергрупа | Бот **не відповідає** на команди й не розсилає нічого в чат. Відповідь-відмова — теж приватна (її не можна надіслати в групу від імені бота, це б розкрило факт підписки). У(group) просто ігноруємо update. |
| Канал | Те саме. |
| Адмін у групі | Те саме: привілеї адміна діють у приватних чатах, не в групових. |

Чому так, а не «у групі теж працює, але без адрес»: відповідь на запит у
групі — це публікація приватних висновків групі, а сам факт участі бота в
групі розкриває, хто з групи підписаний на альтерти. Обидва витоки — лік
приватності, тому группові чати виключені повністю.

**Персональні дані.** У БД: `chat_id` (BIGINT), `user_id` (BIGINT, nullable),
`username`. Це персональні дані. Логи не містять ані `chat_id`, ані `username`
у відкритому вигляді — у лог іде `chat_ref = HMAC-SHA256(bot_secret, chat_id)`
(короткий, 12 hex). Аудит зберігає `chat_ref`, не `chat_id`.

**GDPR.** `/delete` (або команда адміна) → `purge_user(user_id)`:
видаляємо `users`, `subscriptions`, `user_assets`, `deliveries` цього `chat_id`,
рядки `audit_log` цього chat_ref, анонімізуємо `seen_updates` (не прив'язані до
користувача — лишаються, бо це технічні ідентифікатори Telegram, не профіль).
Агрегати в `alert_log` **лишаються**: вони не персональні (актив + вікно +
агрегована метрика) і не дозволяють ідентифікувати людину. Це і є «право на
забування без знищення статистики».

---

## 4. Правило 3 у деталях: заборона гаманців і транзакцій

Три незалежні шари. Кожен самотужний: збій одного не відкриває витоку.

### Шар 1 — типовий (головний)

`wr-client` розбирає відповіді API **вручну**: кожне поле з JSON переноситься в
конкретне поле типу. Поля, яких немає в типі, просто не існують. Адреси
приходять у `RawEvent`/`RawGroup` і не виходять за межі крейта.

`CapitalGroup` — окрема ілюстрація:

```rust
// Сире (з /api/capital-groups) → всередині клієнта:
struct RawGroup { size: u32, members: Vec<RawMember>, common_assets: Vec<String> }
// RawMember { address: String, label: Option<String>, count: u32 }

// Публічне (те, що бачить бот):
pub struct CapitalGroup {
    pub id: GroupId,            // стабільний хеш складу, не адреса
    pub size: u32,
    pub member_count: u32,
    pub common_assets: Vec<String>,
    pub archetypes: Vec<&'static str>,
}
```

### Шар 2 — whitelist полів на вході

Перед розбором — явний `allowed_keys` для кожної відповіді + `denied_keys`:
`address`, `txHash`, `counterparty`, `contract`, `wallet`, `root`, `walletId`,
`explorerUrl`, `url`, `link`. Ключ у denylist → **застереження в лог + поле
відкидається** (не тихо). Плюс: `member.address` → не копіюється, а лише
складається в `id` через хеш.

Контракт токена (`asset.contract`) — теж не експортується: показуємо тікер +
мережу. Правило забороняє explorer-посилання з конкретною адресою, а контракт
токена — це конкретна адреса.

### Шар 3 — санітазація тексту на виході

```rust
// privacy-filter/src/rules.rs
pub const RULES: &[Rule] = &[
    Rule::evm_address(),      // 0x + 40 hex  → приховано
    Rule::tx_hash(),          // 0x + 64 hex  → приховано
    Rule::base58_address(),   // 32..=44 base58 (Solana) → приховано
    Rule::ens_name(),         // name.eth      → приховано
    Rule::explorer_url(),     // etherscan.io / solscan.io / blockscout / dexscreener …
    Rule::long_hex(),         // будь-який hex >= 32 символів — запобіжник
];
```

Заміна — на маркер `приховано` (мова налаштовується). `ScrubReport` містить
**вид** заміни та позицію, але **не сам текст** — щоб у лог не потрапило
випадково замасковане значення (воно й так незбереже, але лог не повинен
містити навіть його довжину з адресою).

**Сума окремої транзакції** не регуляркою ловиться — і не повинна: у
`Metrics` немає поля для суми транзакції, а рендер бере лише агрегати. Тому
числове правило — структурне, а не текстове. Ручні `/broadcast` адміна
проходять через filter (адреси, tx, URL), а числа в них — адміністратор
написував сам; ми показуємо попередження, якщо filter щось змінив.

### Тест-канітар (головна гарантія)

```
golden test:
  1. mock Wallet Radar віддає ФІКСТУРИ з реальними адресами/tx (наш датасет).
  2. бот повністю проганяє цикл: poll → alert-engine → broadcast → notify.
  3. mock Telegram збирає УСІ вихідні повідомлення.
  4. assert: жодне повідомлення не містить жодного токена з корпусу.
  5. assert: кожне повідомлення проходить ScrubReport з 0 redact (тобто
     інформація не потрапила в текст навіть ДО фільтра).
```

Крок 5 важливіший за крок 4: він доводить, що спрацював **типовий** шар, а
не що regex випадково згодився. Корпус — реальні адреси з `dataset/`
(комітнути не будемо: у репозиторій іде лише хеш переліку).

Додатково — fuzz-тест на `scrub()` (随机-вхід не висликає panic) і тест, що
`Metrics`/`Alert` **не мають** жодного поля з рядком-адресою (reflection-тест
по типах).

---

## 5. Алерти: джерела, оновлення, ідемпотентність

### 5.1 Гібрид: WS — тригер, REST — контекст

| Джерело | Роль | Частота | Що дає |
|---|---|---|---|
| `WS /ws` | тригер | push | сирі події (секунди) |
| `GET /api/events?limit=N` | **відновлення після діри** | 15 с (лише в degraded) | сирі події з минулого |
| `GET /api/signals` | стан | 60 с | фази родин, `topEmission`, сектор, обсяг |
| `GET /api/whale-assets` | стан | 60 с | концентрація top-10/top-50, сектор, бенд |
| `GET /api/capital-groups` | стан | 5 хв | склад груп (агреговано) |
| `GET /api/cross-asset` | стан | 5 хв | мультиактивні гаманці → агрегати по символах |
| `GET /api/liquidations` | ринок | 60 с | ліквідації (довгі/короткі, топ-символи) |
| `GET /api/meta-subnets` | архетипи | 15 хв | архетипи спільнот |
| `GET /api/track-record` | дайджест | 1 год | зрізаний signal + caveats |

Логіка розподілу: **подія без контексту — не алерт**. WS-подія «надійшов актив
X на 12 400 USD» — це шум. Алерт «3 учасники накопичили X на 42 800 USD за 15 хв,
частка емісії групи 6.2%» — це вже рішення, і воно потребує контексту з
`/api/signals` + `/api/whale-assets`. Тому алерт-двигун працює з **вікнами**, а
не з подіями поодиноко.

Вікна: `15m` (миттєво), `1h`, `24h` (контекстні підсумки, з дайджесту).

### 5.2 Ідемпотентність — три рівні

```
подія    → seen_events(event_id)          : ігноруємо повтор (реконнект, реплей)
алерт   → alert_log.alert_key UNIQUE      : створюємо один раз
доставка→ deliveries(chat_id, alert_key)  : кожному юзеру — один рядок
команда → seen_updates(update_id)         : повторний update не виконуємо вдруге
```

```rust
// Ключ алерта. Не event_id+user_id: один event_id може дати кілька алертів
// (різні вікна/напрямки), а ключ має бути сталим для ТОГО ЖЕ твердження.
pub struct AlertKey(String);   // sha256(kind|asset|window_start|evidence_digest)

impl AlertKey {
    pub fn derive(kind: AlertKind, asset: &str, window_start: i64, evidence: &[&str]) -> Self {
        let mut h = Sha256::new();
        h.update(kind.as_str().as_bytes()); h.update(b"|");
        h.update(asset.as_bytes());         h.update(b"|");
        h.update(window_start.to_le_bytes()); h.update(b"|");
        for e in evidence { h.update(e.as_bytes()); h.update(b","); }
        Self(hex(h.finalize()))
    }
}
```

`evidence_digest` — хеш відсортованих `event_id`, які стали доказом. Той самий
набір доказів у тому самому вікні → той самий ключ → дублікат відкидається
на рівні `alert_log`, **до** того, як хтось щось отримав.

Вставка — атомарна:

```sql
INSERT INTO alert_log (alert_key, kind, asset, window_start, payload, created_at)
VALUES ($1,$2,$3,$4,$5,$6)
ON CONFLICT (alert_key) DO NOTHING
RETURNING alert_key;   -- порожньо → дублікат, нічого не робимо
```

### 5.3 Доказова база алерту

`alert_engine` не «вірить події» — він рахує з вікна:

```
accumulation(asset A, window 15m):
  events = сирі події In по A за останні 15 хв (з seen_events-фільтром)
  participants = distinct wallet_id (усе одно — хеш, не адреса)
  total_value_usd = Σ value_usd
  умова: participants >= 3 AND total_value_usd >= поріг активу
  контекст: group_share_pct (з /api/signals), concentration_top10_pct (з /api/whale-assets)
```

Це важлива властивість: **пропущена подія зменшує доказову базу, але не
створює хибного алерту**. При дірі в стрічці ми не надсилаємо «впевнений» алерт
із меншою доказовою базою — ми або не надсилаємо нічого, або надсилаємо з
позначкою `degraded`.

Пороги — у конфізі, не в коді:

```toml
[[alert_rules]]
kind = "whale_accumulation"
min_participants = 3
min_total_value_usd = 20000
windows = ["15m", "1h"]
assets = "*"          # або список тікерів
severity = "medium"
```

---

## 6. Розсилка: черга, ліміти, масштаб

### 6.1 Durable-черга

Черга — таблиця `deliveries`, не пам'ять. Память зникає на рестарті, а
«алерт не доставлено» — це втрата грошей/уваги користувача.

```sql
CREATE TABLE deliveries (
  chat_id        BIGINT NOT NULL,
  alert_key      TEXT   NOT NULL,
  priority       SMALLINT NOT NULL,   -- 0 відповідь, 1 дайджест, 2 broadcast
  status         TEXT   NOT NULL,     -- pending | sending | sent | failed | skipped
  attempts       INTEGER NOT NULL DEFAULT 0,
  next_attempt_at BIGINT NOT NULL,
  created_at     BIGINT NOT NULL,
  sent_at        BIGINT,
  last_error     TEXT,
  PRIMARY KEY (chat_id, alert_key)
);
CREATE INDEX deliveries_due ON deliveries (status, next_attempt_at);
```

Статуси `sending` → `sent/failed` дозволяють відновити чергу після падіння:
`pending OR (sending AND created_at < now-5min)` → повернути в `pending`.

### 6.2 Ліміти Telegram

Telegram: ~30 повідомлень/с глобально, ~1/с на чат, 4096 символів, 429 з
`retry_after`.

| Обмеження | Значення | Обґрунтування |
|---|---|---|
| Глобальний bucket | 25 msg/s | запас до ліміту 30 |
| Per-chat | ≥1.05 с між повідомленнями | запас до ліміту 1/с |
| Черга на чат | FIFO, максимум N непрочитаних (default 20) | при переповненні — `skipped` + лічильник «пропущено» (краще ніж спам) |
| Пріоритети | відповідь (0) > дайджест (1) > broadcast (2) | broadcast не має затримувати живу відповідь |
| 429 | `retry_after` → глобальна пауза на цей час | не дропаємо |

Ретраї: 2 с → 4 с → 8 с → … кап 5 хв, максимум 6 спроб. Перманентні помилки
(403 `bot was blocked`, 400 `chat not found`) → `skipped`, реєстрація чату як
недосяжного (подальші спроби припиняються, поки користувач не напише знову).

### 6.3 Масштабування

На 1–5 тис. користувачів: один інстанс, один воркер черги, 25 msg/s → до
~30 тис. повідомлень/хв. Бот, який розсилає 10–20 алертів/год на 3000
користувачів, — це ~1 msg/s. Запас величезний.

Що зламається першим — не Telegram, а **дублікати при перезапуску** та
**спам при спільних сигналах**: 3000 користувачів отримують однаковий алерт.
Тому два обмеження на рівні продукту:

- `broadcast_digest` — якщо алйти за вікно ≥ N (за замовчуванням 25% юзерів
  мають ту саму підписку), алерт згортається в один «зведений» дайджест раз на
  5 хв замість 3000 окремих повідомлень;
- персональні підписки на акти дають вузькі алерти (тільки «мой актив»).

Масштабування по горизонталі (окремі воркери на розсилку, Postgres, Redis-черга)
підтримане схемою БД: `deliveries` вже durable й розділяє **`chat_id` + `alert_key`**
— два воркери не візьмуть той самий рядок (`SELECT … FOR UPDATE SKIP LOCKED`
або `UPDATE … RETURNING` з фільтром).

---

## 7. Персональні дайджести без розкритчу чужих підписок

**Дайджест = агрегат + фільтр, а не персональна вибірка з бази.**

```
спільний зріз (один на всіх):   Snapshot + алерти за період
        ↓
фільтр підписки конкретного юзера: assets ⊆ його підписки, topics ⊆ його теми
        ↓
рендер у його мові/таймзоні
```

Чому так:

1. **Спільний агрегат не може витопити чужих підписок.** «X накопичує 3 групи»
   — факт про ринок, а не про те, хто на що підписаний.
2. **Фільтр — односторонній.** Ми беремо спільне і відкидаємо зайве для цього
   юзера. Обернена операція (зібрати «персональне» й додати порівняння) —
   це те, що витікаєло б.
3. **Ніколи не показуємо лічильники підписок.** У жодній відповіді (у т.ч.
   `/stats` адміна) немає «N користувачів підписано на X», «топ-5 активів за
   підписками» тощо. Адміністратор теж не має доступу до чужої статистики
   підписок — `/stats` показує операційні метрики (доставки, лаг, черга) і
   агреговані лічильники тем, **без привʼязки до людей**.

Дайджести кешуються **спільно**: ключ кешу `(period, lang, topic_set_hash)`,
де `topic_set_hash` — хеш набору тем. Для 3000 юзерів з однаковим набором тем
це один розрахунок і один рендер на мову, а не 3000.

`/digest daily|weekly` → персональний зріз. `timeframe` з `/settings` —
`24h` / `7d` / `30d`, зберігається в `settings`.

---

## 8. Реконнект і пропуски (cursor / replay)

### 8.1 Обмеження джерела (важливо)

- `GET /api/events` **не має серверного курсора**: повертає останні N подій
  newest-first, сервер тримає **500** подій (`store.rs`: `unshift`, `truncate(500)`).
- WS при реконнекті надсилає `init`-кадр з останніми **60** подіями, далі —
  живі кадри. Між `init` і першим живим кадром є вікно, у якому ми могли
  пропустити події.
- Отже: **replay можливий лише в межах 500 подій.** Далі — прогалина не
  відновлюється, і це треба чесно сказати користувачам.

### 8.2 Стратегія

```
cursor: event_watermark_ms  (остання оброблена ts)
        last_update_id

WS reconnect:
  1. backoff 1s → 60s, jitter ±20%, без ліміту спроб
  2. при reconnect — init-кадр: події з ts > watermark (або всі, якщо watermark
     порожній) → обробити через той самий dedup (seen_events)
  3. ЯКЩО watermark < (init-кадр найстаріша ts) − 1s → ГАП (пропущено події):
        stream.gap = true
        алерти з цього періоду позначаються degraded=true
        адмінам — повідомлення «дірка в стрічці»
  4. паралельно — REST-поллер (15 с) як страховка: /api/events?limit=500,
     фільтр ts > watermark, dedup за event_id. Дзеркалить дублікати без шкоди.
  5. поллер зупиняється через 2 успішні WS-кадри після reconnect
```

### 8.3 Режими роботи джерела

| Режим | Умова | Поведінка |
|---|---|---|
| `healthy` | лаг < 5 хв, WS живий | алерти як завжди |
| `degraded` | лаг > 5 хв або REST недоступний > 2 хв | алерти **призупинено**; відповіді на запити — з кешу з позначкою «стан на HH:MM» |
| `stale` | джерело недоступне > 30 хв | бот живий, але повідомляє «джерело недоступне»; черга алертів буферизується |
| `gap` | виявлено пропуск | `degraded=true` на алертах цього періоду + сповіщення адмінам |

Чому в `degraded` алерти **призупинено**, а не «надсилаємо те, що є»: алерт
«3 учасники накопичили X» при дірі в стрічці може бути неповним — а
неповний алерт гірший за відсутність: людина приймає рішення за даними, які
неповні, і не знає про це.

Буфер: алерти, згенеровані під час короткої діри (< 30 хв), **не виходять
покинути** — вони чекають у `alert_log` + `deliveries`. Якщо за 30 хв джерело
не ожило, буфер скидається (алерти застаріли — надсилати «кит накопичував
2 години тому» некоректно, вікно застаріло). При відновленні дублікатів не
буде: `alert_key` той самий (та сама доказова база) → `ON CONFLICT DO NOTHING`.

#### 8.3.1 Реалізація межі буфера

Механізм має три частини — усі в коді, не «на папері»:

**1. Гейт джерела.** `Broadcaster::source_gate()` — спільний `Arc<AtomicBool>`.
Коли джерело недоступне, воркер:

* не відбирає доставки (`pump_once` повертає 0) — черга **буферизується**;
* не зливає згорнуті дайджести (`flush_digests` повертає 0) — інакше вікно
  згортання «з'їдалося» б у буфері.

Що саме вважається «джерело недоступне»: **сервер не відповів узагалі**.
Секції зрізу деградують у порожні (одна недоступна секція не ламає зріз), тож
сам `snapshot()` майже ніколи не падає. Ознака доступності —
`Snapshot::source_reachable`: її рахує клієнт за трьома **опорними**
секціями (`/api/whale-assets`, `/api/capital-groups`, `/api/cross-asset`), і
важливо — саме «сервер відповів», а не «маршрут існує»: відсутність маршруту
(404, як у розгорнутого Wallet Radar для `/api/signals`) — це відповідь
сервера, тож джерело доступне.

Сигнал «недоступне» діє **понад 2 хв** (`SOURCE_DEGRADED_AFTER_SECS` у
`bot/src/main.rs`) — та сама логіка, що в таблиці режимів вище («REST
недоступний > 2 хв» → `degraded`). Зрив самого WS гейту **не** вмикає: WS і
REST дублюють одне одного, і REST-поллер — страховка. При першому ж успішному
зрізі гейт знімається, буфер виливається.

**2. Межа буфера.** `max_backlog_minutes` — скільки хвилин доставка має
право чекати. `Store::expire_pending_before(cutoff)` переводить `pending`-
доставки, старші за межу, у `skipped` з поміткою «буфер прострочено: алерт
застарів». Такі доставки **не надсилаються ніколи** — навіть після відновлення
джерела.

Перевірка межі — **на кожному кроці воркера, до відбору доставок**. Це
важливо: раз на 30 секунд прострочена доставка встигала б вийти з черги й піти
користувачеві на один цикл раніше, ніж її скинули (цей дефект знайдено на
живих даних і закрито тестом `воркер_скидає_прострочене_до_відправлення`).
Щоб перевірка була дешевою, є індекс `deliveries_backlog_idx (status,
created_at)`.

**3. `0` = обмеження вимкнено.** Буфер безстроковий: нічого не скидається.
(На відміну від `digest_threshold_pct`, де `0` означає «максимальна
агресивність» — тут `0` означає «не обмежувати», бо ліміт у нуль викинув би
у чергу й свіжі доставки.)

**Межа стосується тільки черги.** Приватні відповіді (`reply`) і дайджести
за період (`/digest`) не проходять через чергу — вони йдуть одразу, навіть
коли джерело недоступне.

Стан видно в `/stats`: «буфер: N у черзі · скинуто за межею: M».

**Де буфер живе:** у БД (`deliveries` зі статусом `pending` + `fold_pending`),
а не в памʼяті — рестарт процесу не втрачає накопичене.

### 8.4 Heartbeat і «завислий» WS

Telegram `getUpdates` з `timeout=25` повертає порожній результат, коли
немає оновлень. Якщо за 90 секунд не прийшло **жодного** кадру (навіть
порожнього) — вважаємо з'єднання мертвим, перепідключаємося.

Для поллера Telegram «кадр» — це **успішна відповідь на опитування**, навіть
порожня: у спокійному бота оновлень може не бути годинами, і це нормально.
Тому heartbeat міряє час останнього **успішного** опитування, а не останнього
оновлення. (Раніше міряли оновлення — через це бот писав «Telegram не
надсилає оновлень» через три години без жодної команди; це хибне попередження
виправлено.)

### 8.5 Telegram: конфлікт полерів

Якщо Telegram відповідає **409 Conflict** — це означає, що на токені вже
працює інший полер. Це той самий висновок, що й у README `deploy/bot-windows`:
**один полер на токен**.

Політика поведінки — конфігом (`[bot] poll_conflict_policy`):

* `exit` — завершити процес із помилкою (код виходу ненульовий): суворіше,
  нічого не дублюємо; systemd підніме бота знову, але не більше 5 разів за
  5 хвилин;
* `keep_running` (типово) — призупинити **лише опитування команд**, а збір
  даних, згортання й доставку алертів лишити працювати. Опитування
  повторюється щохвилини й оживе саме тоді, коли конфлікт зникне.

#### 8.5.1 Видимість конфлікту

Під час конфлікту сам `/stats` недоступний — адмін просто не отримає
відповіді на команду. Тому стан поллера має два виходи:

1. **Повідомлення адмінам.** На переході в `Conflict` бот пише адмінам
   («на токені вже працює інший поллер (409) — команди не обробляються,
   алерти й далі доставляються»), а на відновленні — «опитування
   відновилося». Захист від спаму: не частіше ніж раз на 15 хвилин, і
   повідомлення про відновлення — лише якщо конфлікт тривав щонайменше
   2 хвилини (коротке «миготіння» не варто показувати).
2. **Рядок у `/stats`.** `PollHealth::describe()` — «команди: працюють» /
   «недоступні — на токені вже працює інший поллер (409)». Стан живе в
   `AtomicU8` і ділиться між циклом опитування й роутером.

Стан: `Unknown → Healthy → Conflict → Error` (`PollState`). Зміну стану
фіксує той, хто має на це право: цикл опитування в `bot/src/main.rs`.

---

## 9. Схема БД

```sql
PRAGMA journal_mode = WAL;          -- SQLite: читачі не блокують запис

CREATE TABLE users (
  chat_id        BIGINT PRIMARY KEY,
  user_id        BIGINT,                        -- особистість (GDPR-основа)
  username       TEXT,
  role           TEXT NOT NULL DEFAULT 'viewer',   -- viewer | admin
  language       TEXT NOT NULL DEFAULT 'uk',
  timezone       TEXT NOT NULL DEFAULT 'UTC',
  timeframe      TEXT NOT NULL DEFAULT '24h',
  muted          INTEGER NOT NULL DEFAULT 0,
  blocked        INTEGER NOT NULL DEFAULT 0,
  state          TEXT NOT NULL DEFAULT 'active',   -- pending | active | blocked
  created_at     BIGINT NOT NULL,
  updated_at     BIGINT NOT NULL,
  last_seen_at   BIGINT
);
CREATE UNIQUE INDEX users_user_id ON users(user_id) WHERE user_id IS NOT NULL;

CREATE TABLE subscriptions (
  chat_id  BIGINT NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
  topic    TEXT NOT NULL,        -- whale | group | cross_asset | liquidation | digest
  min_severity TEXT NOT NULL DEFAULT 'medium',
  PRIMARY KEY (chat_id, topic)
);

CREATE TABLE user_assets (          -- підписка на конкретний актив
  chat_id BIGINT NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
  symbol  TEXT NOT NULL,
  PRIMARY KEY (chat_id, symbol)
);

CREATE TABLE alert_log (
  alert_key    TEXT PRIMARY KEY,     -- idempotency
  kind         TEXT NOT NULL,
  asset        TEXT,
  window_start BIGINT NOT NULL,
  severity     TEXT NOT NULL,
  degraded     INTEGER NOT NULL DEFAULT 0,
  payload      TEXT NOT NULL,        -- JSON БЕЗ адрес (генеруємо самі)
  created_at   BIGINT NOT NULL
);
CREATE INDEX alert_log_time ON alert_log (created_at);

CREATE TABLE deliveries (
  chat_id        BIGINT NOT NULL,
  alert_key      TEXT NOT NULL,
  priority       SMALLINT NOT NULL,
  status         TEXT NOT NULL,
  attempts       INTEGER NOT NULL DEFAULT 0,
  next_attempt_at BIGINT NOT NULL,
  created_at     BIGINT NOT NULL,
  sent_at        BIGINT,
  last_error     TEXT,
  PRIMARY KEY (chat_id, alert_key)
);
CREATE INDEX deliveries_due ON deliveries (status, next_attempt_at, priority);

CREATE TABLE seen_events (
  event_id TEXT PRIMARY KEY,
  ts       BIGINT NOT NULL
);
CREATE INDEX seen_events_ts ON seen_events (ts);

CREATE TABLE seen_updates (
  update_id BIGINT PRIMARY KEY,
  seen_at   BIGINT NOT NULL
);

CREATE TABLE snapshots (            -- останній зріз агрегатів (переживає рестарт)
  as_of   BIGINT PRIMARY KEY,
  kind    TEXT NOT NULL,            -- 'full'
  payload TEXT NOT NULL             -- JSON санітизований
);

CREATE TABLE audit_log (
  id       INTEGER PRIMARY KEY,
  chat_ref TEXT NOT NULL,           -- HMAC, НЕ chat_id
  actor    TEXT NOT NULL,           -- 'user' | 'admin' | 'system'
  action   TEXT NOT NULL,
  detail   TEXT,                    -- без PII, без тексту повідомлень
  at       BIGINT NOT NULL
);

CREATE TABLE kv (                    -- курсори, лічильники, health
  k          TEXT PRIMARY KEY,
  v          TEXT NOT NULL,
  updated_at BIGINT NOT NULL
);
```

**Інваріанти, які тримає сама БД (а не код):**

- `deliveries` PK `(chat_id, alert_key)` — доставка дублюється тільки при явному
  видаленні рядка; повторна доставка = `UPDATE attempts`, не `INSERT`.
- `alert_log.alert_key` UNIQUE — один алерт існує в одному екземплярі.
- `users_user_id` UNIQUE — одна людина = один профіль (навіть якщо змінить
  юзернейм).
- `seen_events`/`seen_updates` — PRIMARY KEY, тому повтор просто не вставляється.

**GDPR-видалення**: `DELETE FROM users WHERE user_id=$1` каскадом зносить
`subscriptions`, `user_assets`, `deliveries` (FK ON DELETE CASCADE). `audit_log`
замінюємо на `DELETE ... WHERE chat_ref = $1`. `alert_log` і `snapshots`
лишаються — вони не персональні.

---

## 10. Конфігурація

Секрети — **тільки** в env (або в окремому файлі з правами 0600, який
підсилює `docker secrets`). Конфіг — TOML, шлях через `--config`.

```toml
# config.toml — жодного секрету в цьому файлі.

[bot]
name = "wallet-radar-alerts"
# Токен береться з env TELEGRAM_BOT_TOKEN; у TOML — лише ім'я змінної.
token_env = "TELEGRAM_BOT_TOKEN"
default_language = "uk"            # uk | en
default_timezone = "Europe/Kyiv"
poll_timeout_secs = 25
heartbeat_timeout_secs = 90
log_level = "info"                 # trace|debug|info|warn|error
# HMAC-секрет для chat_ref у логах. Обов'язковий: без нього логи пишуть raw chat_id.
ref_secret_env = "BOT_REF_SECRET"

[wr]
base_url = "http://127.0.0.1:3000"
username = "admin"                 # reader-логін Wallet Radar
password_env = "WR_PASSWORD"       # пароль у env, не тут
poll_interval_secs = 60            # агрегати
event_poll_interval_secs = 15      # страховка відновлення
request_timeout_secs = 20
# Ендпоінти, які дозволено читати. Забудьте додати — бот не піде за ними.
allow_endpoints = [
  "/api/signals",
  "/api/whale-assets",
  "/api/capital-groups",
  "/api/cross-asset",
  "/api/liquidations",
  "/api/meta-subnets",
  "/api/events",
  "/api/track-record",
]
ws_path = "/ws"

[storage]
backend = "sqlite"                 # sqlite | postgres
sqlite_path = "data/bot.db"
busy_timeout_ms = 5000

[privacy]
mask = "приховано"
redact_ens = true
redact_explorer_urls = true
redact_base58 = true
# Додаткові домени-правила (свої explorer'и)
extra_blocked_domains = ["myscan.example"]

[rate_limits]
per_user_per_min = 20
admin_per_min = 60
global_inbound_per_min = 200
telegram_global_per_sec = 25
telegram_per_chat_min_interval_ms = 1050
max_pending_per_chat = 20

[broadcast]
# Скільки чатів мають отримати той самий алерт, щоб він згорнувся у зведений дайджест
digest_threshold_pct = 25
digest_window_secs = 300
max_backlog_minutes = 30          # межа буфера черги (хв); 0 = без обмеження

[admin]
usernames = ["vitalii_vedana"]    # порожньо = без адмінських команд
access_code_env = "BOT_ACCESS_CODE"  # порожньо = /start без коду

[features]
assistant = false                 # /api/ask — за замовчуванням вимкнено (див. §12)
```

Перекриття: env > TOML > значення за замовчуванням. Секрети читаються
лише з env, ніколи не пишуться в лог.

---

## 11. Адмін-команди й аудит

| Команда | Дія | Аудит |
|---|---|---|
| `/broadcast <текст>` | ручний алерт усім | `audit(admin, broadcast, len)` |
| `/stats` | операційні метрики: юзери, алерти/день, доставки ok/fail, черга, лаг джерела, режим | — |
| `/users` | кількість активних, без списку (список — тільки для конкретного `/user <id>`) | `audit(admin, users_read)` |
| `/block <chat_id>` / `/unblock <chat_id>` | блокування | `audit(admin, block, chat_ref)` |
| `/health` | деталі джерела: режим, лаг, WS-стан | `audit(admin, health)` |
| `/delete` | видалити власні дані (GDPR) | `audit(user, gdpr_delete)` |

`/broadcast` проходить `privacy-filter`; якщо filter змінив текст — адміну
приходить підтвердження «повідомлення змінено (N рядків приховано)».

Аудит пише `chat_ref` (HMAC), дію, час і **без тексту повідомлень**.

---

## 12. Ризики і свідомі рішення

| Ризик | Рішення |
|---|---|
| Витік адреси через `/api/ask` (LLM цитує сирі дані) | `/api/ask` **вимкнено за замовчуванням** (`features.assistant=false`). Навіть увімкнувши — відповідь проходить filter, і бот не відправляє відповідь асистента, поки filter не підтвердить чистоту. |
| Витік через помилку API (сервер повертає stack trace з адресою) | Текст помилки **не відправляється** користувачеві; у відповідь — код помилки + `chat_ref`. Деталі — у лог. |
| Обхід фільтра через Unicode/zero-width | Sanitizer нормалізує (NFKC), зрізає zero-width і керує шляхи. |
| Фільтр з'їдає корисний текст (довгий base58) | Правила звужені: EVM 0x+40 hex, tx 0x+64 hex, base58 рівно 32..=44, explorer-домени з allowlist. `ScrubReport` показує, що саме змінилось. |
| Дуплікати алертів при рестарті | `alert_log.alert_key` UNIQUE + `deliveries` PK. |
| Пропуск алертів під час недоступності джерела | Буфер до 30 хв; далі — зведення в дайджест + позначка «джерело було недоступне». |
| Telegram 409 (два поллери) | Фатальна помилка з чітким текстом, без retry-циклу. |
| Зростання `seen_events` | TTL-прибирання: старіші за 7 днів. |
| Зростання `alert_log` | Архів: `payload` старших за 90 днів → NULL, рядок лишається (історія алертів потрібна для track-record). |
| Бот як «машина для крадіжки даних» | Весь вихід проходить filter; golden-тест; аудит; бот не має жодного команди з shell (на відміну від старого `botd`). |

---

## 13. Що свідомо НЕ входить

- Адмінський shell, запуск бінів, `!`-команди — у новому продукті їх немає
  (це була головна ризикова фіча старого `botd`).
- Сирі рядки провайдера (tx, адреси) — заборонені; `/api/transfers` у самому
  Wallet Radar вимкнений із тієї самої причини.
- Персональні порівняння «ти vs ринок за підписками».
- Публічні групи/канали — бот лише приватний.

---

## 14. План реалізації

| Крок | Крейт | Етап | Тест |
|---|---|---|---|
| 1 | `privacy-filter` | регулярки, NFKC, `ScrubReport` | unit + fuzz + корпус |
| 2 | `storage` | трейт, схема, SQLite | схема + CRUD + GDPR purge |
| 3 | `wr-client` | REST-parse з whitelist, типизація | golden-фікстури API |
| 4 | `alert-engine` | вікна, пороги, `AlertKey` | детерміновані кейси |
| 5 | `broadcast` | черга, idempotency, retry, rate limit | з mock-нотифікатором |
| 6 | `user-session` | стан, підписки, налаштування | ізоляція між двома чатами |
| 7 | `telegram-gateway` | поллінг, роутинг, формати | mock Telegram |
| 8 | `admin` | команди, аудит | golden |
| 9 | `bot` | оркестрація, конфіг, shutdown | smoke |
| 10 | golden | **повний прогін з mock-джерелом**: жодної адреси у виході | **головна гарантія** |
