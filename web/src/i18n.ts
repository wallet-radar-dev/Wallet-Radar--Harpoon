/* Localization (i18n): per-feature translation dicts merged into TR, plus t()/lang. */
import type { Lang } from "./types";

const TR: Record<Lang, Record<string, string>> = {
  uk: {
    doc_title: "Wallet Radar — трекер гаманців",
    brand_subtitle: "мультичейн-трекер гаманців у реальному часі",
    skip_to_content: "Перейти до вмісту",
    lang_label: "Мова інтерфейсу",
    search_label: "Пошук гаманця, активу або сутності",
    cg_refresh_title: "Оновити",
    graph_wallet_label: "Гаманець графа",
    graph_canvas_label: "Інтерактивний граф зв'язків гаманця",
    dash_type_label: "Тип метрики",
    alerts_scope_label: "Область правила",
    alerts_kind_label: "Тип правила",
    tracer_wallet_label: "Гаманець трасування",
    conn_demo: "Підключено (демо)",
    stat_wallets: "Гаманців під наглядом",
    stat_value: "Сумарна вартість",
    stat_events: "Подій сьогодні",
    stat_networks: "Активних мереж",
    add_wallet: "Додати гаманець",
    ph_address: "0x... або адреса гаманця",
    networks_pick: "Мережі (познач галочками)",
    sel_all: "усі",
    sel_none: "очистити",
    ph_label: "назва (необов'язково), напр. «гаманець кита»",
    btn_add_wallet: "Додати гаманець",
    wallets: "Гаманці",
    wallets_count_suffix: "шт.",
    live_feed: "Жива стрічка подій",
    clear: "Очистити",
    feed_empty: "Очікуємо на події… (у демо-режимі вони генеруються автоматично)",
    feed_cleared: "Стрічку очищено. Очікуємо нові події…",
    footer_note: "Демо-режим: дані змодельовано локально. У робочій версії стрічка живиться від Moralis Streams через WebSocket.",
    modal_title: "Деталі події",
    modal_close_title: "Закрити (Esc)",
    wallets_empty: "Ще немає гаманців — додайте перший вище.",
    click_for_details: "Натисни, щоб побачити деталі",
    err: "Помилка",
    err_no_address: "Введіть адресу гаманця",
    err_no_network: "Познач галочкою хоча б одну мережу",
    toast_added_title: "Гаманець додано",
    networks_word: "мереж",
    big_event: "Велика подія: ",
    kind_in: "Отримано", kind_out: "Відправлено", kind_swap: "Обмін", kind_approval: "Дозвіл (approval)",
    entity_cex: "Біржа (CEX)", entity_fund: "Фонд / маркетмейкер", entity_defi: "DeFi-протокол",
    entity_issuer: "Емітент", entity_individual: "Відома особа", entity_unknown: "Не атрибутовано",
    m_type: "Тип події", m_direction: "Напрямок", m_network: "Мережа", m_asset: "Актив",
    m_amount: "Сума", m_value: "Вартість", m_fee: "Комісія мережі", m_wallet: "Гаманець (відстежується)",
    m_counterparty: "Контрагент", m_txhash: "Хеш транзакції", m_block: "Блок", m_time: "Час", m_eventid: "ID події",
    dir_out: "Вихідна (кошти йдуть з гаманця)", dir_in: "Вхідна (кошти надходять)", dir_internal: "Внутрішня / контракт",
    status_confirmed: "підтверджено", status_pending: "очікує", btn_explorer: "Оглядач",
  },
  en: {
    doc_title: "Wallet Radar — wallet tracker",
    brand_subtitle: "multi-chain real-time wallet tracker",
    skip_to_content: "Skip to content",
    lang_label: "Interface language",
    search_label: "Search wallet, asset or entity",
    cg_refresh_title: "Refresh",
    graph_wallet_label: "Graph wallet",
    graph_canvas_label: "Interactive wallet connection graph",
    dash_type_label: "Metric type",
    alerts_scope_label: "Rule scope",
    alerts_kind_label: "Rule type",
    tracer_wallet_label: "Tracer wallet",
    conn_demo: "Connected (demo)",
    stat_wallets: "Wallets tracked",
    stat_value: "Total value",
    stat_events: "Events today",
    stat_networks: "Active networks",
    add_wallet: "Add wallet",
    ph_address: "0x... or wallet address",
    networks_pick: "Networks (tick to select)",
    sel_all: "all",
    sel_none: "clear",
    ph_label: "label (optional), e.g. “whale wallet”",
    btn_add_wallet: "Add wallet",
    wallets: "Wallets",
    wallets_count_suffix: "",
    live_feed: "Live event feed",
    clear: "Clear",
    feed_empty: "Waiting for events… (auto-generated in demo mode)",
    feed_cleared: "Feed cleared. Waiting for new events…",
    footer_note: "Demo mode: data simulated locally. In production the feed is powered by Moralis Streams over WebSocket.",
    modal_title: "Event details",
    modal_close_title: "Close (Esc)",
    wallets_empty: "No wallets yet — add your first above.",
    click_for_details: "Click to see details",
    err: "Error",
    err_no_address: "Enter a wallet address",
    err_no_network: "Tick at least one network",
    toast_added_title: "Wallet added",
    networks_word: "networks",
    big_event: "Large event: ",
    kind_in: "Received", kind_out: "Sent", kind_swap: "Swap", kind_approval: "Approval",
    entity_cex: "Exchange (CEX)", entity_fund: "Fund / market maker", entity_defi: "DeFi protocol",
    entity_issuer: "Issuer", entity_individual: "Known individual", entity_unknown: "Unattributed",
    m_type: "Event type", m_direction: "Direction", m_network: "Network", m_asset: "Asset",
    m_amount: "Amount", m_value: "Value", m_fee: "Network fee", m_wallet: "Wallet (tracked)",
    m_counterparty: "Counterparty", m_txhash: "Transaction hash", m_block: "Block", m_time: "Time", m_eventid: "Event ID",
    dir_out: "Outgoing (funds leave the wallet)", dir_in: "Incoming (funds arrive)", dir_internal: "Internal / contract",
    status_confirmed: "confirmed", status_pending: "pending", btn_explorer: "Explorer",
  },
  de: {
    doc_title: "Wallet Radar — Wallet-Tracker",
    brand_subtitle: "Multichain-Echtzeit-Wallet-Tracker",
    skip_to_content: "Zum Inhalt springen",
    lang_label: "Oberflächensprache",
    search_label: "Wallet, Asset oder Entität suchen",
    cg_refresh_title: "Aktualisieren",
    graph_wallet_label: "Graph-Wallet",
    graph_canvas_label: "Interaktiver Wallet-Verbindungsgraph",
    dash_type_label: "Metriktyp",
    alerts_scope_label: "Regelbereich",
    alerts_kind_label: "Regeltyp",
    tracer_wallet_label: "Tracer-Wallet",
    conn_demo: "Verbunden (Demo)",
    stat_wallets: "Überwachte Wallets",
    stat_value: "Gesamtwert",
    stat_events: "Ereignisse heute",
    stat_networks: "Aktive Netzwerke",
    add_wallet: "Wallet hinzufügen",
    ph_address: "0x... oder Wallet-Adresse",
    networks_pick: "Netzwerke (zum Auswählen ankreuzen)",
    sel_all: "alle",
    sel_none: "leeren",
    ph_label: "Bezeichnung (optional), z. B. „Wal-Wallet“",
    btn_add_wallet: "Wallet hinzufügen",
    wallets: "Wallets",
    wallets_count_suffix: "Stk.",
    live_feed: "Live-Ereignis-Feed",
    clear: "Leeren",
    feed_empty: "Warte auf Ereignisse… (im Demomodus automatisch erzeugt)",
    feed_cleared: "Feed geleert. Warte auf neue Ereignisse…",
    footer_note: "Demomodus: Daten lokal simuliert. In der Produktivversion wird der Feed über Moralis Streams via WebSocket gespeist.",
    modal_title: "Ereignisdetails",
    modal_close_title: "Schließen (Esc)",
    wallets_empty: "Noch keine Wallets — fügen Sie oben Ihre erste hinzu.",
    click_for_details: "Zum Anzeigen der Details klicken",
    err: "Fehler",
    err_no_address: "Wallet-Adresse eingeben",
    err_no_network: "Mindestens ein Netzwerk ankreuzen",
    toast_added_title: "Wallet hinzugefügt",
    networks_word: "Netzwerke",
    big_event: "Großes Ereignis: ",
    kind_in: "Erhalten", kind_out: "Gesendet", kind_swap: "Tausch", kind_approval: "Freigabe (Approval)",
    entity_cex: "Börse (CEX)", entity_fund: "Fonds / Market Maker", entity_defi: "DeFi-Protokoll",
    entity_issuer: "Emittent", entity_individual: "Bekannte Person", entity_unknown: "Nicht zugeordnet",
    m_type: "Ereignistyp", m_direction: "Richtung", m_network: "Netzwerk", m_asset: "Asset",
    m_amount: "Betrag", m_value: "Wert", m_fee: "Netzwerkgebühr", m_wallet: "Wallet (überwacht)",
    m_counterparty: "Gegenpartei", m_txhash: "Transaktions-Hash", m_block: "Block", m_time: "Zeit", m_eventid: "Ereignis-ID",
    dir_out: "Ausgehend (Mittel verlassen die Wallet)", dir_in: "Eingehend (Mittel kommen an)", dir_internal: "Intern / Vertrag",
    status_confirmed: "bestätigt", status_pending: "ausstehend", btn_explorer: "Explorer",
  },
  fr: {
    doc_title: "Wallet Radar — suivi de portefeuilles",
    brand_subtitle: "suivi de portefeuilles multichaînes en temps réel",
    skip_to_content: "Aller au contenu",
    lang_label: "Langue de l'interface",
    search_label: "Rechercher un portefeuille, un actif ou une entité",
    cg_refresh_title: "Actualiser",
    graph_wallet_label: "Portefeuille du graphe",
    graph_canvas_label: "Graphe interactif des connexions du portefeuille",
    dash_type_label: "Type de métrique",
    alerts_scope_label: "Portée de la règle",
    alerts_kind_label: "Type de règle",
    tracer_wallet_label: "Portefeuille du traceur",
    conn_demo: "Connecté (démo)",
    stat_wallets: "Portefeuilles suivis",
    stat_value: "Valeur totale",
    stat_events: "Événements aujourd'hui",
    stat_networks: "Réseaux actifs",
    add_wallet: "Ajouter un portefeuille",
    ph_address: "0x... ou adresse de portefeuille",
    networks_pick: "Réseaux (cochez pour sélectionner)",
    sel_all: "tous",
    sel_none: "effacer",
    ph_label: "libellé (facultatif), ex. « portefeuille baleine »",
    btn_add_wallet: "Ajouter un portefeuille",
    wallets: "Portefeuilles",
    wallets_count_suffix: "",
    live_feed: "Flux d'événements en direct",
    clear: "Effacer",
    feed_empty: "En attente d'événements… (générés automatiquement en mode démo)",
    feed_cleared: "Flux effacé. En attente de nouveaux événements…",
    footer_note: "Mode démo : données simulées localement. En production, le flux est alimenté par Moralis Streams via WebSocket.",
    modal_title: "Détails de l'événement",
    modal_close_title: "Fermer (Échap)",
    wallets_empty: "Aucun portefeuille — ajoutez le premier ci-dessus.",
    click_for_details: "Cliquez pour voir les détails",
    err: "Erreur",
    err_no_address: "Saisissez une adresse de portefeuille",
    err_no_network: "Cochez au moins un réseau",
    toast_added_title: "Portefeuille ajouté",
    networks_word: "réseaux",
    big_event: "Événement important : ",
    kind_in: "Reçu", kind_out: "Envoyé", kind_swap: "Échange", kind_approval: "Autorisation (approval)",
    entity_cex: "Plateforme (CEX)", entity_fund: "Fonds / teneur de marché", entity_defi: "Protocole DeFi",
    entity_issuer: "Émetteur", entity_individual: "Personne connue", entity_unknown: "Non attribué",
    m_type: "Type d'événement", m_direction: "Direction", m_network: "Réseau", m_asset: "Actif",
    m_amount: "Montant", m_value: "Valeur", m_fee: "Frais de réseau", m_wallet: "Portefeuille (suivi)",
    m_counterparty: "Contrepartie", m_txhash: "Hash de transaction", m_block: "Bloc", m_time: "Heure", m_eventid: "ID de l'événement",
    dir_out: "Sortant (les fonds quittent le portefeuille)", dir_in: "Entrant (les fonds arrivent)", dir_internal: "Interne / contrat",
    status_confirmed: "confirmé", status_pending: "en attente", btn_explorer: "Explorateur",
  },
  ru: {
    doc_title: "Wallet Radar — трекер кошельков",
    brand_subtitle: "мультичейн-трекер кошельков в реальном времени",
    skip_to_content: "Перейти к содержимому",
    lang_label: "Язык интерфейса",
    search_label: "Поиск кошелька, актива или сущности",
    cg_refresh_title: "Обновить",
    graph_wallet_label: "Кошелёк графа",
    graph_canvas_label: "Интерактивный граф связей кошелька",
    dash_type_label: "Тип метрики",
    alerts_scope_label: "Область правила",
    alerts_kind_label: "Тип правила",
    tracer_wallet_label: "Кошелёк трассировки",
    conn_demo: "Подключено (демо)",
    stat_wallets: "Кошельков под наблюдением",
    stat_value: "Суммарная стоимость",
    stat_events: "Событий сегодня",
    stat_networks: "Активных сетей",
    add_wallet: "Добавить кошелёк",
    ph_address: "0x... или адрес кошелька",
    networks_pick: "Сети (отметьте галочками)",
    sel_all: "все",
    sel_none: "очистить",
    ph_label: "название (необязательно), напр. «кошелёк кита»",
    btn_add_wallet: "Добавить кошелёк",
    wallets: "Кошельки",
    wallets_count_suffix: "шт.",
    live_feed: "Живая лента событий",
    clear: "Очистить",
    feed_empty: "Ожидаем события… (в демо-режиме генерируются автоматически)",
    feed_cleared: "Лента очищена. Ожидаем новые события…",
    footer_note: "Демо-режим: данные смоделированы локально. В рабочей версии лента питается от Moralis Streams через WebSocket.",
    modal_title: "Детали события",
    modal_close_title: "Закрыть (Esc)",
    wallets_empty: "Кошельков пока нет — добавьте первый выше.",
    click_for_details: "Нажмите, чтобы увидеть детали",
    err: "Ошибка",
    err_no_address: "Введите адрес кошелька",
    err_no_network: "Отметьте хотя бы одну сеть",
    toast_added_title: "Кошелёк добавлен",
    networks_word: "сетей",
    big_event: "Крупное событие: ",
    kind_in: "Получено", kind_out: "Отправлено", kind_swap: "Обмен", kind_approval: "Разрешение (approval)",
    entity_cex: "Биржа (CEX)", entity_fund: "Фонд / маркетмейкер", entity_defi: "DeFi-протокол",
    entity_issuer: "Эмитент", entity_individual: "Известное лицо", entity_unknown: "Не атрибутировано",
    m_type: "Тип события", m_direction: "Направление", m_network: "Сеть", m_asset: "Актив",
    m_amount: "Сумма", m_value: "Стоимость", m_fee: "Комиссия сети", m_wallet: "Кошелёк (отслеживается)",
    m_counterparty: "Контрагент", m_txhash: "Хеш транзакции", m_block: "Блок", m_time: "Время", m_eventid: "ID события",
    dir_out: "Исходящая (средства уходят с кошелька)", dir_in: "Входящая (средства поступают)", dir_internal: "Внутренняя / контракт",
    status_confirmed: "подтверждено", status_pending: "ожидает", btn_explorer: "Обозреватель",
  },
};

const GRAPH_TR: Record<Lang, Record<string, string>> = {
  uk: { graph_btn: "Граф зв'язків", graph_title: "Павутина зв'язків гаманців", graph_hint: "Тягніть вузли · колесо — масштаб · фон — переміщення", graph_empty: "Поки немає зв'язків — зачекайте на події.", graph_entities: "Сутності", graph_links: "Зв'язки" },
  en: { graph_btn: "Connections graph", graph_title: "Wallet connection web", graph_hint: "Drag nodes · wheel to zoom · drag background to pan", graph_empty: "No connections yet — wait for events.", graph_entities: "Entities", graph_links: "Links" },
  de: { graph_btn: "Verbindungsgraph", graph_title: "Wallet-Verbindungsnetz", graph_hint: "Knoten ziehen · Mausrad zum Zoomen · Hintergrund ziehen zum Verschieben", graph_empty: "Noch keine Verbindungen — auf Ereignisse warten.", graph_entities: "Entitäten", graph_links: "Verbindungen" },
  fr: { graph_btn: "Graphe des liens", graph_title: "Toile de connexions des portefeuilles", graph_hint: "Glissez les nœuds · molette pour zoomer · fond pour déplacer", graph_empty: "Pas encore de connexions — attendez des événements.", graph_entities: "Entités", graph_links: "Liens" },
  ru: { graph_btn: "Граф связей", graph_title: "Паутина связей кошельков", graph_hint: "Тяните узлы · колесо — масштаб · фон — перемещение", graph_empty: "Пока нет связей — дождитесь событий.", graph_entities: "Сущности", graph_links: "Связи" },
};
for (const l of Object.keys(GRAPH_TR) as Lang[]) Object.assign(TR[l], GRAPH_TR[l]);

const STRAT_TR: Record<Lang, Record<string, string>> = {
  uk: {
    strat_btn: "Стратегія", strat_title: "🏛 Інституційна стратегія", strat_building: "Будуємо портфель…",
    prof_aggressive: "Агресивна", prof_moderate: "Помірна", prof_conservative: "Консервативна", prof_smartmoney: "Смартмані",
    prof_aggressive_desc: "Макс. альфа. Менше пасиву, великий сателіт (вкл. спірні активи), пізній вихід (E=55%) — їдемо за рухом кита довше. Вища дохідність ↔ вища просадка.",
    prof_moderate_desc: "Баланс. Пасивне ядро BTC/ETH + альфа-сателіт із якісних активів, вихід E=48%. Кращий risk-adjusted, ніж наївний альт-кошик.",
    prof_conservative_desc: "Збереження капіталу. Велике ядро + буфер стейблів, мало сателіта (топ-якість), ранній вихід (E=30%) — фіксуємо прибуток швидко, мінімізуємо просадку.",
    prof_smartmoney_desc: "Чисте слідування за китом. Майже без пасиву — капітал іде в активи, де смарт-гроші ЗАРАЗ накопичують (фаза DCA-IN), вага за силою сигналу. Вихід строго по драбині кита.",
    strat_core: "Ядро (Core)", strat_satellite: "Сателіт", strat_stable: "Стейбл", strat_target: "ціль",
    strat_core_role: "Пасивна база — 🟢 CLARITY-товари (BTC/ETH), низький ризик, низький оборот",
    strat_sat_role: "Активна альфа — активи у фазі накопичення кита (DCA-IN), ризик-вагування + кеп",
    strat_stable_role: "Сухий порох / буфер — стейблкоїни, доливаємо в сателіт на просадках (DCA)",
    strat_factors: "📐 Фактори відбору", strat_risk: "🛡 Ризик-менеджмент", strat_assets: "активів", strat_maxw: "макс. вага",
    strat_f_flow: "Flow (накопичення китів) → відбір і вага сателіта", strat_f_quality: "Quality (низька концентрація холдерів) → допуск",
    strat_f_reg: "Regulatory (CLARITY 🟢 товар) → гейт ядра й сателіта", strat_f_liq: "Liquidity (розмір) → вага ядра",
    strat_r_cap: "Кеп на актив: ядро ≤ {a}%, сателіт ≤ {b}%", strat_r_excl: "Виключено з ризик-слівів: 🔴 спірні, 🟠 біржові, 🟣 RWA",
    strat_r_excl_aggr: "Виключено: 🟠 біржові, 🟣 RWA (🔴 спірні допущені — агресивний профіль)",
    strat_r_conc: "Фільтр концентрації: топ-холдер ≤ 50% (анти-rug)", strat_r_riskoff: "Risk-off: масова роздача китів → перелив сателіт → стейбл",
    strat_r_exit: "Вихід сателіта по драбині DCA-OUT (остаточний при E={a}%)", strat_rebalance: "🔁 Систематичний ребаланс, щомісяця, до цільових ваг",
    strat_bt_btn: "📈 Бектест портфеля (ребаланс vs бенчмарки)", strat_bt_calc: "📈 Рахуємо історію (ребаланс щомісяця)…",
    strat_bt_period: "Період", strat_bt_yr: "р.", strat_bt_mo: "міс.", bt_total: "Заг.", bt_vol: "Вол.",
    strat_bt_note: "Менший Max DD і вища Sharpe за рахунок диверсифікації — головна вигода інституційного підходу.",
  },
  en: {
    strat_btn: "Strategy", strat_title: "🏛 Institutional strategy", strat_building: "Building portfolio…",
    prof_aggressive: "Aggressive", prof_moderate: "Moderate", prof_conservative: "Conservative", prof_smartmoney: "Smart Money",
    prof_aggressive_desc: "Max alpha. Less passive base, large satellite (incl. contested assets), late exit (E=55%) — ride the whale longer. Higher return ↔ higher drawdown.",
    prof_moderate_desc: "Balanced. Passive BTC/ETH core + alpha satellite of quality assets, exit E=48%. Better risk-adjusted than a naive altcoin basket.",
    prof_conservative_desc: "Capital preservation. Large core + stablecoin buffer, small satellite (top quality), early exit (E=30%) — take profit fast, minimize drawdown.",
    prof_smartmoney_desc: "Pure whale-following. Almost no passive base — capital goes to assets where smart money is accumulating NOW (DCA-IN phase), weighted by signal strength. Exit strictly by the whale's ladder.",
    strat_core: "Core", strat_satellite: "Satellite", strat_stable: "Stable", strat_target: "target",
    strat_core_role: "Passive base — 🟢 CLARITY commodities (BTC/ETH), low risk, low turnover",
    strat_sat_role: "Active alpha — assets in whale accumulation phase (DCA-IN), risk-weighted + cap",
    strat_stable_role: "Dry powder / buffer — stablecoins, deploy into satellite on drawdowns (DCA)",
    strat_factors: "📐 Selection factors", strat_risk: "🛡 Risk management", strat_assets: "assets", strat_maxw: "max weight",
    strat_f_flow: "Flow (whale accumulation) → satellite selection & weight", strat_f_quality: "Quality (low holder concentration) → eligibility",
    strat_f_reg: "Regulatory (CLARITY 🟢 commodity) → core & satellite gate", strat_f_liq: "Liquidity (size) → core weight",
    strat_r_cap: "Per-asset cap: core ≤ {a}%, satellite ≤ {b}%", strat_r_excl: "Excluded from risk sleeves: 🔴 contested, 🟠 exchange, 🟣 RWA",
    strat_r_excl_aggr: "Excluded: 🟠 exchange, 🟣 RWA (🔴 contested allowed — aggressive profile)",
    strat_r_conc: "Concentration filter: top holder ≤ 50% (anti-rug)", strat_r_riskoff: "Risk-off: mass whale distribution → satellite → stable",
    strat_r_exit: "Satellite exit via DCA-OUT ladder (final at E={a}%)", strat_rebalance: "🔁 Systematic monthly rebalance to targets",
    strat_bt_btn: "📈 Portfolio backtest (rebalance vs benchmarks)", strat_bt_calc: "📈 Computing history (monthly rebalance)…",
    strat_bt_period: "Period", strat_bt_yr: "yr", strat_bt_mo: "mo", bt_total: "Total", bt_vol: "Vol",
    strat_bt_note: "Lower Max DD and higher Sharpe via diversification — the core benefit of the institutional approach.",
  },
  de: {
    strat_btn: "Strategie", strat_title: "🏛 Institutionelle Strategie", strat_building: "Portfolio wird erstellt…",
    prof_aggressive: "Aggressiv", prof_moderate: "Moderat", prof_conservative: "Konservativ", prof_smartmoney: "Smart Money",
    prof_aggressive_desc: "Maximale Alpha. Weniger passive Basis, großer Satellit (inkl. strittiger Assets), später Ausstieg (E=55%) — länger mit dem Wal mitfahren. Höhere Rendite ↔ höherer Drawdown.",
    prof_moderate_desc: "Ausgewogen. Passiver BTC/ETH-Kern + Alpha-Satellit aus Qualitätsassets, Ausstieg E=48%. Besseres Risiko-Rendite-Verhältnis als ein naiver Altcoin-Korb.",
    prof_conservative_desc: "Kapitalerhalt. Großer Kern + Stablecoin-Puffer, kleiner Satellit (Top-Qualität), früher Ausstieg (E=30%) — schnell Gewinn mitnehmen, Drawdown minimieren.",
    prof_smartmoney_desc: "Reines Wal-Folgen. Fast keine passive Basis — Kapital fließt in Assets, in denen Smart Money JETZT akkumuliert (DCA-IN-Phase), gewichtet nach Signalstärke. Ausstieg streng nach der Wal-Leiter.",
    strat_core: "Kern (Core)", strat_satellite: "Satellit", strat_stable: "Stable", strat_target: "Ziel",
    strat_core_role: "Passive Basis — 🟢 CLARITY-Rohstoffe (BTC/ETH), geringes Risiko, niedriger Umschlag",
    strat_sat_role: "Aktive Alpha — Assets in der Wal-Akkumulationsphase (DCA-IN), risikogewichtet + Cap",
    strat_stable_role: "Trockenpulver / Puffer — Stablecoins, bei Drawdowns in Satellit einsetzen (DCA)",
    strat_factors: "📐 Auswahlfaktoren", strat_risk: "🛡 Risikomanagement", strat_assets: "Assets", strat_maxw: "max. Gewicht",
    strat_f_flow: "Flow (Wal-Akkumulation) → Satellitenauswahl & -gewicht", strat_f_quality: "Qualität (geringe Halterkonzentration) → Zulassung",
    strat_f_reg: "Regulatorisch (CLARITY 🟢 Rohstoff) → Kern- & Satelliten-Gate", strat_f_liq: "Liquidität (Größe) → Kerngewicht",
    strat_r_cap: "Cap pro Asset: Kern ≤ {a}%, Satellit ≤ {b}%", strat_r_excl: "Aus Risiko-Sleeves ausgeschlossen: 🔴 strittig, 🟠 Börse, 🟣 RWA",
    strat_r_excl_aggr: "Ausgeschlossen: 🟠 Börse, 🟣 RWA (🔴 strittig erlaubt — aggressives Profil)",
    strat_r_conc: "Konzentrationsfilter: Top-Halter ≤ 50% (Anti-Rug)", strat_r_riskoff: "Risk-off: massive Wal-Distribution → Satellit → Stable",
    strat_r_exit: "Satelliten-Ausstieg über DCA-OUT-Leiter (final bei E={a}%)", strat_rebalance: "🔁 Systematisches monatliches Rebalancing auf Zielwerte",
    strat_bt_btn: "📈 Portfolio-Backtest (Rebalancing vs. Benchmarks)", strat_bt_calc: "📈 Historie wird berechnet (monatliches Rebalancing)…",
    strat_bt_period: "Zeitraum", strat_bt_yr: "J.", strat_bt_mo: "Mon.", bt_total: "Gesamt", bt_vol: "Vol",
    strat_bt_note: "Geringerer Max DD und höhere Sharpe durch Diversifikation — der Kernvorteil des institutionellen Ansatzes.",
  },
  fr: {
    strat_btn: "Stratégie", strat_title: "🏛 Stratégie institutionnelle", strat_building: "Construction du portefeuille…",
    prof_aggressive: "Agressive", prof_moderate: "Modérée", prof_conservative: "Conservatrice", prof_smartmoney: "Smart Money",
    prof_aggressive_desc: "Alpha max. Moins de base passive, gros satellite (y compris actifs contestés), sortie tardive (E=55%) — suivre la baleine plus longtemps. Rendement plus élevé ↔ drawdown plus élevé.",
    prof_moderate_desc: "Équilibrée. Cœur passif BTC/ETH + satellite alpha d'actifs de qualité, sortie E=48%. Meilleur rendement ajusté au risque qu'un panier d'altcoins naïf.",
    prof_conservative_desc: "Préservation du capital. Grand cœur + tampon de stablecoins, petit satellite (qualité supérieure), sortie précoce (E=30%) — prendre les profits vite, minimiser le drawdown.",
    prof_smartmoney_desc: "Pur suivi des baleines. Presque aucune base passive — le capital va vers les actifs où le smart money accumule MAINTENANT (phase DCA-IN), pondéré par la force du signal. Sortie strictement selon l'échelle de la baleine.",
    strat_core: "Cœur (Core)", strat_satellite: "Satellite", strat_stable: "Stable", strat_target: "cible",
    strat_core_role: "Base passive — 🟢 matières CLARITY (BTC/ETH), faible risque, faible rotation",
    strat_sat_role: "Alpha active — actifs en phase d'accumulation des baleines (DCA-IN), pondéré par le risque + plafond",
    strat_stable_role: "Poudre sèche / tampon — stablecoins, déployés dans le satellite lors des baisses (DCA)",
    strat_factors: "📐 Facteurs de sélection", strat_risk: "🛡 Gestion du risque", strat_assets: "actifs", strat_maxw: "poids max",
    strat_f_flow: "Flux (accumulation des baleines) → sélection et poids du satellite", strat_f_quality: "Qualité (faible concentration des détenteurs) → admissibilité",
    strat_f_reg: "Réglementaire (CLARITY 🟢 matière) → filtre cœur et satellite", strat_f_liq: "Liquidité (taille) → poids du cœur",
    strat_r_cap: "Plafond par actif : cœur ≤ {a}%, satellite ≤ {b}%", strat_r_excl: "Exclus des poches de risque : 🔴 contestés, 🟠 d'échange, 🟣 RWA",
    strat_r_excl_aggr: "Exclus : 🟠 d'échange, 🟣 RWA (🔴 contestés autorisés — profil agressif)",
    strat_r_conc: "Filtre de concentration : top détenteur ≤ 50% (anti-rug)", strat_r_riskoff: "Risk-off : distribution massive des baleines → satellite → stable",
    strat_r_exit: "Sortie du satellite via l'échelle DCA-OUT (finale à E={a}%)", strat_rebalance: "🔁 Rééquilibrage systématique mensuel vers les cibles",
    strat_bt_btn: "📈 Backtest du portefeuille (rééquilibrage vs benchmarks)", strat_bt_calc: "📈 Calcul de l'historique (rééquilibrage mensuel)…",
    strat_bt_period: "Période", strat_bt_yr: "ans", strat_bt_mo: "mois", bt_total: "Total", bt_vol: "Vol",
    strat_bt_note: "Drawdown max plus faible et Sharpe plus élevé grâce à la diversification — l'avantage clé de l'approche institutionnelle.",
  },
  ru: {
    strat_btn: "Стратегия", strat_title: "🏛 Институциональная стратегия", strat_building: "Строим портфель…",
    prof_aggressive: "Агрессивная", prof_moderate: "Умеренная", prof_conservative: "Консервативная", prof_smartmoney: "Смартмани",
    prof_aggressive_desc: "Макс. альфа. Меньше пассива, крупный сателлит (вкл. спорные активы), поздний выход (E=55%) — едем за движением кита дольше. Выше доходность ↔ выше просадка.",
    prof_moderate_desc: "Баланс. Пассивное ядро BTC/ETH + альфа-сателлит из качественных активов, выход E=48%. Лучше risk-adjusted, чем наивная корзина альтов.",
    prof_conservative_desc: "Сохранение капитала. Большое ядро + буфер стейблов, мало сателлита (топ-качество), ранний выход (E=30%) — фиксируем прибыль быстро, минимизируем просадку.",
    prof_smartmoney_desc: "Чистое следование за китом. Почти без пассива — капитал идёт в активы, где смарт-деньги СЕЙЧАС накапливают (фаза DCA-IN), вес по силе сигнала. Выход строго по лестнице кита.",
    strat_core: "Ядро (Core)", strat_satellite: "Сателлит", strat_stable: "Стейбл", strat_target: "цель",
    strat_core_role: "Пассивная база — 🟢 CLARITY-товары (BTC/ETH), низкий риск, низкий оборот",
    strat_sat_role: "Активная альфа — активы в фазе накопления кита (DCA-IN), риск-взвешивание + кэп",
    strat_stable_role: "Сухой порох / буфер — стейблкоины, доливаем в сателлит на просадках (DCA)",
    strat_factors: "📐 Факторы отбора", strat_risk: "🛡 Риск-менеджмент", strat_assets: "активов", strat_maxw: "макс. вес",
    strat_f_flow: "Flow (накопление китов) → отбор и вес сателлита", strat_f_quality: "Quality (низкая концентрация холдеров) → допуск",
    strat_f_reg: "Regulatory (CLARITY 🟢 товар) → гейт ядра и сателлита", strat_f_liq: "Liquidity (размер) → вес ядра",
    strat_r_cap: "Кэп на актив: ядро ≤ {a}%, сателлит ≤ {b}%", strat_r_excl: "Исключено из риск-сливов: 🔴 спорные, 🟠 биржевые, 🟣 RWA",
    strat_r_excl_aggr: "Исключено: 🟠 биржевые, 🟣 RWA (🔴 спорные допущены — агрессивный профиль)",
    strat_r_conc: "Фильтр концентрации: топ-холдер ≤ 50% (анти-rug)", strat_r_riskoff: "Risk-off: массовая раздача китов → перелив сателлит → стейбл",
    strat_r_exit: "Выход сателлита по лестнице DCA-OUT (финальный при E={a}%)", strat_rebalance: "🔁 Систематический ребаланс, ежемесячно, к целевым весам",
    strat_bt_btn: "📈 Бэктест портфеля (ребаланс vs бенчмарки)", strat_bt_calc: "📈 Считаем историю (ребаланс ежемесячно)…",
    strat_bt_period: "Период", strat_bt_yr: "г.", strat_bt_mo: "мес.", bt_total: "Всего", bt_vol: "Вол.",
    strat_bt_note: "Меньший Max DD и выше Sharpe за счёт диверсификации — главная выгода институционального подхода.",
  },
};
for (const l of Object.keys(STRAT_TR) as Lang[]) Object.assign(TR[l], STRAT_TR[l]);

const STRAT_TR2: Record<Lang, Record<string, string>> = {
  uk: { ph_accum: "Накопич.", ph_markup: "Розгін", ph_distr: "Роздача", ph_exit: "Вихід", strat_inspired: "Vanguard (пасивне ядро) · BlackRock (альфа-сателіт) · Fidelity (кеш-менеджмент)" },
  en: { ph_accum: "Accum.", ph_markup: "Markup", ph_distr: "Distrib.", ph_exit: "Exit", strat_inspired: "Vanguard (passive core) · BlackRock (alpha satellite) · Fidelity (cash mgmt)" },
  de: { ph_accum: "Akkum.", ph_markup: "Anstieg", ph_distr: "Verteil.", ph_exit: "Ausstieg", strat_inspired: "Vanguard (passiver Kern) · BlackRock (Alpha-Satellit) · Fidelity (Cash-Mgmt)" },
  fr: { ph_accum: "Accum.", ph_markup: "Hausse", ph_distr: "Distrib.", ph_exit: "Sortie", strat_inspired: "Vanguard (cœur passif) · BlackRock (satellite alpha) · Fidelity (gestion cash)" },
  ru: { ph_accum: "Накопл.", ph_markup: "Разгон", ph_distr: "Раздача", ph_exit: "Выход", strat_inspired: "Vanguard (пассивное ядро) · BlackRock (альфа-сателлит) · Fidelity (кеш-менеджмент)" },
};
for (const l of Object.keys(STRAT_TR2) as Lang[]) Object.assign(TR[l], STRAT_TR2[l]);

const CLARITY_TR: Record<Lang, Record<string, string>> = {
  uk: { cl_commodity: "Товар", "cl_commodity-likely": "Ймовірно товар", cl_stablecoin: "Стейблкоїн", cl_rwa: "RWA", cl_exchange: "Біржовий", cl_contested: "Спірний", cl_unknown: "Залежить" },
  en: { cl_commodity: "Commodity", "cl_commodity-likely": "Likely commodity", cl_stablecoin: "Stablecoin", cl_rwa: "RWA", cl_exchange: "Exchange", cl_contested: "Contested", cl_unknown: "Depends" },
  de: { cl_commodity: "Rohstoff", "cl_commodity-likely": "Wahrsch. Rohstoff", cl_stablecoin: "Stablecoin", cl_rwa: "RWA", cl_exchange: "Börse", cl_contested: "Strittig", cl_unknown: "Hängt ab" },
  fr: { cl_commodity: "Matière", "cl_commodity-likely": "Probable matière", cl_stablecoin: "Stablecoin", cl_rwa: "RWA", cl_exchange: "Échange", cl_contested: "Contesté", cl_unknown: "Dépend" },
  ru: { cl_commodity: "Товар", "cl_commodity-likely": "Вероятно товар", cl_stablecoin: "Стейблкоин", cl_rwa: "RWA", cl_exchange: "Биржевой", cl_contested: "Спорный", cl_unknown: "Зависит" },
};
for (const l of Object.keys(CLARITY_TR) as Lang[]) Object.assign(TR[l], CLARITY_TR[l]);
Object.assign(TR.uk, { strat_bt_dynamic: "🎯 Динамічна (вихід за китом)", strat_bt_static: "тримання" });
Object.assign(TR.en, { strat_bt_dynamic: "🎯 Dynamic (whale-exit)", strat_bt_static: "hold" });
Object.assign(TR.de, { strat_bt_dynamic: "🎯 Dynamisch (Wal-Ausstieg)", strat_bt_static: "Halten" });
Object.assign(TR.fr, { strat_bt_dynamic: "🎯 Dynamique (sortie baleine)", strat_bt_static: "détention" });
Object.assign(TR.ru, { strat_bt_dynamic: "🎯 Динамическая (выход за китом)", strat_bt_static: "удержание" });

const COORD_TR: Record<Lang, Record<string, string>> = {
  uk: { coord_loading: "Аналіз координації…", coord_title: "Впевненість координації", coord_funded: "Фінансує гаманців", coord_reach: "Охоплення", coord_capital: "Капітал", coord_sync: "Синхронність", coord_consistency: "Консистентність", coord_note: "Доказова оцінка з on-chain фінансування — для будь-якого гаманця", coord_high: "Координатор", coord_likely: "Ймовірний координатор", coord_weak: "Слабкий сигнал", coord_none: "Не координатор" },
  en: { coord_loading: "Analyzing coordination…", coord_title: "Coordination confidence", coord_funded: "Funds wallets", coord_reach: "Reach", coord_capital: "Capital", coord_sync: "Synchronicity", coord_consistency: "Consistency", coord_note: "Evidence-based score from on-chain funding — for any wallet", coord_high: "Coordinator", coord_likely: "Likely coordinator", coord_weak: "Weak signal", coord_none: "Not a coordinator" },
  de: { coord_loading: "Koordination wird analysiert…", coord_title: "Koordinationsvertrauen", coord_funded: "Finanziert Wallets", coord_reach: "Reichweite", coord_capital: "Kapital", coord_sync: "Synchronität", coord_consistency: "Konsistenz", coord_note: "Evidenzbasierter Score aus On-Chain-Finanzierung — für jede Wallet", coord_high: "Koordinator", coord_likely: "Wahrsch. Koordinator", coord_weak: "Schwaches Signal", coord_none: "Kein Koordinator" },
  fr: { coord_loading: "Analyse de la coordination…", coord_title: "Confiance de coordination", coord_funded: "Finance portefeuilles", coord_reach: "Portée", coord_capital: "Capital", coord_sync: "Synchronicité", coord_consistency: "Cohérence", coord_note: "Score fondé sur le financement on-chain — pour tout portefeuille", coord_high: "Coordinateur", coord_likely: "Coordinateur probable", coord_weak: "Signal faible", coord_none: "Pas un coordinateur" },
  ru: { coord_loading: "Анализ координации…", coord_title: "Уверенность координации", coord_funded: "Финансирует кошельков", coord_reach: "Охват", coord_capital: "Капитал", coord_sync: "Синхронность", coord_consistency: "Консистентность", coord_note: "Доказательная оценка по on-chain финансированию — для любого кошелька", coord_high: "Координатор", coord_likely: "Вероятный координатор", coord_weak: "Слабый сигнал", coord_none: "Не координатор" },
};
for (const l of Object.keys(COORD_TR) as Lang[]) Object.assign(TR[l], COORD_TR[l]);
// ml_auc, а не ml_acc: позитивів у вибірці 12%, тож «завжди НІ» дало б 88% точності — вища за ту,
// що показувала модель. Точність тут вводила в оману; AUC від перекосу класів не залежить.
Object.assign(TR.uk, { ml_acc: "точність CV", ml_auc: "AUC (CV)", ml_auc_scale: " з 1.0 · монетка 0.50", ml_calibrating: "модель калібрується", ml_acc_title: "AUC на крос-валідації (out-of-sample): наскільки впевнено модель ставить прибуткові гаманці вище за збиткові. 0.50 — монетка, 1.0 — ідеал. Точність тут не показуємо: позитивів лише ~12%, тож «завжди НІ» дало б 88% і виглядало б краще за будь-яку корисну модель. «Калібрується» — замало позитивних прикладів для чесної валідації." });
Object.assign(TR.en, { ml_acc: "CV accuracy", ml_auc: "AUC (CV)", ml_auc_scale: " of 1.0 · coin flip 0.50", ml_calibrating: "model calibrating", ml_acc_title: "Cross-validated AUC (out-of-sample): how reliably the model ranks profitable wallets above unprofitable ones. 0.50 is a coin flip, 1.0 is perfect. Accuracy is deliberately not shown: only ~12% of samples are positive, so 'always no' would score 88% and look better than any useful model. 'Calibrating' — too few positive examples for honest validation yet." });
Object.assign(TR.de, { ml_acc: "CV-Genauigkeit", ml_auc: "AUC (CV)", ml_auc_scale: " von 1,0 · Münzwurf 0,50", ml_calibrating: "Modell kalibriert", ml_acc_title: "Kreuzvalidierte AUC (Out-of-Sample): wie zuverlässig das Modell profitable Wallets über unprofitable stellt. 0,50 = Münzwurf, 1,0 = perfekt. Genauigkeit wird bewusst nicht gezeigt: nur ~12% der Beispiele sind positiv, «immer nein» käme auf 88%. 'Kalibriert' — noch zu wenige positive Beispiele." });
Object.assign(TR.fr, { ml_acc: "précision CV", ml_auc: "AUC (CV)", ml_auc_scale: " sur 1,0 · pile ou face 0,50", ml_calibrating: "modèle en calibration", ml_acc_title: "AUC en validation croisée (hors échantillon) : à quel point le modèle classe les portefeuilles rentables au-dessus des autres. 0,50 = pile ou face, 1,0 = parfait. La précision n'est pas affichée : seuls ~12% des exemples sont positifs, donc « toujours non » ferait 88%. « En calibration » — trop peu d'exemples positifs." });
Object.assign(TR.ru, { ml_acc: "точность CV", ml_auc: "AUC (CV)", ml_auc_scale: " из 1.0 · монетка 0.50", ml_calibrating: "модель калибруется", ml_acc_title: "AUC на кросс-валидации (out-of-sample): насколько уверенно модель ставит прибыльные кошельки выше убыточных. 0.50 — монетка, 1.0 — идеал. Точность не показываем: положительных примеров лишь ~12%, поэтому «всегда нет» дало бы 88% и выглядело бы лучше любой полезной модели. «Калибруется» — мало положительных примеров." });

const MARKET_TR: Record<Lang, Record<string, string>> = {
  uk: { mc_title: "Ринковий контекст", mc_fear: "Страх/Жадібність", mc_mcap: "Капіталізація", mc_accumulate: "Накопичення", mc_neutral: "Нейтрально", mc_derisk: "Зниження ризику", mc_tilt_accumulate: "Макро сприяє розгортанню капіталу — можна збільшувати сателіт (альфу).", mc_tilt_neutral: "Макро нейтральне — тримати цільові ваги профілю.", mc_tilt_derisk: "Макро перегріте — зменшити сателіт, нарощувати стейбл-буфер.", mc_rot: "Ротація капіталу", mc_rot_btc_to_eth: "BTC → ETH (капітал тече в ETH/альти)", mc_rot_eth_to_btc: "ETH → BTC (risk-off)", mc_rot_neutral: "нейтральна", mc_rot_collecting: "збираємо історію домінації (2-3 дні)", mc_rot_onchain_acc: "кити накопичують", mc_rot_onchain_dist: "кити розподіляють" },
  en: { mc_title: "Market context", mc_fear: "Fear/Greed", mc_mcap: "Mkt cap", mc_accumulate: "Accumulate", mc_neutral: "Neutral", mc_derisk: "De-risk", mc_tilt_accumulate: "Macro favors deploying capital — you can increase the satellite (alpha).", mc_tilt_neutral: "Macro neutral — hold the profile's target weights.", mc_tilt_derisk: "Macro overheated — reduce satellite, grow the stable buffer.", mc_rot: "Capital rotation", mc_rot_btc_to_eth: "BTC → ETH (capital flowing into ETH/alts)", mc_rot_eth_to_btc: "ETH → BTC (risk-off)", mc_rot_neutral: "neutral", mc_rot_collecting: "collecting dominance history (2-3 days)", mc_rot_onchain_acc: "whales accumulating", mc_rot_onchain_dist: "whales distributing" },
  de: { mc_title: "Marktkontext", mc_fear: "Angst/Gier", mc_mcap: "Marktkap.", mc_accumulate: "Akkumulieren", mc_neutral: "Neutral", mc_derisk: "Risiko senken", mc_tilt_accumulate: "Makro begünstigt Kapitaleinsatz — Satellit (Alpha) kann erhöht werden.", mc_tilt_neutral: "Makro neutral — Zielgewichte des Profils halten.", mc_tilt_derisk: "Makro überhitzt — Satellit reduzieren, Stable-Puffer ausbauen.", mc_rot: "Kapitalrotation", mc_rot_btc_to_eth: "BTC → ETH (Kapital fließt in ETH/Alts)", mc_rot_eth_to_btc: "ETH → BTC (Risk-off)", mc_rot_neutral: "neutral", mc_rot_collecting: "sammle Dominanz-Historie (2-3 Tage)", mc_rot_onchain_acc: "Wale akkumulieren", mc_rot_onchain_dist: "Wale verteilen" },
  fr: { mc_title: "Contexte de marché", mc_fear: "Peur/Avidité", mc_mcap: "Cap. marché", mc_accumulate: "Accumuler", mc_neutral: "Neutre", mc_derisk: "Réduire le risque", mc_tilt_accumulate: "La macro favorise le déploiement — on peut augmenter le satellite (alpha).", mc_tilt_neutral: "Macro neutre — garder les poids cibles du profil.", mc_tilt_derisk: "Macro en surchauffe — réduire le satellite, renforcer le tampon stable.", mc_rot: "Rotation du capital", mc_rot_btc_to_eth: "BTC → ETH (capital vers ETH/alts)", mc_rot_eth_to_btc: "ETH → BTC (risk-off)", mc_rot_neutral: "neutre", mc_rot_collecting: "collecte de l'historique de dominance (2-3 jours)", mc_rot_onchain_acc: "baleines en accumulation", mc_rot_onchain_dist: "baleines en distribution" },
  ru: { mc_title: "Рыночный контекст", mc_fear: "Страх/Жадность", mc_mcap: "Капитализация", mc_accumulate: "Накопление", mc_neutral: "Нейтрально", mc_derisk: "Снижение риска", mc_tilt_accumulate: "Макро способствует развёртыванию капитала — можно увеличивать сателлит (альфу).", mc_tilt_neutral: "Макро нейтрально — держать целевые веса профиля.", mc_tilt_derisk: "Макро перегрето — уменьшить сателлит, наращивать стейбл-буфер.", mc_rot: "Ротация капитала", mc_rot_btc_to_eth: "BTC → ETH (капитал в ETH/альты)", mc_rot_eth_to_btc: "ETH → BTC (risk-off)", mc_rot_neutral: "нейтральная", mc_rot_collecting: "собираем историю доминации (2-3 дня)", mc_rot_onchain_acc: "киты накапливают", mc_rot_onchain_dist: "киты распределяют" },
};
for (const l of Object.keys(MARKET_TR) as Lang[]) Object.assign(TR[l], MARKET_TR[l]);
Object.assign(TR.uk, { orc_title: "Крос-актив оркестратори", orc_assets: "активів" });
Object.assign(TR.en, { orc_title: "Cross-asset orchestrators", orc_assets: "assets" });
Object.assign(TR.de, { orc_title: "Cross-Asset-Orchestratoren", orc_assets: "Assets" });
Object.assign(TR.fr, { orc_title: "Orchestrateurs multi-actifs", orc_assets: "actifs" });
Object.assign(TR.ru, { orc_title: "Кросс-актив оркестраторы", orc_assets: "активов" });
Object.assign(TR.uk, { disclaimer_note: "⚠️ Лише аналітична інформація для дослідження. НЕ є фінансовою порадою. Дані — з публічного блокчейну. Wallet Radar не має доступу до ваших коштів." });
Object.assign(TR.en, { disclaimer_note: "⚠️ Analytical research information only. NOT financial advice. Data from public blockchain. Wallet Radar never has access to your funds." });
Object.assign(TR.de, { disclaimer_note: "⚠️ Nur analytische Recherche-Informationen. KEINE Finanzberatung. Daten aus öffentlicher Blockchain. Wallet Radar hat keinen Zugriff auf Ihre Mittel." });
Object.assign(TR.fr, { disclaimer_note: "⚠️ Informations analytiques de recherche uniquement. PAS un conseil financier. Données de la blockchain publique. Wallet Radar n'a jamais accès à vos fonds." });
Object.assign(TR.ru, { disclaimer_note: "⚠️ Только аналитическая информация для исследования. НЕ финансовый совет. Данные из публичного блокчейна. Wallet Radar не имеет доступа к вашим средствам." });
export function clarityShortL(status?: string): string { const k = "cl_" + (status || "unknown"); const v = t(k); return v === k ? (status || "") : v; }

const GRAPH2_TR: Record<Lang, Record<string, string>> = {
  uk: { g_focus: "Фокус — його павутина", g_show_all: "← Усі зв'язки", g_connected_entities: "Пов'язані сутності", g_connected_wallets: "Пов'язані гаманці", g_balance: "Баланс (демо)", g_events: "Подій", g_networks: "Мережі", g_wallet_one: "Гаманець", g_entity_one: "Сутність" },
  en: { g_focus: "Focus — its web", g_show_all: "← All connections", g_connected_entities: "Connected entities", g_connected_wallets: "Connected wallets", g_balance: "Balance (demo)", g_events: "Events", g_networks: "Networks", g_wallet_one: "Wallet", g_entity_one: "Entity" },
  de: { g_focus: "Fokus — sein Netz", g_show_all: "← Alle Verbindungen", g_connected_entities: "Verbundene Entitäten", g_connected_wallets: "Verbundene Wallets", g_balance: "Guthaben (Demo)", g_events: "Ereignisse", g_networks: "Netzwerke", g_wallet_one: "Wallet", g_entity_one: "Entität" },
  fr: { g_focus: "Focus — sa toile", g_show_all: "← Toutes les connexions", g_connected_entities: "Entités connectées", g_connected_wallets: "Portefeuilles connectés", g_balance: "Solde (démo)", g_events: "Événements", g_networks: "Réseaux", g_wallet_one: "Portefeuille", g_entity_one: "Entité" },
  ru: { g_focus: "Фокус — его паутина", g_show_all: "← Все связи", g_connected_entities: "Связанные сущности", g_connected_wallets: "Связанные кошельки", g_balance: "Баланс (демо)", g_events: "Событий", g_networks: "Сети", g_wallet_one: "Кошелёк", g_entity_one: "Сущность" },
};
for (const l of Object.keys(GRAPH2_TR) as Lang[]) Object.assign(TR[l], GRAPH2_TR[l]);

const PRICE_TR: Record<Lang, Record<string, string>> = {
  uk: { prices_word: "Ціни", prices_offline: "резерв", prices_src: "Джерело: CoinGecko · оновлюється щохвилини", g_holdings: "Активи (холдинги)" },
  en: { prices_word: "Prices", prices_offline: "cached", prices_src: "Source: CoinGecko · refreshed every minute", g_holdings: "Holdings" },
  de: { prices_word: "Preise", prices_offline: "Cache", prices_src: "Quelle: CoinGecko · jede Minute aktualisiert", g_holdings: "Bestände" },
  fr: { prices_word: "Prix", prices_offline: "cache", prices_src: "Source : CoinGecko · actualisé chaque minute", g_holdings: "Avoirs" },
  ru: { prices_word: "Цены", prices_offline: "резерв", prices_src: "Источник: CoinGecko · обновляется ежеминутно", g_holdings: "Активы (холдинги)" },
};
for (const l of Object.keys(PRICE_TR) as Lang[]) Object.assign(TR[l], PRICE_TR[l]);

const PF_TR: Record<Lang, Record<string, string>> = {
  uk: { pf_title: "Портфель гаманця", pf_open: "Активи / портфель", pf_total: "Загальна вартість", pf_allocation: "Розподіл активів", pf_assets: "Активи", pf_share: "Частка", pf_price: "Ціна", pf_amount: "Кількість", pf_click_hint: "Натисни, щоб переглянути активи", pf_other: "Інше" },
  en: { pf_title: "Wallet portfolio", pf_open: "Assets / portfolio", pf_total: "Total value", pf_allocation: "Asset allocation", pf_assets: "Assets", pf_share: "Share", pf_price: "Price", pf_amount: "Amount", pf_click_hint: "Click to view assets", pf_other: "Other" },
  de: { pf_title: "Wallet-Portfolio", pf_open: "Vermögenswerte / Portfolio", pf_total: "Gesamtwert", pf_allocation: "Vermögensaufteilung", pf_assets: "Vermögenswerte", pf_share: "Anteil", pf_price: "Preis", pf_amount: "Menge", pf_click_hint: "Zum Anzeigen der Assets klicken", pf_other: "Sonstige" },
  fr: { pf_title: "Portefeuille", pf_open: "Actifs / portefeuille", pf_total: "Valeur totale", pf_allocation: "Répartition des actifs", pf_assets: "Actifs", pf_share: "Part", pf_price: "Prix", pf_amount: "Quantité", pf_click_hint: "Cliquez pour voir les actifs", pf_other: "Autres" },
  ru: { pf_title: "Портфель кошелька", pf_open: "Активы / портфель", pf_total: "Общая стоимость", pf_allocation: "Распределение активов", pf_assets: "Активы", pf_share: "Доля", pf_price: "Цена", pf_amount: "Количество", pf_click_hint: "Нажмите, чтобы посмотреть активы", pf_other: "Другое" },
};
for (const l of Object.keys(PF_TR) as Lang[]) Object.assign(TR[l], PF_TR[l]);

const SEARCH_TR: Record<Lang, Record<string, string>> = {
  uk: { search_ph: "Пошук: адреса (будь-яка мережа) або будь-який актив…", search_assets: "Активи", search_wallets: "Гаманці", search_open_address: "Відкрити адресу", search_no_results: "Нічого не знайдено", search_loading: "Шукаю серед усіх активів…", price_unknown: "Ринкова ціна недоступна", pf_chart: "Баланс за весь час", holders_title: "Топ-тримачі", holders_note: "Дані демонстраційні (топ-тримачі змодельовані). Реальні — з Arkham / Moralis.", um_rank: "Ранг:", um_mcap: "Капа:", cbt_title: "Бектест сигналу (кит зайшов)", cbt_excess: "Надлишок над ETH", cbt_events: "Опрацьовано подій", cbt_note: "Подієвий бектест по цьому активу; надлишкова дохідність над ETH на горизонті.", cnet_title: "Скоординовані мережі з цим активом" },
  en: { search_ph: "Search: address (any chain) or any asset…", search_assets: "Assets", search_wallets: "Wallets", search_open_address: "Open address", search_no_results: "Nothing found", search_loading: "Searching all assets…", price_unknown: "Market price unavailable", pf_chart: "All-time balance", holders_title: "Top holders", holders_note: "Demo data (top holders simulated). Real data from Arkham / Moralis.", um_rank: "Rank:", um_mcap: "MCap:", cbt_title: "Signal backtest (whale entered)", cbt_excess: "Excess over ETH", cbt_events: "Events resolved", cbt_note: "Event backtest for this asset; excess return over ETH across the horizon.", cnet_title: "Coordinated networks holding this asset" },
  de: { search_ph: "Suche: Adresse (jede Chain) oder beliebiges Asset…", search_assets: "Assets", search_wallets: "Wallets", search_open_address: "Adresse öffnen", search_no_results: "Nichts gefunden", search_loading: "Suche in allen Assets…", price_unknown: "Marktpreis nicht verfügbar", pf_chart: "Guthabenverlauf", holders_title: "Top-Halter", holders_note: "Demodaten (Top-Halter simuliert). Echte Daten von Arkham / Moralis." },
  fr: { search_ph: "Recherche : adresse (toute chaîne) ou tout actif…", search_assets: "Actifs", search_wallets: "Portefeuilles", search_open_address: "Ouvrir l'adresse", search_no_results: "Aucun résultat", search_loading: "Recherche parmi tous les actifs…", price_unknown: "Prix de marché indisponible", pf_chart: "Solde sur toute la période", holders_title: "Principaux détenteurs", holders_note: "Données de démonstration (détenteurs simulés). Données réelles via Arkham / Moralis." },
  ru: { search_ph: "Поиск: адрес (любая сеть) или любой актив…", search_assets: "Активы", search_wallets: "Кошельки", search_open_address: "Открыть адрес", search_no_results: "Ничего не найдено", search_loading: "Ищу среди всех активов…", price_unknown: "Рыночная цена недоступна", pf_chart: "Баланс за всё время", holders_title: "Топ-держатели", holders_note: "Демоданные (топ-держатели смоделированы). Реальные — из Arkham / Moralis." },
};
for (const l of Object.keys(SEARCH_TR) as Lang[]) Object.assign(TR[l], SEARCH_TR[l]);

const GSEL_TR: Record<Lang, Record<string, string>> = {
  uk: { g_all_wallets: "Усі гаманці", graph_del_title: "Прибрати гаманець зі списку графа" }, en: { g_all_wallets: "All wallets", graph_del_title: "Remove wallet from graph list" },
  de: { g_all_wallets: "Alle Wallets", graph_del_title: "Wallet aus Graph-Liste entfernen" }, fr: { g_all_wallets: "Tous les portefeuilles", graph_del_title: "Retirer le portefeuille de la liste" }, ru: { g_all_wallets: "Все кошельки", graph_del_title: "Убрать кошелёк из списка графа" },
};
for (const l of Object.keys(GSEL_TR) as Lang[]) Object.assign(TR[l], GSEL_TR[l]);

const TRACER_TR: Record<Lang, Record<string, string>> = {
  uk: { tracer_btn: "Tracer", tracer_title: "Tracer — трасування коштів", tracer_source: "Джерело", tracer_hint: "Демо-трасування (шлях змодельовано). Реальні потоки — з Arkham Tracer / Moralis.", action_transfer: "Переказ", action_bridge: "Міст (між мережами)", action_swap: "Обмін", action_cex: "Депозит на біржу", tracer_open: "🧭 Трасувати кошти" },
  en: { tracer_btn: "Tracer", tracer_title: "Tracer — fund flow tracing", tracer_source: "Source", tracer_hint: "Demo trace (path simulated). Real flows from Arkham Tracer / Moralis.", action_transfer: "Transfer", action_bridge: "Bridge (cross-chain)", action_swap: "Swap", action_cex: "Exchange deposit", tracer_open: "🧭 Trace funds" },
  de: { tracer_btn: "Tracer", tracer_title: "Tracer — Geldfluss-Verfolgung", tracer_source: "Quelle", tracer_hint: "Demo-Trace (Pfad simuliert). Echte Flüsse von Arkham Tracer / Moralis.", action_transfer: "Überweisung", action_bridge: "Bridge (Cross-Chain)", action_swap: "Tausch", action_cex: "Börsen-Einzahlung", tracer_open: "🧭 Geld verfolgen" },
  fr: { tracer_btn: "Tracer", tracer_title: "Tracer — suivi des flux", tracer_source: "Source", tracer_hint: "Trace de démo (chemin simulé). Flux réels via Arkham Tracer / Moralis.", action_transfer: "Transfert", action_bridge: "Pont (cross-chain)", action_swap: "Échange", action_cex: "Dépôt sur plateforme", tracer_open: "🧭 Tracer les fonds" },
  ru: { tracer_btn: "Tracer", tracer_title: "Tracer — трассировка средств", tracer_source: "Источник", tracer_hint: "Демо-трассировка (путь смоделирован). Реальные потоки — из Arkham Tracer / Moralis.", action_transfer: "Перевод", action_bridge: "Мост (между сетями)", action_swap: "Обмен", action_cex: "Депозит на биржу", tracer_open: "🧭 Трассировать средства" },
};
for (const l of Object.keys(TRACER_TR) as Lang[]) Object.assign(TR[l], TRACER_TR[l]);

const ALERT_TR: Record<Lang, Record<string, string>> = {
  uk: { alerts_btn: "Сповіщення", alerts_title: "Сповіщення (правила)", alerts_rules_h: "Активні правила", alerts_add: "Додати правило", alerts_minusd: "мін. $", alerts_log_h: "Спрацювання", alerts_empty: "Правил ще немає.", alerts_no_hits: "Поки без спрацювань.", alerts_triggered: "Спрацювало правило", alerts_any: "будь-який", alerts_target_ph: "напр. ETH або назва гаманця" },
  en: { alerts_btn: "Alerts", alerts_title: "Alerts (rules)", alerts_rules_h: "Active rules", alerts_add: "Add rule", alerts_minusd: "min $", alerts_log_h: "Triggers", alerts_empty: "No rules yet.", alerts_no_hits: "No triggers yet.", alerts_triggered: "Alert triggered", alerts_any: "any", alerts_target_ph: "e.g. ETH or wallet name" },
  de: { alerts_btn: "Alerts", alerts_title: "Alerts (Regeln)", alerts_rules_h: "Aktive Regeln", alerts_add: "Regel hinzufügen", alerts_minusd: "min. $", alerts_log_h: "Auslösungen", alerts_empty: "Noch keine Regeln.", alerts_no_hits: "Noch keine Auslösungen.", alerts_triggered: "Regel ausgelöst", alerts_any: "beliebig", alerts_target_ph: "z. B. ETH oder Wallet-Name" },
  fr: { alerts_btn: "Alertes", alerts_title: "Alertes (règles)", alerts_rules_h: "Règles actives", alerts_add: "Ajouter une règle", alerts_minusd: "min $", alerts_log_h: "Déclenchements", alerts_empty: "Aucune règle.", alerts_no_hits: "Aucun déclenchement.", alerts_triggered: "Règle déclenchée", alerts_any: "tout", alerts_target_ph: "ex. ETH ou nom du portefeuille" },
  ru: { alerts_btn: "Уведомления", alerts_title: "Уведомления (правила)", alerts_rules_h: "Активные правила", alerts_add: "Добавить правило", alerts_minusd: "мин. $", alerts_log_h: "Срабатывания", alerts_empty: "Правил пока нет.", alerts_no_hits: "Пока без срабатываний.", alerts_triggered: "Сработало правило", alerts_any: "любой", alerts_target_ph: "напр. ETH или имя кошелька" },
};
for (const l of Object.keys(ALERT_TR) as Lang[]) Object.assign(TR[l], ALERT_TR[l]);

const ENTITY_TR: Record<Lang, Record<string, string>> = {
  uk: { ent_profile_btn: "📂 Профіль сутності", ent_title: "Профіль сутності", ent_balance: "Сукупний баланс", ent_addresses: "Адреси сутності", ent_interacts: "Взаємодіє з (ваші гаманці)" },
  en: { ent_profile_btn: "📂 Entity profile", ent_title: "Entity profile", ent_balance: "Total balance", ent_addresses: "Entity addresses", ent_interacts: "Interacts with (your wallets)" },
  de: { ent_profile_btn: "📂 Entitätsprofil", ent_title: "Entitätsprofil", ent_balance: "Gesamtguthaben", ent_addresses: "Adressen der Entität", ent_interacts: "Interagiert mit (Ihre Wallets)" },
  fr: { ent_profile_btn: "📂 Profil de l'entité", ent_title: "Profil de l'entité", ent_balance: "Solde total", ent_addresses: "Adresses de l'entité", ent_interacts: "Interagit avec (vos portefeuilles)" },
  ru: { ent_profile_btn: "📂 Профиль сущности", ent_title: "Профиль сущности", ent_balance: "Совокупный баланс", ent_addresses: "Адреса сущности", ent_interacts: "Взаимодействует с (ваши кошельки)" },
};
for (const l of Object.keys(ENTITY_TR) as Lang[]) Object.assign(TR[l], ENTITY_TR[l]);

const DASH_TR: Record<Lang, Record<string, string>> = {
  uk: { dash_btn: "Дашборд", dash_title: "Мій дашборд", dash_add: "Додати віджет", dash_widget_total: "Сукупна вартість", dash_widget_price: "Ціна активу", dash_widget_wallet: "Баланс гаманця", dash_target_ph: "ціль: BTC / назва гаманця", dash_empty: "Немає віджетів. Додайте перший нижче." },
  en: { dash_btn: "Dashboard", dash_title: "My dashboard", dash_add: "Add widget", dash_widget_total: "Total value", dash_widget_price: "Asset price", dash_widget_wallet: "Wallet balance", dash_target_ph: "target: BTC / wallet name", dash_empty: "No widgets. Add your first below." },
  de: { dash_btn: "Dashboard", dash_title: "Mein Dashboard", dash_add: "Widget hinzufügen", dash_widget_total: "Gesamtwert", dash_widget_price: "Asset-Preis", dash_widget_wallet: "Wallet-Guthaben", dash_target_ph: "Ziel: BTC / Wallet-Name", dash_empty: "Keine Widgets. Fügen Sie unten hinzu." },
  fr: { dash_btn: "Tableau de bord", dash_title: "Mon tableau de bord", dash_add: "Ajouter un widget", dash_widget_total: "Valeur totale", dash_widget_price: "Prix de l'actif", dash_widget_wallet: "Solde du portefeuille", dash_target_ph: "cible : BTC / nom du portefeuille", dash_empty: "Aucun widget. Ajoutez le premier ci-dessous." },
  ru: { dash_btn: "Дашборд", dash_title: "Мой дашборд", dash_add: "Добавить виджет", dash_widget_total: "Совокупная стоимость", dash_widget_price: "Цена актива", dash_widget_wallet: "Баланс кошелька", dash_target_ph: "цель: BTC / имя кошелька", dash_empty: "Нет виджетов. Добавьте первый ниже." },
};
for (const l of Object.keys(DASH_TR) as Lang[]) Object.assign(TR[l], DASH_TR[l]);

const CONN_TR: Record<Lang, Record<string, string>> = {
  uk: { conn_live: "Підключено (live)", conn_reconnect: "Перепідключення…" },
  en: { conn_live: "Connected (live)", conn_reconnect: "Reconnecting…" },
  de: { conn_live: "Verbunden (live)", conn_reconnect: "Wiederverbinden…" },
  fr: { conn_live: "Connecté (live)", conn_reconnect: "Reconnexion…" },
  ru: { conn_live: "Подключено (live)", conn_reconnect: "Переподключение…" },
};
for (const l of Object.keys(CONN_TR) as Lang[]) Object.assign(TR[l], CONN_TR[l]);

const PF2_TR: Record<Lang, Record<string, string>> = {
  uk: { pf_recent_events: "Останні події гаманця" }, en: { pf_recent_events: "Recent wallet events" },
  de: { pf_recent_events: "Letzte Wallet-Ereignisse" }, fr: { pf_recent_events: "Événements récents du portefeuille" }, ru: { pf_recent_events: "Последние события кошелька" },
};
for (const l of Object.keys(PF2_TR) as Lang[]) Object.assign(TR[l], PF2_TR[l]);

const CONC_TR: Record<Lang, Record<string, string>> = {
  uk: { conc_btn: "Концентрація", conc_title: "Концентрація власників токена", conc_ph: "Контракт ERC-20 0x… (Ethereum)", conc_analyze: "Аналіз", conc_hint: "Вставте контракт токена (Ethereum) → Аналіз. Покаже, скільки сукупно тримають топ-власники й у яких активах сидять. Дані публічні (Ethplorer).", conc_loading: "Завантаження…", conc_err: "Помилка", conc_bad: "Невірний контракт (треба 0x… 40 hex)", conc_top50: "Топ-50 сукупно", conc_reach: "Поріг на № гаманців", conc_assets: "Активи", conc_track: "🐋 Стежити", conc_export: "⬇ Експорт JSON", conc_export_csv: "Експорт CSV", conc_who_holds: "Хто тримає (за категоріями)" },
  en: { conc_btn: "Concentration", conc_title: "Token holder concentration", conc_ph: "ERC-20 contract 0x… (Ethereum)", conc_analyze: "Analyze", conc_hint: "Paste a token contract (Ethereum) → Analyze. Shows how much top holders collectively hold and which assets they sit in. Public data (Ethplorer).", conc_loading: "Loading…", conc_err: "Error", conc_bad: "Invalid contract (need 0x… 40 hex)", conc_top50: "Top-50 combined", conc_reach: "Threshold at # wallets", conc_assets: "Assets", conc_track: "🐋 Track", conc_export: "⬇ Export JSON", conc_export_csv: "Export CSV", conc_who_holds: "Who holds (by category)" },
  de: { conc_btn: "Konzentration", conc_title: "Halterkonzentration des Tokens", conc_ph: "ERC-20-Vertrag 0x… (Ethereum)", conc_analyze: "Analysieren", conc_hint: "Token-Vertrag (Ethereum) einfügen → Analysieren. Zeigt den Anteil der Top-Halter und ihre Assets. Öffentliche Daten (Ethplorer).", conc_loading: "Laden…", conc_err: "Fehler", conc_bad: "Ungültiger Vertrag (0x… 40 hex)", conc_top50: "Top-50 zusammen", conc_reach: "Schwelle bei # Wallets", conc_assets: "Assets", conc_track: "🐋 Verfolgen", conc_export: "⬇ JSON exportieren", conc_export_csv: "CSV exportieren", conc_who_holds: "Wer hält (nach Kategorie)" },
  fr: { conc_btn: "Concentration", conc_title: "Concentration des détenteurs", conc_ph: "Contrat ERC-20 0x… (Ethereum)", conc_analyze: "Analyser", conc_hint: "Collez un contrat de token (Ethereum) → Analyser. Montre la part des principaux détenteurs et leurs actifs. Données publiques (Ethplorer).", conc_loading: "Chargement…", conc_err: "Erreur", conc_bad: "Contrat invalide (0x… 40 hex)", conc_top50: "Top-50 cumulé", conc_reach: "Seuil à # portefeuilles", conc_assets: "Actifs", conc_track: "🐋 Suivre", conc_export: "⬇ Export JSON", conc_export_csv: "Export CSV", conc_who_holds: "Qui détient (par catégorie)" },
  ru: { conc_btn: "Концентрация", conc_title: "Концентрация держателей токена", conc_ph: "Контракт ERC-20 0x… (Ethereum)", conc_analyze: "Анализ", conc_hint: "Вставьте контракт токена (Ethereum) → Анализ. Покажет долю топ-держателей и в каких активах они сидят. Публичные данные (Ethplorer).", conc_loading: "Загрузка…", conc_err: "Ошибка", conc_bad: "Неверный контракт (нужно 0x… 40 hex)", conc_top50: "Топ-50 суммарно", conc_reach: "Порог на № кошельков", conc_assets: "Активы", conc_track: "🐋 Следить", conc_export: "⬇ Экспорт JSON", conc_export_csv: "Экспорт CSV", conc_who_holds: "Кто держит (по категориям)" },
};
for (const l of Object.keys(CONC_TR) as Lang[]) Object.assign(TR[l], CONC_TR[l]);
const AF_TR: Record<Lang, Record<string, string>> = {
  uk: { af_title: "Графік ціни за весь період", af_loading: "Завантаження графіка ціни…", af_none: "Немає історії ціни для цього активу", af_now: "Зараз", af_holders: "тримачів", af_entry: "Сер. вхід китів", af_exit: "Сер. вихід китів", af_last_entry: "Останній відкуп", af_note: "Середні точки зважені за обсягом, по топ-тримачах (ончейн, історичні ціни DefiLlama)", af_mcap: "Капіталізація", af_wpnl: "Кити", af_wprofit: "у плюсі", af_wloss: "у мінусі", af_wphase_acc: "сер. вхід вище ціни → фаза накопичення", af_wphase_dist: "глибоко в плюсі → ризик розподілу", af_wphase_prof: "у прибутку відносно входу", af_liq: "Ліквідність", af_liq_deep: "глибока", af_liq_moderate: "помірна", af_liq_thin: "тонка", af_liq_dd: "тонка ліквідність — просадка ≠ вихід китів (внизу нема об кого продавати)", af_pump: "Сигнал пампу", af_pump_prepump: "Пре-памп (наповнення)", af_pump_pump: "Памп", af_pump_distribution: "Розподіл (вихід китів)", af_pump_neutral: "Нейтрально", af_pump_social: "соц-активність зростає", af_pump_exited: "виведено китами", af_lb: "Топ прибуткових гаманців", af_lb_profit: "прибуток", af_lb_holds: "тримає", af_g_liq: "Ліквідність", af_g_sent: "Настрій", af_g_whale: "PnL китів", af_g_exit: "Вихід китів" },
  en: { af_title: "Price chart (full period)", af_loading: "Loading price chart…", af_none: "No price history for this asset", af_now: "Now", af_holders: "holders", af_entry: "Whales avg entry", af_exit: "Whales avg exit", af_last_entry: "Latest re-buy", af_note: "Volume-weighted averages over top holders (on-chain, DefiLlama historical prices)", af_mcap: "Market cap", af_wpnl: "Whales", af_wprofit: "in profit", af_wloss: "underwater", af_wphase_acc: "avg entry above price → accumulation phase", af_wphase_dist: "deep in profit → distribution risk", af_wphase_prof: "in profit vs entry", af_liq: "Liquidity", af_liq_deep: "deep", af_liq_moderate: "moderate", af_liq_thin: "thin", af_liq_dd: "thin liquidity — a drawdown ≠ whales exiting (no one to sell to at the bottom)", af_pump: "Pump signal", af_pump_prepump: "Pre-pump (filling)", af_pump_pump: "Pump", af_pump_distribution: "Distribution (whales exiting)", af_pump_neutral: "Neutral", af_pump_social: "social activity rising", af_pump_exited: "exited by whales", af_lb: "Top profitable wallets", af_lb_profit: "profit", af_lb_holds: "holds", af_g_liq: "Liquidity", af_g_sent: "Sentiment", af_g_whale: "Whale PnL", af_g_exit: "Whale exit" },
  de: { af_title: "Preisdiagramm (gesamter Zeitraum)", af_loading: "Lade Preisdiagramm…", af_none: "Keine Preishistorie für dieses Asset", af_now: "Jetzt", af_holders: "Halter", af_entry: "Ø Einstieg Wale", af_exit: "Ø Ausstieg Wale", af_last_entry: "Letzter Rückkauf", af_note: "Volumengewichtete Mittel über Top-Halter (On-Chain, DefiLlama-Historie)", af_mcap: "Marktkap.", af_wpnl: "Wale", af_wprofit: "im Plus", af_wloss: "im Minus", af_wphase_acc: "Ø Einstieg über Preis → Akkumulationsphase", af_wphase_dist: "tief im Plus → Verteilungsrisiko", af_wphase_prof: "im Gewinn ggü. Einstieg", af_liq: "Liquidität", af_liq_deep: "tief", af_liq_moderate: "moderat", af_liq_thin: "dünn", af_liq_dd: "dünne Liquidität — ein Rückgang ≠ Wale steigen aus (unten gibt es keine Käufer)", af_pump: "Pump-Signal", af_pump_prepump: "Vor-Pump (Auffüllen)", af_pump_pump: "Pump", af_pump_distribution: "Verteilung (Wale steigen aus)", af_pump_neutral: "Neutral", af_pump_social: "soziale Aktivität steigt", af_pump_exited: "von Walen ausgestiegen", af_lb: "Top-profitable Wallets", af_lb_profit: "Gewinn", af_lb_holds: "hält", af_g_liq: "Liquidität", af_g_sent: "Stimmung", af_g_whale: "Wal-PnL", af_g_exit: "Wal-Ausstieg" },
  fr: { af_title: "Graphique des prix (période complète)", af_loading: "Chargement du graphique…", af_none: "Pas d'historique de prix", af_now: "Actuel", af_holders: "détenteurs", af_entry: "Entrée moy. baleines", af_exit: "Sortie moy. baleines", af_last_entry: "Dernier rachat", af_note: "Moyennes pondérées par volume sur les principaux détenteurs (on-chain, DefiLlama)", af_mcap: "Capitalisation", af_wpnl: "Baleines", af_wprofit: "en profit", af_wloss: "en perte", af_wphase_acc: "entrée moy. au-dessus du prix → phase d'accumulation", af_wphase_dist: "fort profit → risque de distribution", af_wphase_prof: "en profit vs entrée", af_liq: "Liquidité", af_liq_deep: "profonde", af_liq_moderate: "modérée", af_liq_thin: "faible", af_liq_dd: "faible liquidité — une baisse ≠ sortie des baleines (personne à qui vendre en bas)", af_pump: "Signal de pump", af_pump_prepump: "Pré-pump (remplissage)", af_pump_pump: "Pump", af_pump_distribution: "Distribution (sortie des baleines)", af_pump_neutral: "Neutre", af_pump_social: "activité sociale en hausse", af_pump_exited: "sorti par les baleines", af_lb: "Top wallets rentables", af_lb_profit: "profit", af_lb_holds: "détient", af_g_liq: "Liquidité", af_g_sent: "Sentiment", af_g_whale: "PnL baleines", af_g_exit: "Sortie baleines" },
  ru: { af_title: "График цены за весь период", af_loading: "Загрузка графика цены…", af_none: "Нет истории цены для этого актива", af_now: "Сейчас", af_holders: "держателей", af_entry: "Ср. вход китов", af_exit: "Ср. выход китов", af_last_entry: "Последний откуп", af_note: "Средние точки взвешены по объёму, по топ-держателям (ончейн, исторические цены DefiLlama)", af_mcap: "Капитализация", af_wpnl: "Киты", af_wprofit: "в плюсе", af_wloss: "в минусе", af_wphase_acc: "ср. вход выше цены → фаза накопления", af_wphase_dist: "глубоко в плюсе → риск распределения", af_wphase_prof: "в прибыли относительно входа", af_liq: "Ликвидность", af_liq_deep: "глубокая", af_liq_moderate: "умеренная", af_liq_thin: "тонкая", af_liq_dd: "тонкая ликвидность — просадка ≠ выход китов (внизу некому продавать)", af_pump: "Сигнал пампа", af_pump_prepump: "Пре-памп (наполнение)", af_pump_pump: "Памп", af_pump_distribution: "Распределение (выход китов)", af_pump_neutral: "Нейтрально", af_pump_social: "соц-активность растёт", af_pump_exited: "выведено китами", af_lb: "Топ прибыльных кошельков", af_lb_profit: "прибыль", af_lb_holds: "держит", af_g_liq: "Ликвидность", af_g_sent: "Настроение", af_g_whale: "PnL китов", af_g_exit: "Выход китов" },
};
for (const l of Object.keys(AF_TR) as Lang[]) Object.assign(TR[l], AF_TR[l]);
const HIST_TR: Record<Lang, Record<string, string>> = {
  uk: { hist_conc_trend: "Тренд концентрації (топ-10)", hist_collecting: "Збираємо історію — тренд з'явиться за кілька днів", hist_points: "точок", hist_cl_trend: "тренд кластерів" },
  en: { hist_conc_trend: "Concentration trend (top-10)", hist_collecting: "Collecting history — trend appears in a few days", hist_points: "points", hist_cl_trend: "clusters trend" },
  de: { hist_conc_trend: "Konzentrationstrend (Top-10)", hist_collecting: "Verlauf wird gesammelt — Trend in einigen Tagen", hist_points: "Punkte", hist_cl_trend: "Cluster-Trend" },
  fr: { hist_conc_trend: "Tendance de concentration (top-10)", hist_collecting: "Collecte d'historique — tendance dans quelques jours", hist_points: "points", hist_cl_trend: "tendance des clusters" },
  ru: { hist_conc_trend: "Тренд концентрации (топ-10)", hist_collecting: "Собираем историю — тренд появится через несколько дней", hist_points: "точек", hist_cl_trend: "тренд кластеров" },
};
for (const l of Object.keys(HIST_TR) as Lang[]) Object.assign(TR[l], HIST_TR[l]);
const ALPHA_TR: Record<Lang, Record<string, string>> = {
  uk: { vol_watch_h: "🎯 Волатильність: кити набирають → великий ±рух", vol_watch_empty: "Немає волатильних кандидатів", alpha_feed_h: "🟢 Alpha: кити заходять у нові токени", alpha_new_token: "новий токен", alpha_toast: "Smart Money сигнал", alpha_empty: "Поки немає сигналів — накопичується (потрібно ≥2 цикли воркера)" },
  en: { vol_watch_h: "🎯 Volatility: whales loading → big ± move likely", vol_watch_empty: "No volatility candidates", alpha_feed_h: "🟢 Alpha: whales entering new tokens", alpha_new_token: "new token", alpha_toast: "Smart Money signal", alpha_empty: "No signals yet — accumulating (needs ≥2 worker cycles)" },
  de: { vol_watch_h: "🎯 Volatilität: Wale laden → große ±Bewegung wahrscheinlich", vol_watch_empty: "Keine Volatilitäts-Kandidaten", alpha_feed_h: "🟢 Alpha: Wale steigen in neue Token ein", alpha_new_token: "neuer Token", alpha_toast: "Smart-Money-Signal", alpha_empty: "Noch keine Signale — wird gesammelt (≥2 Zyklen nötig)" },
  fr: { vol_watch_h: "🎯 Volatilité : baleines chargent → gros mouvement ± probable", vol_watch_empty: "Aucun candidat de volatilité", alpha_feed_h: "🟢 Alpha : baleines entrant sur de nouveaux tokens", alpha_new_token: "nouveau token", alpha_toast: "Signal Smart Money", alpha_empty: "Pas encore de signaux — en accumulation (≥2 cycles requis)" },
  ru: { vol_watch_h: "🎯 Волатильность: киты набирают → крупное ±движение", vol_watch_empty: "Нет волатильных кандидатов", alpha_feed_h: "🟢 Alpha: киты заходят в новые токены", alpha_new_token: "новый токен", alpha_toast: "Smart Money сигнал", alpha_empty: "Пока нет сигналов — накапливается (нужно ≥2 цикла воркера)" },
};
for (const l of Object.keys(ALPHA_TR) as Lang[]) Object.assign(TR[l], ALPHA_TR[l]);
const WS_TR: Record<Lang, Record<string, string>> = {
  uk: { ws_title: "Скоринг гаманця (Smart Money)", ws_loading: "Рахуємо скоринг (ROI, win-rate)…", ws_none: "Замало історії для скорингу", ws_roi: "ROI", ws_winrate: "Win-rate", ws_pnl: "Realized PnL", ws_hold: "Сер. утримання", ws_days: "д", ws_closed: "закритих позицій", ws_positions: "усього", ws_best: "найкраща", ws_worst: "найгірша", ws_note: "За ончейн-історією торгів (зважена собівартість, історичні ціни DefiLlama)", ws_ai: "AI-ймовірність Smart Money", ws_ai_note: "(модель само-навчання на PnL)" },
  en: { ws_title: "Wallet score (Smart Money)", ws_loading: "Computing score (ROI, win-rate)…", ws_none: "Not enough history for scoring", ws_roi: "ROI", ws_winrate: "Win-rate", ws_pnl: "Realized PnL", ws_hold: "Avg hold", ws_days: "d", ws_closed: "closed positions", ws_positions: "total", ws_best: "best", ws_worst: "worst", ws_note: "From on-chain trade history (weighted cost basis, DefiLlama historical prices)", ws_ai: "AI Smart Money probability", ws_ai_note: "(self-learning model on PnL)" },
  de: { ws_title: "Wallet-Score (Smart Money)", ws_loading: "Score wird berechnet (ROI, Win-Rate)…", ws_none: "Zu wenig Historie für Score", ws_roi: "ROI", ws_winrate: "Win-Rate", ws_pnl: "Realisierter PnL", ws_hold: "Ø Haltedauer", ws_days: "T", ws_closed: "geschlossene Positionen", ws_positions: "gesamt", ws_best: "beste", ws_worst: "schlechteste", ws_note: "Aus On-Chain-Handelshistorie (gewichtete Kostenbasis, DefiLlama-Historie)", ws_ai: "KI-Smart-Money-Wahrscheinlichkeit", ws_ai_note: "(selbstlernendes Modell auf PnL)" },
  fr: { ws_title: "Score du portefeuille (Smart Money)", ws_loading: "Calcul du score (ROI, win-rate)…", ws_none: "Historique insuffisant pour le score", ws_roi: "ROI", ws_winrate: "Taux de réussite", ws_pnl: "PnL réalisé", ws_hold: "Détention moy.", ws_days: "j", ws_closed: "positions fermées", ws_positions: "total", ws_best: "meilleure", ws_worst: "pire", ws_note: "D'après l'historique on-chain (coût moyen pondéré, prix historiques DefiLlama)", ws_ai: "Probabilité Smart Money (IA)", ws_ai_note: "(modèle auto-apprenant sur PnL)" },
  ru: { ws_title: "Скоринг кошелька (Smart Money)", ws_loading: "Считаем скоринг (ROI, win-rate)…", ws_none: "Мало истории для скоринга", ws_roi: "ROI", ws_winrate: "Win-rate", ws_pnl: "Realized PnL", ws_hold: "Ср. удержание", ws_days: "д", ws_closed: "закрытых позиций", ws_positions: "всего", ws_best: "лучшая", ws_worst: "худшая", ws_note: "По ончейн-истории торгов (взвешенная себестоимость, исторические цены DefiLlama)", ws_ai: "AI-вероятность Smart Money", ws_ai_note: "(самообучающаяся модель на PnL)" },
};
for (const l of Object.keys(WS_TR) as Lang[]) Object.assign(TR[l], WS_TR[l]);
const NF_TR: Record<Lang, Record<string, string>> = {
  uk: { nf_title: "Потік біржі (CEX)", nf_in: "На біржу", nf_out: "З біржі", nf_net: "Чистий потік", nf_accumulation: "Накопичення", nf_distribution: "Роздача", nf_neutral: "Нейтрально", nf_note: "з біржі = накопичення, на біржу = тиск на продаж (останні перекази)" },
  en: { nf_title: "Exchange flow (CEX)", nf_in: "To exchange", nf_out: "From exchange", nf_net: "Net flow", nf_accumulation: "Accumulation", nf_distribution: "Distribution", nf_neutral: "Neutral", nf_note: "from exchange = accumulation, to exchange = sell pressure (recent transfers)" },
  de: { nf_title: "Börsenfluss (CEX)", nf_in: "Zur Börse", nf_out: "Von Börse", nf_net: "Nettofluss", nf_accumulation: "Akkumulation", nf_distribution: "Verteilung", nf_neutral: "Neutral", nf_note: "von Börse = Akkumulation, zur Börse = Verkaufsdruck (jüngste Transfers)" },
  fr: { nf_title: "Flux d'échange (CEX)", nf_in: "Vers l'échange", nf_out: "Depuis l'échange", nf_net: "Flux net", nf_accumulation: "Accumulation", nf_distribution: "Distribution", nf_neutral: "Neutre", nf_note: "depuis l'échange = accumulation, vers l'échange = pression vendeuse (transferts récents)" },
  ru: { nf_title: "Поток биржи (CEX)", nf_in: "На биржу", nf_out: "С биржи", nf_net: "Чистый поток", nf_accumulation: "Накопление", nf_distribution: "Раздача", nf_neutral: "Нейтрально", nf_note: "с биржи = накопление, на биржу = давление на продажу (последние переводы)" },
};
for (const l of Object.keys(NF_TR) as Lang[]) Object.assign(TR[l], NF_TR[l]);
const BT_TR: Record<Lang, Record<string, string>> = {
  uk: { bt_title: "Бектест сигналу «вхід китів»", bt_collecting: "Рахуємо бектест сигналів…", bt_excess: "Надлишок над {b}", bt_median: "медіана", bt_raw: "сира дохідність", bt_hitrate: "влучань", bt_events: "подій входу", bt_wallets: "гаманців", bt_pending: "подій набирають горизонт", bt_inconclusive: "Довірчий інтервал накриває 50% — сигнал статистично не відрізняється від монетки", bt_note: "Дохідність за {h} днів від входу кита, мінус {b} за той самий період" },
  en: { bt_title: "Backtest: whale-entry signal", bt_collecting: "Computing signal backtest…", bt_excess: "Excess over {b}", bt_median: "median", bt_raw: "raw return", bt_hitrate: "hit-rate", bt_events: "entry events", bt_wallets: "wallets", bt_pending: "events still maturing", bt_inconclusive: "The confidence interval spans 50% — this signal is not distinguishable from a coin flip", bt_note: "Return over {h} days from the whale entry, minus {b} across the same window" },
  de: { bt_title: "Backtest: Wal-Einstiegssignal", bt_collecting: "Signal-Backtest wird berechnet…", bt_excess: "Überrendite ggü. {b}", bt_median: "Median", bt_raw: "Rohrendite", bt_hitrate: "Trefferquote", bt_events: "Einstiegsereignisse", bt_wallets: "Wallets", bt_pending: "Ereignisse reifen noch", bt_inconclusive: "Das Konfidenzintervall umfasst 50% — das Signal ist nicht von einem Münzwurf zu unterscheiden", bt_note: "Rendite über {h} Tage ab dem Wal-Einstieg, abzüglich {b} im selben Zeitraum" },
  fr: { bt_title: "Backtest : signal d'entrée des baleines", bt_collecting: "Calcul du backtest…", bt_excess: "Surperformance vs {b}", bt_median: "médiane", bt_raw: "rendement brut", bt_hitrate: "taux de réussite", bt_events: "événements d'entrée", bt_wallets: "portefeuilles", bt_pending: "événements en cours de maturation", bt_inconclusive: "L'intervalle de confiance couvre 50% — le signal ne se distingue pas d'un tirage au sort", bt_note: "Rendement sur {h} jours depuis l'entrée de la baleine, moins {b} sur la même période" },
  ru: { bt_title: "Бэктест сигнала «вход китов»", bt_collecting: "Считаем бэктест сигналов…", bt_excess: "Избыток над {b}", bt_median: "медиана", bt_raw: "сырая доходность", bt_hitrate: "попаданий", bt_events: "событий входа", bt_wallets: "кошельков", bt_pending: "событий набирают горизонт", bt_inconclusive: "Доверительный интервал накрывает 50% — сигнал статистически не отличим от монетки", bt_note: "Доходность за {h} дней от входа кита, минус {b} за тот же период" },
};
for (const l of Object.keys(BT_TR) as Lang[]) Object.assign(TR[l], BT_TR[l]);
const POS_TR: Record<Lang, Record<string, string>> = {
  uk: { pos_h: "Хто де сидить (кит → активи)", net_h: "Координовані мережі китів", net_one: "Мережа", net_shared: "спільні активи", net_graph: "Граф мережі", net_collective: "Сукупні активи", mv_h: "Синхронні рухи (72год)", mv_accum: "синхронно накопичує", mv_dist: "синхронно роздає", gf_btn: "Потік групи" }, en: { pos_h: "Who sits where (whale → assets)", net_h: "Coordinated whale networks", net_one: "Network", net_shared: "shared assets", net_graph: "Network graph", net_collective: "Collective assets", mv_h: "Synchronized moves (72h)", mv_accum: "synchronously accumulating", mv_dist: "synchronously distributing", gf_btn: "Group flow" },
  de: { pos_h: "Wer sitzt wo (Wal → Assets)", net_h: "Koordinierte Wal-Netzwerke", net_one: "Netzwerk", net_shared: "geteilte Assets", net_graph: "Netzwerkgraph", net_collective: "Gesamt-Assets", mv_h: "Synchrone Bewegungen (72h)", mv_accum: "akkumuliert synchron", mv_dist: "verteilt synchron", gf_btn: "Gruppenfluss" }, fr: { pos_h: "Qui est où (baleine → actifs)", net_h: "Réseaux de baleines coordonnés", net_one: "Réseau", net_shared: "actifs communs", net_graph: "Graphe du réseau", net_collective: "Actifs collectifs", mv_h: "Mouvements synchronisés (72h)", mv_accum: "accumule en synchrone", mv_dist: "distribue en synchrone", gf_btn: "Flux du groupe" }, ru: { pos_h: "Кто где сидит (кит → активы)", net_h: "Координированные сети китов", net_one: "Сеть", net_shared: "общие активы", net_graph: "Граф сети", net_collective: "Совокупные активы", mv_h: "Синхронные движения (72ч)", mv_accum: "синхронно накапливает", mv_dist: "синхронно раздаёт", gf_btn: "Поток группы" },
};
for (const l of Object.keys(POS_TR) as Lang[]) Object.assign(TR[l], POS_TR[l]);
const GDB_TR: Record<Lang, Record<string, string>> = {
  uk: { gdb_base: "База графа", gdb_wallets: "гаманців", gdb_links: "звʼязків", gdb_groups: "груп", gdb_volume: "обсяг", dg_h: "Виявлені групи (з бази графа)", dg_group: "Група" },
  en: { gdb_base: "Graph DB", gdb_wallets: "wallets", gdb_links: "links", gdb_groups: "groups", gdb_volume: "volume", dg_h: "Discovered groups (from graph DB)", dg_group: "Group" },
  de: { gdb_base: "Graph-DB", gdb_wallets: "Wallets", gdb_links: "Verbindungen", gdb_groups: "Gruppen", gdb_volume: "Volumen", dg_h: "Entdeckte Gruppen (aus Graph-DB)", dg_group: "Gruppe" },
  fr: { gdb_base: "BD du graphe", gdb_wallets: "portefeuilles", gdb_links: "liens", gdb_groups: "groupes", gdb_volume: "volume", dg_h: "Groupes découverts (BD du graphe)", dg_group: "Groupe" },
  ru: { gdb_base: "База графа", gdb_wallets: "кошельков", gdb_links: "связей", gdb_groups: "групп", gdb_volume: "объём", dg_h: "Обнаруженные группы (из базы графа)", dg_group: "Группа" },
};
for (const l of Object.keys(GDB_TR) as Lang[]) Object.assign(TR[l], GDB_TR[l]);
const WL_TR: Record<Lang, Record<string, string>> = {
  // Стани балансу гаманця. `wl_idle` — ще НЕ запитували (баланс тягнеться, лише коли гаманець
  // потрапляє в поле зору); `wl_failed` — запит упав, і це клікабельно для повтору. Раніше на всі
  // випадки був один `wl_loading`, тож упалий запит виглядав як вічне завантаження.
  // `stat_value_partial` не дає прочитати часткову суму як підсумок по всіх гаманцях.
  uk: { wl_loading: "баланс завантажується…", wl_queued: "у черзі на баланс", wl_idle: "баланс не запитано", wl_failed: "баланс не завантажився — повторити", stat_value_partial: "за {n} із {m} гаманців" },
  en: { wl_loading: "loading balance…", wl_queued: "queued for balance", wl_idle: "balance not requested", wl_failed: "balance failed — retry", stat_value_partial: "across {n} of {m} wallets" },
  de: { wl_loading: "Saldo wird geladen…", wl_queued: "Saldo in Warteschlange", wl_idle: "Saldo nicht abgefragt", wl_failed: "Saldo fehlgeschlagen — erneut", stat_value_partial: "über {n} von {m} Wallets" },
  fr: { wl_loading: "chargement du solde…", wl_queued: "solde en file d'attente", wl_idle: "solde non demandé", wl_failed: "échec du solde — réessayer", stat_value_partial: "sur {n} portefeuilles sur {m}" },
  ru: { wl_loading: "баланс загружается…", wl_queued: "в очереди на баланс", wl_idle: "баланс не запрошен", wl_failed: "баланс не загрузился — повторить", stat_value_partial: "по {n} из {m} кошельков" },
};
for (const l of Object.keys(WL_TR) as Lang[]) Object.assign(TR[l], WL_TR[l]);
const MT_TR: Record<Lang, Record<string, string>> = {
  uk: { mt_btn: "Простежити гроші (15 колін)", mt_loading: "Трасування потоку грошей…", mt_hops: "колін", mt_exits: "Депозити на біржу (продаж)", mt_total: "Усього", mt_exchange: "біржа", mt_flows: "Потоки біржі", mt_withdrawals: "Виведення з біржі (накопичення)", mt_net: "Чистий потік (підсумок)", mt_accum: "накопичує", mt_dump: "скидає" },
  en: { mt_btn: "Trace money (15 hops)", mt_loading: "Tracing money flow…", mt_hops: "hops", mt_exits: "Exchange deposits (selling)", mt_total: "Total", mt_exchange: "exchange", mt_flows: "Exchange flows", mt_withdrawals: "Exchange withdrawals (accumulation)", mt_net: "Net flow (summary)", mt_accum: "accumulating", mt_dump: "dumping" },
  de: { mt_btn: "Geld verfolgen (15 Hops)", mt_loading: "Geldfluss wird verfolgt…", mt_hops: "Hops", mt_exits: "Börsen-Einzahlungen (Verkauf)", mt_total: "Gesamt", mt_exchange: "Börse", mt_flows: "Börsen-Flüsse", mt_withdrawals: "Börsen-Abhebungen (Akkumulation)", mt_net: "Netto-Fluss (Bilanz)", mt_accum: "akkumuliert", mt_dump: "verkauft" },
  fr: { mt_btn: "Tracer l'argent (15 sauts)", mt_loading: "Traçage du flux…", mt_hops: "sauts", mt_exits: "Dépôts sur l'échange (vente)", mt_total: "Total", mt_exchange: "échange", mt_flows: "Flux d'échange", mt_withdrawals: "Retraits d'échange (accumulation)", mt_net: "Flux net (bilan)", mt_accum: "accumule", mt_dump: "vend" },
  ru: { mt_btn: "Проследить деньги (15 колен)", mt_loading: "Трассировка потока денег…", mt_hops: "колен", mt_exits: "Депозиты на биржу (продажа)", mt_total: "Всего", mt_exchange: "биржа", mt_flows: "Потоки биржи", mt_withdrawals: "Вывод с биржи (накопление)", mt_net: "Чистый поток (итог)", mt_accum: "накапливает", mt_dump: "сбрасывает" },
};
for (const l of Object.keys(MT_TR) as Lang[]) Object.assign(TR[l], MT_TR[l]);
const WF_TR: Record<Lang, Record<string, string>> = {
  uk: { wf_groups_h: "Координовані китові групи ($500M+)", cond_h: "Диригенти (крос-актив координатори)" }, en: { wf_groups_h: "Coordinated whale groups ($500M+)", cond_h: "Conductors (cross-asset coordinators)" },
  de: { wf_groups_h: "Koordinierte Wal-Gruppen ($500M+)", cond_h: "Dirigenten (asset-übergreifend)" }, fr: { wf_groups_h: "Groupes de baleines ($500M+)", cond_h: "Chefs d'orchestre (multi-actifs)" }, ru: { wf_groups_h: "Координированные китовые группы ($500M+)", cond_h: "Дирижёры (кросс-актив координаторы)" },
};
for (const l of Object.keys(WF_TR) as Lang[]) Object.assign(TR[l], WF_TR[l]);

const CONC2_TR: Record<Lang, Record<string, string>> = {
  uk: { intel_entry: "Entry Score", intel_fg: "Страх/Жадібність", intel_behaviour: "Поведінка", intel_top10: "Топ-10 тримають", intel_disclaimer: "Концентрація й топ-холдери — реальні дані (Ethplorer). Entry / Страх-Жадібність / Поведінка — евристичні скори (демо).", class_whale: "Кит", class_cex: "Біржа", class_burn: "Спалено", class_contract: "Контракт", class_large: "Великий", class_holder: "Холдер", class_mm: "Маркетмейкер", class_fund: "Фонд", class_retail: "Ретейл" },
  en: { intel_entry: "Entry Score", intel_fg: "Fear/Greed", intel_behaviour: "Behaviour", intel_top10: "Top-10 hold", intel_disclaimer: "Concentration & top holders are real data (Ethplorer). Entry / Fear-Greed / Behaviour are heuristic scores (demo).", class_whale: "Whale", class_cex: "Exchange", class_burn: "Burned", class_contract: "Contract", class_large: "Large", class_holder: "Holder", class_mm: "Market maker", class_fund: "Fund", class_retail: "Retail" },
  de: { intel_entry: "Entry Score", intel_fg: "Angst/Gier", intel_behaviour: "Verhalten", intel_top10: "Top-10 halten", intel_disclaimer: "Konzentration & Top-Halter sind echte Daten (Ethplorer). Entry / Angst-Gier / Verhalten sind heuristische Scores (Demo).", class_whale: "Wal", class_cex: "Börse", class_burn: "Verbrannt", class_contract: "Vertrag", class_large: "Groß", class_holder: "Halter", class_mm: "Market Maker", class_fund: "Fonds", class_retail: "Retail" },
  fr: { intel_entry: "Entry Score", intel_fg: "Peur/Avidité", intel_behaviour: "Comportement", intel_top10: "Top-10 détient", intel_disclaimer: "Concentration & principaux détenteurs : données réelles (Ethplorer). Entry / Peur-Avidité / Comportement : scores heuristiques (démo).", class_whale: "Baleine", class_cex: "Plateforme", class_burn: "Brûlé", class_contract: "Contrat", class_large: "Grand", class_holder: "Détenteur", class_mm: "Market maker", class_fund: "Fonds", class_retail: "Retail" },
  ru: { intel_entry: "Entry Score", intel_fg: "Страх/Жадность", intel_behaviour: "Поведение", intel_top10: "Топ-10 держат", intel_disclaimer: "Концентрация и топ-держатели — реальные данные (Ethplorer). Entry / Страх-Жадность / Поведение — эвристические скоры (демо).", class_whale: "Кит", class_cex: "Биржа", class_burn: "Сожжено", class_contract: "Контракт", class_large: "Крупный", class_holder: "Холдер", class_mm: "Маркетмейкер", class_fund: "Фонд", class_retail: "Ретейл" },
};
for (const l of Object.keys(CONC2_TR) as Lang[]) Object.assign(TR[l], CONC2_TR[l]);

const ONCHAIN_TR: Record<Lang, Record<string, string>> = {
  uk: { onchain_real: "Реальний баланс ончейн (нативний, усі EVM)", onchain_total: "Разом ончейн" },
  en: { onchain_real: "Real on-chain balance (native, all EVM)", onchain_total: "On-chain total" },
  de: { onchain_real: "Echtes On-Chain-Guthaben (nativ, alle EVM)", onchain_total: "On-Chain gesamt" },
  fr: { onchain_real: "Solde on-chain réel (natif, tout EVM)", onchain_total: "Total on-chain" },
  ru: { onchain_real: "Реальный баланс ончейн (нативный, все EVM)", onchain_total: "Всего ончейн" },
};
for (const l of Object.keys(ONCHAIN_TR) as Lang[]) Object.assign(TR[l], ONCHAIN_TR[l]);

const BEH_TR: Record<Lang, Record<string, string>> = {
  uk: { behavior_title: "Поведінка (аналітика)", beh_netflow: "Чистий потік", beh_in: "Вхід", beh_out: "Вихід", beh_cps: "Контрагентів", beh_tx: "Транзакцій", beh_whale: "Кит", beh_exchange: "Біржа", beh_mm: "Маркетмейкер/бот", beh_accumulator: "Накопичувач", beh_distributor: "Розпродавач", beh_trader: "Трейдер", beh_low_data: "Мало даних" },
  en: { behavior_title: "Behaviour (analytics)", beh_netflow: "Net flow", beh_in: "In", beh_out: "Out", beh_cps: "Counterparties", beh_tx: "Txs", beh_whale: "Whale", beh_exchange: "Exchange", beh_mm: "Market maker/bot", beh_accumulator: "Accumulator", beh_distributor: "Distributor", beh_trader: "Trader", beh_low_data: "Low data" },
  de: { behavior_title: "Verhalten (Analyse)", beh_netflow: "Nettofluss", beh_in: "Ein", beh_out: "Aus", beh_cps: "Gegenparteien", beh_tx: "Txs", beh_whale: "Wal", beh_exchange: "Börse", beh_mm: "Market Maker/Bot", beh_accumulator: "Akkumulierer", beh_distributor: "Verteiler", beh_trader: "Trader", beh_low_data: "Wenig Daten" },
  fr: { behavior_title: "Comportement (analyse)", beh_netflow: "Flux net", beh_in: "Entrée", beh_out: "Sortie", beh_cps: "Contreparties", beh_tx: "Txs", beh_whale: "Baleine", beh_exchange: "Plateforme", beh_mm: "Market maker/bot", beh_accumulator: "Accumulateur", beh_distributor: "Distributeur", beh_trader: "Trader", beh_low_data: "Peu de données" },
  ru: { behavior_title: "Поведение (аналитика)", beh_netflow: "Чистый поток", beh_in: "Вход", beh_out: "Выход", beh_cps: "Контрагентов", beh_tx: "Транзакций", beh_whale: "Кит", beh_exchange: "Биржа", beh_mm: "Маркетмейкер/бот", beh_accumulator: "Накопитель", beh_distributor: "Распродавец", beh_trader: "Трейдер", beh_low_data: "Мало данных" },
};
for (const l of Object.keys(BEH_TR) as Lang[]) Object.assign(TR[l], BEH_TR[l]);

const ML_TR: Record<Lang, Record<string, string>> = {
  uk: { ml_title: "ML-клас", ml_exchange: "Біржа", ml_fund: "Фонд", ml_contract: "Контракт", ml_burn: "Burn", ml_individual: "Особа", ml_mm: "MM/бот", ml_other: "Інше" },
  en: { ml_title: "ML class", ml_exchange: "Exchange", ml_fund: "Fund", ml_contract: "Contract", ml_burn: "Burn", ml_individual: "Individual", ml_mm: "MM/bot", ml_other: "Other" },
  de: { ml_title: "ML-Klasse", ml_exchange: "Börse", ml_fund: "Fonds", ml_contract: "Vertrag", ml_burn: "Burn", ml_individual: "Person", ml_mm: "MM/Bot", ml_other: "Andere" },
  fr: { ml_title: "Classe ML", ml_exchange: "Plateforme", ml_fund: "Fonds", ml_contract: "Contrat", ml_burn: "Burn", ml_individual: "Personne", ml_mm: "MM/bot", ml_other: "Autre" },
  ru: { ml_title: "ML-класс", ml_exchange: "Биржа", ml_fund: "Фонд", ml_contract: "Контракт", ml_burn: "Burn", ml_individual: "Лицо", ml_mm: "MM/бот", ml_other: "Прочее" },
};
for (const l of Object.keys(ML_TR) as Lang[]) Object.assign(TR[l], ML_TR[l]);

const CL_TR: Record<Lang, Record<string, string>> = {
  uk: { cl_open: "Кластери та групи", cl_title: "Кластери гаманців · аналітика груп", cl_goal: "Кінцева ціль потоків", cl_cps: "контрагентів", cl_basis: "база", cl_addr: "адрес", cl_avgbal: "сер. баланс групи", cl_in: "прихід", cl_out: "відтік", cl_analyzing: "Аналіз ончейн-даних по всіх мережах…", cl_cat_cex: "Біржі (CEX)", cl_cat_dex: "DEX / свопи", cl_cat_bridge: "Мости", cl_cat_defi: "DeFi / контракти", cl_cat_eoa: "Гаманці (особи)", cl_v_sell_cex: "Продаж / виведення на біржі", cl_v_swap_dex: "Обмін на DEX (свопи)", cl_v_bridge: "Переміщення через мости", cl_v_defi: "Взаємодія з DeFi", cl_v_transfer: "Перекази на гаманці", cl_v_hold: "Утримання (немає відтоку)", cl_timeline: "Активність у часі", cl_period: "Період", cl_coverage: "оцінено цінами", cl_v_buy_cex: "Купівля / завід з бірж", cl_buy: "Купівля", cl_sell: "Продаж", cl_stance: "Позиція", cl_accumulating: "накопичує", cl_distributing: "роздає", cl_btn: "Кластери", cl_run: "Аналізувати", cl_ph: "0x… / BTC / Tron / Solana адреса", cl_2hop_btn: "Знайти пов'язані групи (2-hop)", cl_2hop_loading: "Сканування 2-hop…", cl_2hop_none: "Скоординованих груп не виявлено", cl_2hop_group: "Група", cl_2hop_direct: "прямий зв'язок", cl_2hop_shared: "спільне джерело" },
  en: { cl_open: "Clusters & groups", cl_title: "Wallet clusters · group analytics", cl_goal: "End goal of flows", cl_cps: "counterparties", cl_basis: "basis", cl_addr: "addrs", cl_avgbal: "avg group balance", cl_in: "in", cl_out: "out", cl_analyzing: "Analyzing on-chain data across networks…", cl_cat_cex: "Exchanges (CEX)", cl_cat_dex: "DEX / swaps", cl_cat_bridge: "Bridges", cl_cat_defi: "DeFi / contracts", cl_cat_eoa: "Wallets (EOA)", cl_v_sell_cex: "Selling / offramp to exchanges", cl_v_swap_dex: "Swapping on DEX", cl_v_bridge: "Bridging cross-chain", cl_v_defi: "Interacting with DeFi", cl_v_transfer: "Transfers to wallets", cl_v_hold: "Holding (no outflow)", cl_timeline: "Activity over time", cl_period: "Period", cl_coverage: "priced", cl_v_buy_cex: "Buying / onramp from exchanges", cl_buy: "Buy", cl_sell: "Sell", cl_stance: "Stance", cl_accumulating: "accumulating", cl_distributing: "distributing", cl_btn: "Clusters", cl_run: "Analyze", cl_ph: "0x… / BTC / Tron / Solana address", cl_2hop_btn: "Find related groups (2-hop)", cl_2hop_loading: "Scanning 2-hop…", cl_2hop_none: "No coordinated groups found", cl_2hop_group: "Group", cl_2hop_direct: "direct link", cl_2hop_shared: "shared source" },
  de: { cl_open: "Cluster & Gruppen", cl_title: "Wallet-Cluster · Gruppenanalyse", cl_goal: "Endziel der Flüsse", cl_cps: "Gegenparteien", cl_basis: "Basis", cl_addr: "Adressen", cl_avgbal: "Ø Gruppensaldo", cl_in: "Ein", cl_out: "Aus", cl_analyzing: "On-Chain-Analyse über Netzwerke…", cl_cat_cex: "Börsen (CEX)", cl_cat_dex: "DEX / Swaps", cl_cat_bridge: "Bridges", cl_cat_defi: "DeFi / Verträge", cl_cat_eoa: "Wallets (EOA)", cl_v_sell_cex: "Verkauf / Abzug an Börsen", cl_v_swap_dex: "Tausch auf DEX", cl_v_bridge: "Cross-Chain-Bridging", cl_v_defi: "DeFi-Interaktion", cl_v_transfer: "Überweisungen an Wallets", cl_v_hold: "Halten (kein Abfluss)", cl_timeline: "Aktivität über Zeit", cl_period: "Zeitraum", cl_coverage: "bepreist", cl_v_buy_cex: "Kauf / Zufluss von Börsen", cl_buy: "Kauf", cl_sell: "Verkauf", cl_stance: "Position", cl_accumulating: "akkumuliert", cl_distributing: "verteilt", cl_btn: "Cluster", cl_run: "Analysieren", cl_ph: "0x… / BTC / Tron / Solana Adresse", cl_2hop_btn: "Verbundene Gruppen finden (2-hop)", cl_2hop_loading: "2-hop-Scan…", cl_2hop_none: "Keine koordinierten Gruppen", cl_2hop_group: "Gruppe", cl_2hop_direct: "direkte Verbindung", cl_2hop_shared: "gemeinsame Quelle" },
  fr: { cl_open: "Clusters et groupes", cl_title: "Clusters de portefeuilles · analyse de groupes", cl_goal: "Objectif final des flux", cl_cps: "contreparties", cl_basis: "base", cl_addr: "adr.", cl_avgbal: "solde moyen du groupe", cl_in: "entrée", cl_out: "sortie", cl_analyzing: "Analyse on-chain sur les réseaux…", cl_cat_cex: "Plateformes (CEX)", cl_cat_dex: "DEX / swaps", cl_cat_bridge: "Ponts", cl_cat_defi: "DeFi / contrats", cl_cat_eoa: "Portefeuilles (EOA)", cl_v_sell_cex: "Vente / sortie vers plateformes", cl_v_swap_dex: "Échange sur DEX", cl_v_bridge: "Transfert via ponts", cl_v_defi: "Interaction DeFi", cl_v_transfer: "Transferts vers portefeuilles", cl_v_hold: "Conservation (aucune sortie)", cl_timeline: "Activité dans le temps", cl_period: "Période", cl_coverage: "valorisé", cl_v_buy_cex: "Achat / entrée depuis plateformes", cl_buy: "Achat", cl_sell: "Vente", cl_stance: "Position", cl_accumulating: "accumule", cl_distributing: "distribue", cl_btn: "Clusters", cl_run: "Analyser", cl_ph: "Adresse 0x… / BTC / Tron / Solana", cl_2hop_btn: "Trouver des groupes liés (2-hop)", cl_2hop_loading: "Analyse 2-hop…", cl_2hop_none: "Aucun groupe coordonné", cl_2hop_group: "Groupe", cl_2hop_direct: "lien direct", cl_2hop_shared: "source commune" },
  ru: { cl_open: "Кластеры и группы", cl_title: "Кластеры кошельков · аналитика групп", cl_goal: "Конечная цель потоков", cl_cps: "контрагентов", cl_basis: "база", cl_addr: "адресов", cl_avgbal: "ср. баланс группы", cl_in: "приход", cl_out: "отток", cl_analyzing: "Анализ ончейн-данных по сетям…", cl_cat_cex: "Биржи (CEX)", cl_cat_dex: "DEX / свопы", cl_cat_bridge: "Мосты", cl_cat_defi: "DeFi / контракты", cl_cat_eoa: "Кошельки (EOA)", cl_v_sell_cex: "Продажа / вывод на биржи", cl_v_swap_dex: "Обмен на DEX", cl_v_bridge: "Перемещение через мосты", cl_v_defi: "Взаимодействие с DeFi", cl_v_transfer: "Переводы на кошельки", cl_v_hold: "Удержание (нет оттока)", cl_timeline: "Активность во времени", cl_period: "Период", cl_coverage: "оценено ценами", cl_v_buy_cex: "Покупка / завод с бирж", cl_buy: "Покупка", cl_sell: "Продажа", cl_stance: "Позиция", cl_accumulating: "накапливает", cl_distributing: "раздаёт", cl_btn: "Кластеры", cl_run: "Анализировать", cl_ph: "0x… / BTC / Tron / Solana адрес", cl_2hop_btn: "Найти связанные группы (2-hop)", cl_2hop_loading: "Сканирование 2-hop…", cl_2hop_none: "Скоординированных групп не найдено", cl_2hop_group: "Группа", cl_2hop_direct: "прямая связь", cl_2hop_shared: "общий источник" },
};
for (const l of Object.keys(CL_TR) as Lang[]) Object.assign(TR[l], CL_TR[l]);

const BEHA_TR: Record<Lang, Record<string, string>> = {
  uk: { beh_alerts_h: "Поведінкові алерти (зміна цілі/позиції)", beh_arm: "Озброїти базу", beh_check: "Перевірити", beh_checking: "Перевірка…", beh_armed: "База збережена ✓", beh_no_base: "Спершу озброїти базу", beh_nochange: "Без змін", beh_stance_change: "Змінив позицію", beh_focus_change: "Змінив фокус", beh_sell_up: "Різко зріс продаж на біржі", beh_buy_up: "Різко зросла купівля з біржі", beh_netflip: "Чистий потік змінив напрям", beh_spike: "Сплеск активності", beh_alert_toast: "Зміна поведінки гаманця" },
  en: { beh_alerts_h: "Behavior alerts (goal/stance change)", beh_arm: "Arm baseline", beh_check: "Check", beh_checking: "Checking…", beh_armed: "Baseline saved ✓", beh_no_base: "Arm baseline first", beh_nochange: "No change", beh_stance_change: "Stance changed", beh_focus_change: "Focus changed", beh_sell_up: "Sell-to-exchange surged", beh_buy_up: "Buy-from-exchange surged", beh_netflip: "Net flow flipped", beh_spike: "Activity spike", beh_alert_toast: "Wallet behavior change" },
  de: { beh_alerts_h: "Verhaltensalarme (Ziel/Position)", beh_arm: "Basis setzen", beh_check: "Prüfen", beh_checking: "Prüfe…", beh_armed: "Basis gespeichert ✓", beh_no_base: "Zuerst Basis setzen", beh_nochange: "Keine Änderung", beh_stance_change: "Position geändert", beh_focus_change: "Fokus geändert", beh_sell_up: "Verkauf an Börse stark gestiegen", beh_buy_up: "Kauf von Börse stark gestiegen", beh_netflip: "Nettofluss umgekehrt", beh_spike: "Aktivitätsspitze", beh_alert_toast: "Verhaltensänderung der Wallet" },
  fr: { beh_alerts_h: "Alertes comportement (objectif/position)", beh_arm: "Définir base", beh_check: "Vérifier", beh_checking: "Vérification…", beh_armed: "Base enregistrée ✓", beh_no_base: "Définir la base d'abord", beh_nochange: "Aucun changement", beh_stance_change: "Position changée", beh_focus_change: "Focus changé", beh_sell_up: "Ventes vers plateforme en hausse", beh_buy_up: "Achats depuis plateforme en hausse", beh_netflip: "Flux net inversé", beh_spike: "Pic d'activité", beh_alert_toast: "Changement de comportement" },
  ru: { beh_alerts_h: "Поведенческие алерты (смена цели/позиции)", beh_arm: "Задать базу", beh_check: "Проверить", beh_checking: "Проверка…", beh_armed: "База сохранена ✓", beh_no_base: "Сначала задать базу", beh_nochange: "Без изменений", beh_stance_change: "Сменил позицию", beh_focus_change: "Сменил фокус", beh_sell_up: "Резко вырос сброс на биржу", beh_buy_up: "Резко выросла покупка с биржи", beh_netflip: "Чистый поток развернулся", beh_spike: "Всплеск активности", beh_alert_toast: "Изменение поведения кошелька" },
};
for (const l of Object.keys(BEHA_TR) as Lang[]) Object.assign(TR[l], BEHA_TR[l]);

const AG_TR: Record<Lang, Record<string, string>> = {
  uk: { ag_btn: "Групи крупного капіталу (40-80%)", ag_loading: "Аналіз пов'язаних холдерів…", ag_top: "Топ-холдерів", ag_sum: "сумарно", ag_largest: "найбільша група", ag_warn: "контролює великий капітал!", ag_group: "Група", ag_of_supply: "емісії", ag_no_groups: "Незалежних великих груп не виявлено — холдери не пов'язані." },
  en: { ag_btn: "Large-capital groups (40-80%)", ag_loading: "Analyzing linked holders…", ag_top: "Top holders", ag_sum: "total", ag_largest: "largest group", ag_warn: "controls major supply!", ag_group: "Group", ag_of_supply: "of supply", ag_no_groups: "No coordinated large groups — holders unlinked." },
  de: { ag_btn: "Großkapital-Gruppen (40-80%)", ag_loading: "Analysiere verbundene Halter…", ag_top: "Top-Halter", ag_sum: "gesamt", ag_largest: "größte Gruppe", ag_warn: "kontrolliert großes Kapital!", ag_group: "Gruppe", ag_of_supply: "des Angebots", ag_no_groups: "Keine koordinierten Großgruppen." },
  fr: { ag_btn: "Groupes de gros capital (40-80%)", ag_loading: "Analyse des détenteurs liés…", ag_top: "Top détenteurs", ag_sum: "total", ag_largest: "plus grand groupe", ag_warn: "contrôle un gros capital !", ag_group: "Groupe", ag_of_supply: "de l'offre", ag_no_groups: "Aucun grand groupe coordonné." },
  ru: { ag_btn: "Группы крупного капитала (40-80%)", ag_loading: "Анализ связанных холдеров…", ag_top: "Топ-холдеров", ag_sum: "суммарно", ag_largest: "крупнейшая группа", ag_warn: "контролирует крупный капитал!", ag_group: "Группа", ag_of_supply: "эмиссии", ag_no_groups: "Скоординированных крупных групп не найдено." },
};
for (const l of Object.keys(AG_TR) as Lang[]) Object.assign(TR[l], AG_TR[l]);

const SM_TR: Record<Lang, Record<string, string>> = {
  uk: { sm_btn: "Smart Money", sm_title: "Smart Money — крупний капітал", sm_assets_h: "Активи з концентрацією крупного капіталу", sm_players_h: "Гравці у кількох активах (зв'язки)", sm_loading: "Завантаження…", sm_band_note: "смуга 40-80% = координовані кити", sm_in_assets: "активів", sm_empty: "Дані ще не зібрано (сканер не запускався).", sm_graph_btn: "Граф гравців", sm_forensics_h: "🔬 Координовані групи холдерів (forensics)", sm_coord_groups: "груп", sm_tier: "концентрація", sm_group_by: "Групувати:", sm_group_sector: "Сектор", sm_group_conc: "Концентрація", sm_filter_all: "Всі", narr_title: "Наративи — куди йдуть смарт-гроші", narr_hot: "гаряче", narr_conc: "конц.", narr_assets: "активів", narr_entries: "нових заходів", narr_quiet: "тихо", rot_title: "Ротація наративів — нетто-потік китів по місяцях", fam_title: "Китові родини", fam_open: "Родини", fam_families: "родин", fam_wallets: "гаманців", fam_family: "Родина", fam_links: "звʼязків", fam_emission: "частка емісії", fam_note: "згруповано за funding-графом, хаби виключено", ask_ph: "Запитай про китів, родини, наративи…", ask_btn: "Запитати", ask_thinking: "Думаю…", wf_label: "Частина родини", fam_accum: "\ud83d\udcc8 набирає", fam_distrib: "\ud83d\udcc9 роздає", sig_title: "Сигнали китів — активний потік", sig_distrib: "Роздають", sig_accum: "Набирають", sec_depin: "📡 DePIN", sec_memecoin: "🐸 Мемкоїни", sec_defi: "🏦 DeFi", sec_rwa: "🏛 RWA", sec_other: "◽ Інші" },
  en: { sm_btn: "Smart Money", sm_title: "Smart Money — large capital", sm_assets_h: "Assets with large-capital concentration", sm_players_h: "Players across multiple assets (links)", sm_loading: "Loading…", sm_band_note: "40-80% band = coordinated whales", sm_in_assets: "assets", sm_empty: "No data yet (scanner not run).", sm_graph_btn: "Players graph", sm_forensics_h: "🔬 Coordinated holder groups (forensics)", sm_coord_groups: "groups", sm_tier: "concentration", sm_group_by: "Group by:", sm_group_sector: "Sector", sm_group_conc: "Concentration", sm_filter_all: "All", narr_title: "Narratives — where smart money flows", narr_hot: "hot", narr_conc: "conc.", narr_assets: "assets", narr_entries: "new entries", narr_quiet: "quiet", rot_title: "Narrative rotation — monthly net whale flow", fam_title: "Whale families", fam_open: "Families", fam_families: "families", fam_wallets: "wallets", fam_family: "Family", fam_links: "links", fam_emission: "emission share", fam_note: "grouped by funding graph, hubs excluded", ask_ph: "Ask about whales, families, narratives…", ask_btn: "Ask", ask_thinking: "Thinking…", wf_label: "Part of a family", fam_accum: "\ud83d\udcc8 accumulating", fam_distrib: "\ud83d\udcc9 distributing", sig_title: "Whale signals — active flow", sig_distrib: "Distributing", sig_accum: "Accumulating", sec_depin: "📡 DePIN", sec_memecoin: "🐸 Memecoins", sec_defi: "🏦 DeFi", sec_rwa: "🏛 RWA", sec_other: "◽ Others" },
  de: { sm_btn: "Smart Money", sm_title: "Smart Money — Großkapital", sm_assets_h: "Assets mit Großkapital-Konzentration", sm_players_h: "Akteure in mehreren Assets (Verbindungen)", sm_loading: "Laden…", sm_band_note: "40-80%-Band = koordinierte Wale", sm_in_assets: "Assets", sm_empty: "Noch keine Daten.", sm_graph_btn: "Akteursgraph", sm_forensics_h: "🔬 Koordinierte Halter-Gruppen", sm_coord_groups: "Gruppen", sm_tier: "Konzentration" },
  fr: { sm_btn: "Smart Money", sm_title: "Smart Money — gros capital", sm_assets_h: "Actifs à concentration de gros capital", sm_players_h: "Acteurs sur plusieurs actifs (liens)", sm_loading: "Chargement…", sm_band_note: "bande 40-80% = baleines coordonnées", sm_in_assets: "actifs", sm_empty: "Pas encore de données.", sm_graph_btn: "Graphe acteurs", sm_forensics_h: "🔬 Groupes de détenteurs coordonnés", sm_coord_groups: "groupes", sm_tier: "concentration" },
  ru: { sm_btn: "Smart Money", sm_title: "Smart Money — крупный капитал", sm_assets_h: "Активы с концентрацией крупного капитала", sm_players_h: "Игроки в нескольких активах (связи)", sm_loading: "Загрузка…", sm_band_note: "полоса 40-80% = координированные киты", sm_in_assets: "активов", sm_empty: "Данные ещё не собраны.", sm_graph_btn: "Граф игроков", sm_forensics_h: "🔬 Скоординированные группы холдеров", sm_coord_groups: "групп", sm_tier: "концентрация" },
};
for (const l of Object.keys(SM_TR) as Lang[]) Object.assign(TR[l], SM_TR[l]);

const WA_TR: Record<Lang, Record<string, string>> = {
  uk: { wa_title: "Активи та обсяги (за весь час)", wa_in: "прихід", wa_out: "відтік", wa_txs: "транзакцій", wa_chains: "мереж", wa_period: "період", wa_days: "дн.", wa_total: "Сума активів", wa_assets: "активів" },
  en: { wa_title: "Assets & volumes (all-time)", wa_in: "in", wa_out: "out", wa_txs: "txs", wa_chains: "chains", wa_period: "period", wa_days: "days", wa_total: "Total assets", wa_assets: "assets" },
  de: { wa_title: "Assets & Volumen (gesamt)", wa_in: "Ein", wa_out: "Aus", wa_txs: "Txs", wa_chains: "Chains", wa_period: "Zeitraum", wa_days: "Tage", wa_total: "Summe", wa_assets: "Assets" },
  fr: { wa_title: "Actifs et volumes (total)", wa_in: "entrée", wa_out: "sortie", wa_txs: "txs", wa_chains: "chaînes", wa_period: "période", wa_days: "jours", wa_total: "Total actifs", wa_assets: "actifs" },
  ru: { wa_title: "Активы и объёмы (за всё время)", wa_in: "приход", wa_out: "отток", wa_txs: "транзакций", wa_chains: "сетей", wa_period: "период", wa_days: "дн.", wa_total: "Сумма активов", wa_assets: "активов" },
};
for (const l of Object.keys(WA_TR) as Lang[]) Object.assign(TR[l], WA_TR[l]);

const HP_TR2: Record<Lang, Record<string, string>> = {
  uk: { hp_workers: "Воркери", hp_ok: "ОК", hp_stale: "застій" }, en: { hp_workers: "Workers", hp_ok: "OK", hp_stale: "stale" },
  de: { hp_workers: "Worker", hp_ok: "OK", hp_stale: "veraltet" }, fr: { hp_workers: "Workers", hp_ok: "OK", hp_stale: "obsolète" }, ru: { hp_workers: "Воркеры", hp_ok: "ОК", hp_stale: "застой" },
};
for (const l of Object.keys(HP_TR2) as Lang[]) Object.assign(TR[l], HP_TR2[l]);

// Блок «Групи капіталу» (варіант E): KPI, панель процесів, картки груп із вкладками.
// Множина: _one/_few/_many (uk/ru — три форми; en/de/fr — _few = _many).
// cgw_* — назви воркерів за `key` з /api/workers-health (бекенд віддає label лише українською).
const CGV_TR: Record<Lang, Record<string, string>> = {
  uk: {
    cgv_fresh: "Дані груп оновлено {ago}", cgv_fresh_none: "Дані груп ще не формувались",
    cgv_groups_one: "група", cgv_groups_few: "групи", cgv_groups_many: "груп",
    cgv_wallets_one: "гаманець", cgv_wallets_few: "гаманці", cgv_wallets_many: "гаманців",
    cgv_whales_one: "кит", cgv_whales_few: "кити", cgv_whales_many: "китів",
    cgv_links_one: "зв'язок", cgv_links_few: "зв'язки", cgv_links_many: "зв'язків",
    cgv_volume: "обсяг",
    cgv_proc: "Фонові процеси", cgv_working: "{n} з {total} працюють", cgv_hdr_age: "оновлено · норма",
    cgv_lg_ok_one: "працює", cgv_lg_ok_few: "працюють", cgv_lg_ok_many: "працюють",
    cgv_lg_run_one: "обробляє", cgv_lg_run_few: "обробляють", cgv_lg_run_many: "обробляють",
    cgv_lg_stale: "застій",
    cgv_lg_never_one: "ще не запускався", cgv_lg_never_few: "ще не запускались", cgv_lg_never_many: "ще не запускались",
    cgv_never: "ще не запускався", cgv_running: "обробляє {n} з {total}",
    cgv_ago_now: "щойно", cgv_ago_min: "{n} хв тому", cgv_ago_h: "{n} год тому", cgv_ago_d: "{n} дн тому",
    cgv_norm_min: "≤ {n} хв", cgv_norm_h: "≤ {n} год",
    cgv_found: "Знайдені групи", cgv_sorted: "за контролем емісії",
    cgv_empty_t: "Групи ще не зібрано", cgv_empty_h: "Запустіть крос-активний аналіз — групи з'являться тут.",
    cgv_nth: "{n}-{suf} група", cgv_type_asset: "За активом", cgv_type_desk: "Мульти-монетний деск",
    cgv_emission: "Контроль емісії", cgv_comp: "Склад", cgv_core: "ядро", cgv_holder: "холдер",
    cgv_core_n: "{n} ядро", cgv_holders_one: "холдер", cgv_holders_few: "холдери", cgv_holders_many: "холдерів",
    cgv_tab_members: "Учасники", cgv_tab_link: "Зв'язок", cgv_tab_coord: "Координація", cgv_tabs_label: "Дії з групою {n}",
    cgv_more: "+ ще {n}", cgv_less: "Згорнути", cgv_shown: "Показано {n} з {total}",
    cgv_path_between: "Шлях між першими двома гаманцями групи", cgv_member: "учасник групи", cgv_hop: "проміжний гаманець",
    cgv_show_path: "Показати шлях на графі", cgv_timeout: "Сервер не відповів за 60 с — перевірка триває довше.", cgv_retry: "Спробувати ще раз", cgv_nolink_hint: "Шукали між першими двома гаманцями групи в усіх мережах.",
    cgv_hops_one: "коліно", cgv_hops_few: "коліна", cgv_hops_many: "колін",
    cgv_funders_one: "спільний фандер", cgv_funders_few: "спільні фандери", cgv_funders_many: "спільних фандерів",
    cgw_scan: "Скан активів", cgw_whale: "Мисливець на китів", cgw_forensics: "Forensics", cgw_conductors: "Диригенти",
    cgw_active: "Активні групи", cgw_graph: "Граф БД", cgw_backtest: "Бектест",
  },
  en: {
    cgv_fresh: "Group data updated {ago}", cgv_fresh_none: "Group data not built yet",
    cgv_groups_one: "group", cgv_groups_few: "groups", cgv_groups_many: "groups",
    cgv_wallets_one: "wallet", cgv_wallets_few: "wallets", cgv_wallets_many: "wallets",
    cgv_whales_one: "whale", cgv_whales_few: "whales", cgv_whales_many: "whales",
    cgv_links_one: "link", cgv_links_few: "links", cgv_links_many: "links",
    cgv_volume: "volume",
    cgv_proc: "Background processes", cgv_working: "{n} of {total} running", cgv_hdr_age: "updated · limit",
    cgv_lg_ok_one: "running", cgv_lg_ok_few: "running", cgv_lg_ok_many: "running",
    cgv_lg_run_one: "processing", cgv_lg_run_few: "processing", cgv_lg_run_many: "processing",
    cgv_lg_stale: "stale",
    cgv_lg_never_one: "never started", cgv_lg_never_few: "never started", cgv_lg_never_many: "never started",
    cgv_never: "never started", cgv_running: "processing {n} of {total}",
    cgv_ago_now: "just now", cgv_ago_min: "{n} min ago", cgv_ago_h: "{n} h ago", cgv_ago_d: "{n} d ago",
    cgv_norm_min: "≤ {n} min", cgv_norm_h: "≤ {n} h",
    cgv_found: "Groups found", cgv_sorted: "by supply controlled",
    cgv_empty_t: "No groups collected yet", cgv_empty_h: "Run the cross-asset analysis — groups will appear here.",
    cgv_nth: "Group {n}", cgv_type_asset: "By asset", cgv_type_desk: "Multi-coin desk",
    cgv_emission: "Supply controlled", cgv_comp: "Composition", cgv_core: "core", cgv_holder: "holder",
    cgv_core_n: "{n} core", cgv_holders_one: "holder", cgv_holders_few: "holders", cgv_holders_many: "holders",
    cgv_tab_members: "Members", cgv_tab_link: "Link", cgv_tab_coord: "Coordination", cgv_tabs_label: "Group {n} actions",
    cgv_more: "+ {n} more", cgv_less: "Collapse", cgv_shown: "Showing {n} of {total}",
    cgv_path_between: "Path between the group's first two wallets", cgv_member: "group member", cgv_hop: "intermediate wallet",
    cgv_show_path: "Show path on graph", cgv_timeout: "No response from the server within 60 s — the check takes longer.", cgv_retry: "Try again", cgv_nolink_hint: "Searched between the group's first two wallets across all chains.",
    cgv_hops_one: "hop", cgv_hops_few: "hops", cgv_hops_many: "hops",
    cgv_funders_one: "common funder", cgv_funders_few: "common funders", cgv_funders_many: "common funders",
    cgw_scan: "Asset scan", cgw_whale: "Whale finder", cgw_forensics: "Forensics", cgw_conductors: "Conductors",
    cgw_active: "Active groups", cgw_graph: "Graph DB", cgw_backtest: "Backtest",
  },
  de: {
    cgv_fresh: "Gruppendaten aktualisiert {ago}", cgv_fresh_none: "Gruppendaten noch nicht erstellt",
    cgv_groups_one: "Gruppe", cgv_groups_few: "Gruppen", cgv_groups_many: "Gruppen",
    cgv_wallets_one: "Wallet", cgv_wallets_few: "Wallets", cgv_wallets_many: "Wallets",
    cgv_whales_one: "Wal", cgv_whales_few: "Wale", cgv_whales_many: "Wale",
    cgv_links_one: "Verbindung", cgv_links_few: "Verbindungen", cgv_links_many: "Verbindungen",
    cgv_volume: "Volumen",
    cgv_proc: "Hintergrundprozesse", cgv_working: "{n} von {total} laufen", cgv_hdr_age: "aktualisiert · Limit",
    cgv_lg_ok_one: "läuft", cgv_lg_ok_few: "laufen", cgv_lg_ok_many: "laufen",
    cgv_lg_run_one: "verarbeitet", cgv_lg_run_few: "verarbeiten", cgv_lg_run_many: "verarbeiten",
    cgv_lg_stale: "veraltet",
    cgv_lg_never_one: "nie gestartet", cgv_lg_never_few: "nie gestartet", cgv_lg_never_many: "nie gestartet",
    cgv_never: "nie gestartet", cgv_running: "verarbeitet {n} von {total}",
    cgv_ago_now: "gerade eben", cgv_ago_min: "vor {n} Min.", cgv_ago_h: "vor {n} Std.", cgv_ago_d: "vor {n} T.",
    cgv_norm_min: "≤ {n} Min.", cgv_norm_h: "≤ {n} Std.",
    cgv_found: "Gefundene Gruppen", cgv_sorted: "nach Angebotskontrolle",
    cgv_empty_t: "Noch keine Gruppen erfasst", cgv_empty_h: "Starten Sie die Cross-Asset-Analyse — Gruppen erscheinen hier.",
    cgv_nth: "Gruppe {n}", cgv_type_asset: "Nach Asset", cgv_type_desk: "Multi-Coin-Desk",
    cgv_emission: "Angebotskontrolle", cgv_comp: "Zusammensetzung", cgv_core: "Kern", cgv_holder: "Halter",
    cgv_core_n: "{n} Kern", cgv_holders_one: "Halter", cgv_holders_few: "Halter", cgv_holders_many: "Halter",
    cgv_tab_members: "Mitglieder", cgv_tab_link: "Verbindung", cgv_tab_coord: "Koordination", cgv_tabs_label: "Aktionen für Gruppe {n}",
    cgv_more: "+ {n} weitere", cgv_less: "Einklappen", cgv_shown: "{n} von {total} angezeigt",
    cgv_path_between: "Pfad zwischen den ersten zwei Wallets der Gruppe", cgv_member: "Gruppenmitglied", cgv_hop: "Zwischen-Wallet",
    cgv_show_path: "Pfad im Graph zeigen", cgv_timeout: "Keine Antwort des Servers innerhalb von 60 s — die Prüfung dauert länger.", cgv_retry: "Erneut versuchen", cgv_nolink_hint: "Gesucht zwischen den ersten zwei Wallets der Gruppe über alle Chains.",
    cgv_hops_one: "Sprung", cgv_hops_few: "Sprünge", cgv_hops_many: "Sprünge",
    cgv_funders_one: "gemeinsamer Geldgeber", cgv_funders_few: "gemeinsame Geldgeber", cgv_funders_many: "gemeinsame Geldgeber",
    cgw_scan: "Asset-Scan", cgw_whale: "Wal-Finder", cgw_forensics: "Forensik", cgw_conductors: "Dirigenten",
    cgw_active: "Aktive Gruppen", cgw_graph: "Graph-DB", cgw_backtest: "Backtest",
  },
  fr: {
    cgv_fresh: "Données des groupes mises à jour {ago}", cgv_fresh_none: "Données des groupes pas encore construites",
    cgv_groups_one: "groupe", cgv_groups_few: "groupes", cgv_groups_many: "groupes",
    cgv_wallets_one: "portefeuille", cgv_wallets_few: "portefeuilles", cgv_wallets_many: "portefeuilles",
    cgv_whales_one: "baleine", cgv_whales_few: "baleines", cgv_whales_many: "baleines",
    cgv_links_one: "lien", cgv_links_few: "liens", cgv_links_many: "liens",
    cgv_volume: "volume",
    cgv_proc: "Processus d'arrière-plan", cgv_working: "{n} sur {total} actifs", cgv_hdr_age: "mis à jour · limite",
    cgv_lg_ok_one: "actif", cgv_lg_ok_few: "actifs", cgv_lg_ok_many: "actifs",
    cgv_lg_run_one: "en traitement", cgv_lg_run_few: "en traitement", cgv_lg_run_many: "en traitement",
    cgv_lg_stale: "en retard",
    cgv_lg_never_one: "jamais lancé", cgv_lg_never_few: "jamais lancés", cgv_lg_never_many: "jamais lancés",
    cgv_never: "jamais lancé", cgv_running: "traite {n} sur {total}",
    cgv_ago_now: "à l'instant", cgv_ago_min: "il y a {n} min", cgv_ago_h: "il y a {n} h", cgv_ago_d: "il y a {n} j",
    cgv_norm_min: "≤ {n} min", cgv_norm_h: "≤ {n} h",
    cgv_found: "Groupes trouvés", cgv_sorted: "par contrôle de l'offre",
    cgv_empty_t: "Aucun groupe collecté", cgv_empty_h: "Lancez l'analyse cross-actifs — les groupes apparaîtront ici.",
    cgv_nth: "Groupe {n}", cgv_type_asset: "Par actif", cgv_type_desk: "Desk multi-jetons",
    cgv_emission: "Offre contrôlée", cgv_comp: "Composition", cgv_core: "noyau", cgv_holder: "détenteur",
    cgv_core_n: "{n} noyau", cgv_holders_one: "détenteur", cgv_holders_few: "détenteurs", cgv_holders_many: "détenteurs",
    cgv_tab_members: "Membres", cgv_tab_link: "Lien", cgv_tab_coord: "Coordination", cgv_tabs_label: "Actions du groupe {n}",
    cgv_more: "+ {n} de plus", cgv_less: "Réduire", cgv_shown: "{n} sur {total} affichés",
    cgv_path_between: "Chemin entre les deux premiers portefeuilles du groupe", cgv_member: "membre du groupe", cgv_hop: "portefeuille intermédiaire",
    cgv_show_path: "Voir le chemin sur le graphe", cgv_timeout: "Pas de réponse du serveur en 60 s — la vérification prend plus de temps.", cgv_retry: "Réessayer", cgv_nolink_hint: "Recherche entre les deux premiers portefeuilles du groupe sur toutes les chaînes.",
    cgv_hops_one: "saut", cgv_hops_few: "sauts", cgv_hops_many: "sauts",
    cgv_funders_one: "financeur commun", cgv_funders_few: "financeurs communs", cgv_funders_many: "financeurs communs",
    cgw_scan: "Scan des actifs", cgw_whale: "Chasseur de baleines", cgw_forensics: "Forensique", cgw_conductors: "Chefs d'orchestre",
    cgw_active: "Groupes actifs", cgw_graph: "Base du graphe", cgw_backtest: "Backtest",
  },
  ru: {
    cgv_fresh: "Данные групп обновлены {ago}", cgv_fresh_none: "Данные групп ещё не формировались",
    cgv_groups_one: "группа", cgv_groups_few: "группы", cgv_groups_many: "групп",
    cgv_wallets_one: "кошелёк", cgv_wallets_few: "кошелька", cgv_wallets_many: "кошельков",
    cgv_whales_one: "кит", cgv_whales_few: "кита", cgv_whales_many: "китов",
    cgv_links_one: "связь", cgv_links_few: "связи", cgv_links_many: "связей",
    cgv_volume: "объём",
    cgv_proc: "Фоновые процессы", cgv_working: "{n} из {total} работают", cgv_hdr_age: "обновлено · норма",
    cgv_lg_ok_one: "работает", cgv_lg_ok_few: "работают", cgv_lg_ok_many: "работают",
    cgv_lg_run_one: "обрабатывает", cgv_lg_run_few: "обрабатывают", cgv_lg_run_many: "обрабатывают",
    cgv_lg_stale: "застой",
    cgv_lg_never_one: "ещё не запускался", cgv_lg_never_few: "ещё не запускались", cgv_lg_never_many: "ещё не запускались",
    cgv_never: "ещё не запускался", cgv_running: "обрабатывает {n} из {total}",
    cgv_ago_now: "только что", cgv_ago_min: "{n} мин назад", cgv_ago_h: "{n} ч назад", cgv_ago_d: "{n} дн назад",
    cgv_norm_min: "≤ {n} мин", cgv_norm_h: "≤ {n} ч",
    cgv_found: "Найденные группы", cgv_sorted: "по контролю эмиссии",
    cgv_empty_t: "Группы ещё не собраны", cgv_empty_h: "Запустите кросс-активный анализ — группы появятся здесь.",
    cgv_nth: "{n}-я группа", cgv_type_asset: "По активу", cgv_type_desk: "Мультимонетный деск",
    cgv_emission: "Контроль эмиссии", cgv_comp: "Состав", cgv_core: "ядро", cgv_holder: "холдер",
    cgv_core_n: "{n} ядро", cgv_holders_one: "холдер", cgv_holders_few: "холдера", cgv_holders_many: "холдеров",
    cgv_tab_members: "Участники", cgv_tab_link: "Связь", cgv_tab_coord: "Координация", cgv_tabs_label: "Действия с группой {n}",
    cgv_more: "+ ещё {n}", cgv_less: "Свернуть", cgv_shown: "Показано {n} из {total}",
    cgv_path_between: "Путь между первыми двумя кошельками группы", cgv_member: "участник группы", cgv_hop: "промежуточный кошелёк",
    cgv_show_path: "Показать путь на графе", cgv_timeout: "Сервер не ответил за 60 с — проверка идёт дольше.", cgv_retry: "Повторить", cgv_nolink_hint: "Искали между первыми двумя кошельками группы во всех сетях.",
    cgv_hops_one: "колено", cgv_hops_few: "колена", cgv_hops_many: "колен",
    cgv_funders_one: "общий фандер", cgv_funders_few: "общих фандера", cgv_funders_many: "общих фандеров",
    cgw_scan: "Скан активов", cgw_whale: "Охотник на китов", cgw_forensics: "Forensics", cgw_conductors: "Дирижёры",
    cgw_active: "Активные группы", cgw_graph: "Граф БД", cgw_backtest: "Бэктест",
  },
};
for (const l of Object.keys(CGV_TR) as Lang[]) Object.assign(TR[l], CGV_TR[l]);
const REP_TR: Record<Lang, Record<string, string>> = {
  uk: { rep_btn: "Звіт + прогноз", rep_gen: "Генеруємо аналітичний звіт…", rep_conf: "впевненість", rep_why: "Чому такий прогноз", rep_nodata: "Недостатньо сигналів для прогнозу", rep_note: "Прогноз — пояснювана евристика з ончейн-сигналів, не фінансова порада.", rep_export: "Експорт досьє (HTML/PDF)", rep_tp: "Драбина фіксації тіла", rep_tp_take: "фіксувати тіло: продати", rep_tp_approach: "наближається до 3x", rep_tp_body: "тіло" },
  en: { rep_btn: "Report + forecast", rep_gen: "Generating report…", rep_conf: "confidence", rep_why: "Why this forecast", rep_nodata: "Not enough signals", rep_note: "Forecast is an explainable heuristic from on-chain signals, not financial advice.", rep_export: "Export dossier (HTML/PDF)", rep_tp: "Take-profit ladder", rep_tp_take: "recover principal: sell", rep_tp_approach: "approaching 3x", rep_tp_body: "principal" },
  de: { rep_btn: "Bericht + Prognose", rep_gen: "Bericht wird erstellt…", rep_conf: "Konfidenz", rep_why: "Warum diese Prognose", rep_nodata: "Zu wenig Signale", rep_note: "Prognose ist eine erklärbare Heuristik, keine Finanzberatung.", rep_export: "Dossier exportieren (HTML/PDF)", rep_tp: "Gewinnmitnahme-Leiter", rep_tp_take: "Einsatz sichern: verkaufe", rep_tp_approach: "nähert sich 3x", rep_tp_body: "Einsatz" },
  fr: { rep_btn: "Rapport + prévision", rep_gen: "Génération du rapport…", rep_conf: "confiance", rep_why: "Pourquoi cette prévision", rep_nodata: "Pas assez de signaux", rep_note: "Prévision = heuristique explicable, pas un conseil financier.", rep_export: "Exporter le dossier (HTML/PDF)", rep_tp: "Échelle de prise de profit", rep_tp_take: "récupérer la mise: vendre", rep_tp_approach: "approche de 3x", rep_tp_body: "mise" },
  ru: { rep_btn: "Отчёт + прогноз", rep_gen: "Генерируем отчёт…", rep_conf: "уверенность", rep_why: "Почему такой прогноз", rep_nodata: "Недостаточно сигналов", rep_note: "Прогноз — объяснимая эвристика из ончейн-сигналов, не финсовет.", rep_export: "Экспорт досье (HTML/PDF)", rep_tp: "Лестница фиксации тела", rep_tp_take: "фиксировать тело: продать", rep_tp_approach: "приближается к 3x", rep_tp_body: "тело" },
};
for (const l of Object.keys(REP_TR) as Lang[]) Object.assign(TR[l], REP_TR[l]);
const DOS_TR: Record<Lang, Record<string, string>> = {
  uk: { dos_title: "Досьє гаманця", dos_verdict: "Вердикт і прогноз", dos_summary: "Резюме", dos_metrics: "Метрики", dos_volume: "Обсяг", dos_assets: "Активів", dos_score: "Smart-money скоринг", dos_holdings: "Холдинги (топ)", dos_generated: "Згенеровано Wallet Radar" },
  en: { dos_title: "Wallet dossier", dos_verdict: "Verdict & forecast", dos_summary: "Summary", dos_metrics: "Metrics", dos_volume: "Volume", dos_assets: "Assets", dos_score: "Smart-money score", dos_holdings: "Holdings (top)", dos_generated: "Generated by Wallet Radar" },
  de: { dos_title: "Wallet-Dossier", dos_verdict: "Urteil & Prognose", dos_summary: "Zusammenfassung", dos_metrics: "Kennzahlen", dos_volume: "Volumen", dos_assets: "Assets", dos_score: "Smart-Money-Score", dos_holdings: "Bestände (Top)", dos_generated: "Erstellt von Wallet Radar" },
  fr: { dos_title: "Dossier du portefeuille", dos_verdict: "Verdict et prévision", dos_summary: "Résumé", dos_metrics: "Métriques", dos_volume: "Volume", dos_assets: "Actifs", dos_score: "Score smart-money", dos_holdings: "Avoirs (top)", dos_generated: "Généré par Wallet Radar" },
  ru: { dos_title: "Досье кошелька", dos_verdict: "Вердикт и прогноз", dos_summary: "Резюме", dos_metrics: "Метрики", dos_volume: "Объём", dos_assets: "Активов", dos_score: "Smart-money скоринг", dos_holdings: "Холдинги (топ)", dos_generated: "Сгенерировано Wallet Radar" },
};
for (const l of Object.keys(DOS_TR) as Lang[]) Object.assign(TR[l], DOS_TR[l]);
const EV_TR: Record<Lang, Record<string, string>> = {
  uk: { ev_conductor: "🎼 Новий диригент", ev_bgroup: "🐋 $1B+ координована група", ev_bwhale: "🐋 $1B+ кит", ev_funding: "🎼 Диригент профінансував", ev_smart: "🟢 Smart Money: новий токен", ev_active: "🔥 Активна група", ev_trace: "🐋 Слід китів", ev_move_acc: "🟢 Синхронно накопичує", ev_move_dist: "🔴 Синхронно роздає", ev_members: "членів", ev_liq: "💥 Ліквідаційний флеш", ev_liq_long: "лонги", ev_liq_short: "шорти" },
  en: { ev_conductor: "🎼 New conductor", ev_bgroup: "🐋 $1B+ coordinated group", ev_bwhale: "🐋 $1B+ whale", ev_funding: "🎼 Conductor funded", ev_smart: "🟢 Smart Money: new token", ev_active: "🔥 Active group", ev_trace: "🐋 Whale trace", ev_move_acc: "🟢 Synchronously accumulating", ev_move_dist: "🔴 Synchronously distributing", ev_members: "members", ev_liq: "💥 Liquidation flush", ev_liq_long: "longs", ev_liq_short: "shorts" },
  de: { ev_conductor: "🎼 Neuer Dirigent", ev_bgroup: "🐋 $1B+ koordinierte Gruppe", ev_bwhale: "🐋 $1B+ Wal", ev_funding: "🎼 Dirigent finanzierte", ev_smart: "🟢 Smart Money: neuer Token", ev_active: "🔥 Aktive Gruppe", ev_trace: "🐋 Wal-Spur", ev_move_acc: "🟢 Akkumuliert synchron", ev_move_dist: "🔴 Verteilt synchron", ev_members: "Mitglieder", ev_liq: "💥 Liquidations-Flush", ev_liq_long: "Longs", ev_liq_short: "Shorts" },
  fr: { ev_conductor: "🎼 Nouveau chef d'orchestre", ev_bgroup: "🐋 Groupe coordonné $1B+", ev_bwhale: "🐋 Baleine $1B+", ev_funding: "🎼 Chef a financé", ev_smart: "🟢 Smart Money : nouveau token", ev_active: "🔥 Groupe actif", ev_trace: "🐋 Trace de baleine", ev_move_acc: "🟢 Accumule en synchrone", ev_move_dist: "🔴 Distribue en synchrone", ev_members: "membres", ev_liq: "💥 Flush de liquidations", ev_liq_long: "longs", ev_liq_short: "shorts" },
  ru: { ev_conductor: "🎼 Новый дирижёр", ev_bgroup: "🐋 $1B+ координированная группа", ev_bwhale: "🐋 $1B+ кит", ev_funding: "🎼 Дирижёр профинансировал", ev_smart: "🟢 Smart Money: новый токен", ev_active: "🔥 Активная группа", ev_trace: "🐋 След китов", ev_move_acc: "🟢 Синхронно накапливает", ev_move_dist: "🔴 Синхронно раздаёт", ev_members: "участников", ev_liq: "💥 Ликвидационный флеш", ev_liq_long: "лонги", ev_liq_short: "шорты" },
};
for (const l of Object.keys(EV_TR) as Lang[]) Object.assign(TR[l], EV_TR[l]);
const LIQ_TR: Record<Lang, Record<string, string>> = {
  uk: { liq_h: "Ліквідації фʼючерсів (Bybit)", liq_long: "Лонги", liq_short: "Шорти", liq_total: "Разом", liq_top: "Топ активів", liq_5m: "5хв", liq_1h: "1год", liq_24h: "24год", liq_off: "монітор підключається…", liq_flush: "флеш-поріг" },
  en: { liq_h: "Futures liquidations (Bybit)", liq_long: "Longs", liq_short: "Shorts", liq_total: "Total", liq_top: "Top assets", liq_5m: "5m", liq_1h: "1h", liq_24h: "24h", liq_off: "monitor connecting…", liq_flush: "flush threshold" },
  de: { liq_h: "Futures-Liquidationen (Bybit)", liq_long: "Longs", liq_short: "Shorts", liq_total: "Gesamt", liq_top: "Top-Assets", liq_5m: "5Min", liq_1h: "1Std", liq_24h: "24Std", liq_off: "Monitor verbindet…", liq_flush: "Flush-Schwelle" },
  fr: { liq_h: "Liquidations futures (Bybit)", liq_long: "Longs", liq_short: "Shorts", liq_total: "Total", liq_top: "Top actifs", liq_5m: "5min", liq_1h: "1h", liq_24h: "24h", liq_off: "moniteur en connexion…", liq_flush: "seuil de flush" },
  ru: { liq_h: "Ликвидации фьючерсов (Bybit)", liq_long: "Лонги", liq_short: "Шорты", liq_total: "Всего", liq_top: "Топ активов", liq_5m: "5м", liq_1h: "1ч", liq_24h: "24ч", liq_off: "монитор подключается…", liq_flush: "порог флеша" },
};
for (const l of Object.keys(LIQ_TR) as Lang[]) Object.assign(TR[l], LIQ_TR[l]);
const AI_TR: Record<Lang, Record<string, string>> = {
  uk: { ai_title: "AI-асистент по токену", ai_placeholder: "Спитайте про токен…", ai_ask: "Спитати", ai_thinking: "Аналізую…", ai_q_phase: "Яка фаза?", ai_q_liq: "Ліквідність?", ai_q_pump: "Памп?", ai_q_leaders: "Хто в плюсі?", ai_a_whales_prof: "Кити в прибутку", ai_a_whales_loss: "Кити в мінусі", ai_a_leaders: "Найприбутковіші гаманці", ai_a_price: "Ціна", ai_a_nodata: "Недостатньо даних для відповіді", ai_note: "Відповідь на основі ончейн-метрик, не фінансова порада." },
  en: { ai_title: "Token AI assistant", ai_placeholder: "Ask about the token…", ai_ask: "Ask", ai_thinking: "Analyzing…", ai_q_phase: "What phase?", ai_q_liq: "Liquidity?", ai_q_pump: "Pump?", ai_q_leaders: "Who's in profit?", ai_a_whales_prof: "Whales in profit", ai_a_whales_loss: "Whales underwater", ai_a_leaders: "Most profitable wallets", ai_a_price: "Price", ai_a_nodata: "Not enough data to answer", ai_note: "Answer based on on-chain metrics, not financial advice." },
  de: { ai_title: "Token-KI-Assistent", ai_placeholder: "Frage zum Token…", ai_ask: "Fragen", ai_thinking: "Analysiere…", ai_q_phase: "Welche Phase?", ai_q_liq: "Liquidität?", ai_q_pump: "Pump?", ai_q_leaders: "Wer im Plus?", ai_a_whales_prof: "Wale im Plus", ai_a_whales_loss: "Wale im Minus", ai_a_leaders: "Profitabelste Wallets", ai_a_price: "Preis", ai_a_nodata: "Nicht genug Daten für eine Antwort", ai_note: "Antwort basiert auf On-Chain-Metriken, keine Finanzberatung." },
  fr: { ai_title: "Assistant IA du token", ai_placeholder: "Posez une question…", ai_ask: "Demander", ai_thinking: "Analyse…", ai_q_phase: "Quelle phase ?", ai_q_liq: "Liquidité ?", ai_q_pump: "Pump ?", ai_q_leaders: "Qui est en profit ?", ai_a_whales_prof: "Baleines en profit", ai_a_whales_loss: "Baleines en perte", ai_a_leaders: "Wallets les plus rentables", ai_a_price: "Prix", ai_a_nodata: "Pas assez de données pour répondre", ai_note: "Réponse basée sur des métriques on-chain, pas un conseil financier." },
  ru: { ai_title: "AI-ассистент по токену", ai_placeholder: "Спросите про токен…", ai_ask: "Спросить", ai_thinking: "Анализирую…", ai_q_phase: "Какая фаза?", ai_q_liq: "Ликвидность?", ai_q_pump: "Памп?", ai_q_leaders: "Кто в плюсе?", ai_a_whales_prof: "Киты в прибыли", ai_a_whales_loss: "Киты в минусе", ai_a_leaders: "Самые прибыльные кошельки", ai_a_price: "Цена", ai_a_nodata: "Недостаточно данных для ответа", ai_note: "Ответ на основе ончейн-метрик, не финсовет." },
};
for (const l of Object.keys(AI_TR) as Lang[]) Object.assign(TR[l], AI_TR[l]);
const FEAT_TR: Record<Lang, Record<string, string>> = {
  uk: { feat_conductor: "Диригент", feat_note: "Непублічний координатор %1: координує групи холдерів у %2 активах. Потік грошей конвергує на %3 (~%4)." },
  en: { feat_conductor: "Conductor", feat_note: "Non-public coordinator %1: coordinates holder groups across %2 assets. Cash flow converges to %3 (~%4)." },
  de: { feat_conductor: "Dirigent", feat_note: "Nicht-öffentlicher Koordinator %1: koordiniert Halter-Gruppen in %2 Assets. Geldfluss konvergiert zu %3 (~%4)." },
  fr: { feat_conductor: "Chef d'orchestre", feat_note: "Coordinateur non public %1 : coordonne des groupes de détenteurs sur %2 actifs. Les flux convergent vers %3 (~%4)." },
  ru: { feat_conductor: "Дирижёр", feat_note: "Непубличный координатор %1: координирует группы холдеров в %2 активах. Поток денег сходится на %3 (~%4)." },
};
for (const l of Object.keys(FEAT_TR) as Lang[]) Object.assign(TR[l], FEAT_TR[l]);
const CG_TR: Record<Lang, Record<string, string>> = {
  uk: { cg_title: "Групи капіталу", cg_sub: "Взаємопов'язані кити по знайдених активах", cg_members: "гаманців", cg_assets: "спільні активи", cg_empty: "Ще не зібрано (запустіть крос-актив аналіз)", cg_loading: "Завантаження…", cg_check: "🔗 Зв'язок", cg_checking: "Пошук по всіх мережах…", cg_linked: "Пов'язані через", cg_hops: "колін", cg_nolink: "Зв'язку до 15 колін не знайдено", cg_coord: "🔬 Координація", cg_likely: "Ймовірно скоординовані", cg_weak: "Слабкі сигнали", cg_common: "спільних фандерів", cg_direct: "пряме фінансування", cg_pat_peel_chain: "peel-chain (розподіл/відмивання)", cg_pat_gas_funding: "фінансування газу", cg_funder_gas: "газ", cg_funder_value: "актив", cg_open_group: "Розкрити групу (обсяг + статистика)", cg_group_vol: "Обсяг групи" },
  en: { cg_title: "Capital groups", cg_sub: "Interconnected whales across found assets", cg_members: "wallets", cg_assets: "shared assets", cg_empty: "Not collected yet (run cross-asset analysis)", cg_loading: "Loading…", cg_check: "🔗 Link", cg_checking: "Searching all chains…", cg_linked: "Linked via", cg_hops: "hops", cg_nolink: "No link within 15 hops", cg_coord: "🔬 Coordination", cg_likely: "Likely coordinated", cg_weak: "Weak signals", cg_common: "common funders", cg_direct: "direct funding", cg_pat_peel_chain: "peel-chain (layering)", cg_pat_gas_funding: "gas funding", cg_funder_gas: "gas", cg_funder_value: "value", cg_open_group: "Open group (volume + stats)", cg_group_vol: "Group volume" },
  de: { cg_title: "Kapitalgruppen", cg_sub: "Verbundene Wale über gefundene Assets", cg_members: "Wallets", cg_assets: "gemeinsame Assets", cg_empty: "Noch nicht erfasst", cg_loading: "Laden…", cg_check: "🔗 Verbindung", cg_checking: "Suche über alle Chains…", cg_linked: "Verbunden über", cg_hops: "Sprünge", cg_nolink: "Keine Verbindung in 15", cg_coord: "🔬 Koordination", cg_likely: "Wahrscheinlich koordiniert", cg_weak: "Schwache Signale", cg_common: "gemeinsame Geldgeber", cg_direct: "direkte Finanzierung", cg_pat_peel_chain: "Peel-Chain", cg_pat_gas_funding: "Gas-Finanzierung", cg_funder_gas: "Gas", cg_funder_value: "Wert", cg_open_group: "Gruppe öffnen (Volumen + Statistik)", cg_group_vol: "Gruppenvolumen" },
  fr: { cg_title: "Groupes de capital", cg_sub: "Baleines liées sur les actifs trouvés", cg_members: "portefeuilles", cg_assets: "actifs communs", cg_empty: "Pas encore collecté", cg_loading: "Chargement…", cg_check: "🔗 Lien", cg_checking: "Recherche toutes chaînes…", cg_linked: "Lié via", cg_hops: "sauts", cg_nolink: "Aucun lien en 15 sauts", cg_coord: "🔬 Coordination", cg_likely: "Probablement coordonnés", cg_weak: "Signaux faibles", cg_common: "financeurs communs", cg_direct: "financement direct", cg_pat_peel_chain: "peel-chain", cg_pat_gas_funding: "financement gaz", cg_funder_gas: "gaz", cg_funder_value: "valeur", cg_open_group: "Ouvrir le groupe (volume + stats)", cg_group_vol: "Volume du groupe" },
  ru: { cg_title: "Группы капитала", cg_sub: "Взаимосвязанные киты по найденным активам", cg_members: "кошельков", cg_assets: "общие активы", cg_empty: "Ещё не собрано", cg_loading: "Загрузка…", cg_check: "🔗 Связь", cg_checking: "Поиск по всем сетям…", cg_linked: "Связаны через", cg_hops: "колен", cg_nolink: "Связи в 15 коленах не найдено", cg_coord: "🔬 Координация", cg_likely: "Вероятно скоординированы", cg_weak: "Слабые сигналы", cg_common: "общих фандеров", cg_direct: "прямое финансирование", cg_pat_peel_chain: "peel-chain (распределение)", cg_pat_gas_funding: "финансирование газа", cg_funder_gas: "газ", cg_funder_value: "актив", cg_open_group: "Раскрыть группу (объём + статистика)", cg_group_vol: "Объём группы" },
};
for (const l of Object.keys(CG_TR) as Lang[]) Object.assign(TR[l], CG_TR[l]);

const CGF_TR: Record<Lang, Record<string, string>> = {
  uk: { cg_forensics_h: "🔬 Координація холдерів по активах", cg_backer: "бекер" },
  en: { cg_forensics_h: "🔬 Holder coordination by asset", cg_backer: "backer" },
  de: { cg_forensics_h: "🔬 Halter-Koordination je Asset", cg_backer: "Geldgeber" },
  fr: { cg_forensics_h: "🔬 Coordination par actif", cg_backer: "financeur" },
  ru: { cg_forensics_h: "🔬 Координация холдеров по активам", cg_backer: "бэкер" },
};
for (const l of Object.keys(CGF_TR) as Lang[]) Object.assign(TR[l], CGF_TR[l]);
const WCL_TR: Record<Lang, Record<string, string>> = {
  uk: { wc_h: "🐳 Китові кластери (крос-актив)", wc_mirror: "дзеркальні портфелі", wc_assets: "активів", wc_sf: "спільні фандери" },
  en: { wc_h: "🐳 Whale clusters (cross-asset)", wc_mirror: "mirror portfolios", wc_assets: "assets", wc_sf: "shared funders" },
  de: { wc_h: "🐳 Wal-Cluster (asset-übergreifend)", wc_mirror: "Spiegel-Portfolios", wc_assets: "Assets", wc_sf: "geteilte Geldgeber" },
  fr: { wc_h: "🐳 Clusters de baleines (multi-actifs)", wc_mirror: "portefeuilles miroirs", wc_assets: "actifs", wc_sf: "financeurs communs" },
  ru: { wc_h: "🐳 Китовые кластеры (кросс-актив)", wc_mirror: "зеркальные портфели", wc_assets: "активов", wc_sf: "общие фандеры" },
};
for (const l of Object.keys(WCL_TR) as Lang[]) Object.assign(TR[l], WCL_TR[l]);

const GD_TR: Record<Lang, Record<string, string>> = {
  uk: { gd_card: "Відкрити Arkham-картку", gd_subgraph: "Граф цього гаманця", gd_hint: "Подвійний клік по вузлу — перейти в його граф", gd_asset: "Актив (токен)", gd_verified: "Верифіковано", gd_holdings: "Вміст гаманця" },
  en: { gd_card: "Open Arkham card", gd_subgraph: "This wallet's graph", gd_hint: "Double-click a node to open its graph", gd_asset: "Asset (token)", gd_verified: "Verified", gd_holdings: "Wallet contents" },
  de: { gd_card: "Arkham-Karte öffnen", gd_subgraph: "Graph dieser Wallet", gd_hint: "Doppelklick auf Knoten öffnet dessen Graph", gd_asset: "Asset (Token)", gd_verified: "Verifiziert", gd_holdings: "Wallet-Inhalt" },
  fr: { gd_card: "Ouvrir la carte Arkham", gd_subgraph: "Graphe de ce portefeuille", gd_hint: "Double-cliquez un nœud pour son graphe", gd_asset: "Actif (token)", gd_verified: "Vérifié", gd_holdings: "Contenu du portefeuille" },
  ru: { gd_card: "Открыть Arkham-карту", gd_subgraph: "Граф этого кошелька", gd_hint: "Двойной клик по узлу — его граф", gd_asset: "Актив (токен)", gd_verified: "Верифицировано", gd_holdings: "Содержимое кошелька" },
};
for (const l of Object.keys(GD_TR) as Lang[]) Object.assign(TR[l], GD_TR[l]);

/* Послужний список сигналів. Українська — повна; решта мов падає на неї через
   відкат у t(), і це чесніше, ніж машинний переклад методичних формулювань,
   де кожне слово («надлишок», «випадковий вхід») має точний зміст. */
const TREC_TR: Record<Lang, Record<string, string>> = {
  uk: {
    tr_btn: "Послужний список",
    tr_title: "Послужний список сигналів",
    tr_loading: "Рахую підсумок журналу…",
    tr_err: "Сервер не відповів — це не означає, що сигнали не перевірялись",
    tr_empty: "Журнал ще порожній: сигнали пишуться в нього з моменту появи, перші присуди будуть, коли мине перший горизонт.",
    tr_head: "Чи справдились наші власні твердження",
    tr_claims: "тверджень",
    tr_checked: "перевірено",
    tr_waiting: "чекають дозрівання",
    tr_since: "від",
    tr_excess_note: "Надлишок — дохідність активу мінус {bench} за те саме вікно. Судимо лише за ним: у бичачий тиждень зростає майже все.",
    tr_col_dir: "напрям",
    tr_col_n: "спостережень",
    tr_col_signal: "сигнал",
    tr_col_random: "випадковий вхід",
    tr_col_verdict: "присуд",
    tr_dir_buy: "купівля",
    tr_dir_sell: "продаж",
    tr_days: "днів",
    tr_assets: "активів",
    tr_src_live: "живий монітор",
    tr_src_retro: "ретро-реплей",
    tr_v_insufficient: "замало спостережень (треба {n})",
    tr_v_nonull: "нема з чим порівняти",
    tr_v_same: "те саме, що випадковий вхід ({lift} при межі ±{band})",
    tr_v_better: "краще за випадковий вхід на {lift}",
    tr_v_worse: "ГІРШЕ за випадковий вхід на {lift}",
    tr_dominated: "⚠ {sym} дає {share}% вибірки — це висновок радше про нього, ніж про сигнал",
    tr_lowcover: "⚠ нульова база покриває лише {pct}% вибірки",
    tr_howread: "Як це читати",
    tr_retro_note: "Ретро: вибірка — активи, які групи тримають сьогодні (виживання), частка емісії рахована з сьогоднішнього випуску (зазирання вперед).",
    tr_unresolved: "без ціни: {n}",
    tr_excluded: "виключено як базовий актив ({sym}): {n}",
    tr_badlines: "битих рядків у журналі: {n}",
    tr_freshness: "вік даних",
    tr_missing: "немає",
    tr_min: "хв",
    tr_hour: "год",
    tr_day: "дн",
    chk_title: "Перевірка за 30 секунд",
    chk_kind_token: "токен",
    chk_kind_wallet: "гаманець",
    chk_kind_unknown: "невідомо що",
    chk_unknown: "Ми про цю адресу нічого не знаємо. Це НЕ означає, що з нею все гаразд.",
    chk_sources: "джерела",
    chk_nosources: "жодне джерело не покриває цю адресу — оцінки немає, а не «чисто»",
    tracer_hop: "коліно",
    tracer_nodes: "адрес у сліді",
    tracer_edges: "переказів",
    tracer_depth: "глибина",
    tracer_cexout: "вийшло на біржі",
    tracer_cexin: "прийшло з бірж",
    tracer_none: "Слід не знайдено: з цієї адреси немає переказів, які можна простежити. Це не помилка — просто нема чого показати.",
    tracer_mainpath: "Показано найважчий за грошима шлях: на кожному коліні взято найбільший перехід. Це не єдиний маршрут коштів, а головний.",
    tracer_err: "Трасування не вдалося — сервер не відповів",
    holders_ours: "Наші відстежувані гаманці",
    holders_onchain: "Топ холдерів у ланцюгу",
    holders_cum: "накопичено",
    holders_nocontract: "Повного списку холдерів для цього активу немає: контракт невідомий. Показано лише те, що ми справді знаємо.",
    holders_chain_err: "Список холдерів не завантажився — показано лише наші гаманці",
    dash_no_holders: "Жоден відстежуваний гаманець не тримає цей актив",
    bulk_btn: "Перевірка списку",
    bulk_title: "Масова перевірка адрес",
    bulk_hint: "до 500 адрес · будь-який роздільник · рахується з локальних даних, без мережі",
    bulk_run: "Перевірити",
    bulk_running: "Перевіряю {n} адрес…",
    bulk_err: "Перевірка не вдалася — сервер не відповів",
    bulk_checked: "перевірено",
    bulk_unknown: "невідомих",
    bulk_flagged: "із застереженнями",
    bulk_unknown_note: "Невідомі адреси рахуються окремо й НЕ входять у розподіл оцінок: відсутність даних — це не «чисто».",
    bulk_truncated: "не влізло в межу {limit}: {n}",
    bulk_invalid: "не схожі на адресу: {n}",
    bulk_col_grade: "оцінка",
    bulk_col_addr: "адреса",
    bulk_col_kind: "тип",
    bulk_col_facts: "що знайдено",
    bulk_csv: "Вивантажити CSV",
    ep_btn: "Тиск на вихід",
    ep_title: "Тиск на вихід: позиція в днях обороту",
    ep_loading: "Рахую тиск…",
    ep_err: "Не вдалося завантажити — сервер не відповів",
    ep_empty: "Даних немає: сигнали по групах ще не рахувались.",
    ep_head: "Хто тримає стільки, що вийти без просідання не зможе",
    ep_ranked: "позицій",
    ep_trapped: "у пастці й роздають",
    ep_nomarket: "без ринку",
    ep_groups: "груп із сигналами: {n} із {all}",
    ep_trapped_h: "Роздають те, з чого не вийти швидко",
    ep_nomarket_h: "Ринку немає",
    ep_nomarket_note: "Добовий оборот нижчий за $1 тис. — «дні обороту» тут не вимірюються, тому позиції не ранжуються. Вийти з них нікуди взагалі.",
    ep_col_pressure: "тиск",
    ep_col_asset: "актив",
    ep_col_emission: "% емісії",
    ep_col_size: "позиція",
    ep_col_days: "на вихід",
    ep_col_phase: "фаза",
    ep_days: "дн обороту",
    ep_months: "міс обороту",
    ep_years: "р обороту",
    ep_dropped: "не опубліковано",
    ep_drop_unpriceable: "без ціни",
    ep_drop_insaneUsd: "неправдоподібна сума",
    ep_drop_noEmission: "без частки емісії",
    ep_drop_noVolume: "без обсягу",
    ep_drop_dust: "пил",
    fr_fresh: "дані свіжі",
    fr_stale: "застаріло {n} із {all}",
    fr_unknown: "стан даних невідомий",
    fr_err: "Стан даних не завантажився — це не означає, що все гаразд",
    fr_all_fresh: "Усі набори оновлюються вчасно.",
    fr_stale_note: "Застарілих наборів: {n}. Панелі, що на них живляться, показують старі числа з виглядом свіжих.",
    fr_note: "Вік рахується за часом зміни файлів, які пишуть воркери. Вимкнений воркер = файл не оновлюється, а ендпоінт усе одно відповідає.",
    fr_running: "працює",
    fr_never: "не оновлювалось",
    fr_min: "хв тому",
    fr_hour: "год тому",
    fr_day: "дн тому",
    mv_title: "Перевірка переказу",
    mv_hint: "Чи це рух ринку, чи біржа перекладає власні монети між своїми гаманцями. Найпоширеніша хиба китових стрічок.",
    mv_from: "звідки",
    mv_to: "куди",
    mv_run: "Перевірити",
    mv_checking: "перевіряю…",
    mv_need_both: "потрібні обидві адреси",
    mv_err: "не вдалося — сервер не відповів",
    mv_nolabel: "без мітки",
    "mv_v_internal-reshuffle": "🔁 Перекладання всередині біржі — НЕ подія ринку",
    "mv_v_between-exchanges": "↔ Переказ між різними біржами",
    "mv_v_deposit-to-exchange": "🏦 Занесення на біржу",
    "mv_v_withdrawal-from-exchange": "🏧 Виведення з біржі",
    "mv_v_on-chain": "⛓ Рух усередині ланцюга",
    mv_v_unlabeled: "❔ Не знаємо — принаймні одна адреса без мітки",
    fr_data_h: "Набори даних: проблемних {n} із {all}",
    fr_data_ok: "Усі {n} наборів у межах свого строку.",
    fr_empty: "порожній",
    fr_day_short: "дн",
  },
  en: {
    tr_btn: "Track record",
    tr_title: "Signal track record",
    tr_col_dir: "direction",
    tr_col_n: "observations",
    tr_col_signal: "signal",
    tr_col_random: "random entry",
    tr_col_verdict: "verdict",
    tr_dir_buy: "buy",
    tr_dir_sell: "sell",
    tr_days: "days",
    tr_assets: "assets",
  },
  de: {}, fr: {}, ru: {},
};
for (const l of Object.keys(TREC_TR) as Lang[]) Object.assign(TR[l], TREC_TR[l]);

export const BEH_META: Record<string, { icon: string; key: string; cls: string }> = {
  exchange: { icon: "🏦", key: "beh_exchange", cls: "text-yellow-400 bg-yellow-400/10" },
  mm: { icon: "🤖", key: "beh_mm", cls: "text-purple-400 bg-purple-400/10" },
  accumulator: { icon: "📈", key: "beh_accumulator", cls: "text-good bg-good/10" },
  distributor: { icon: "📉", key: "beh_distributor", cls: "text-bad bg-bad/10" },
  trader: { icon: "🔄", key: "beh_trader", cls: "text-sky-400 bg-sky-400/10" },
  low_data: { icon: "·", key: "beh_low_data", cls: "text-slate-400 bg-slate-500/10" },
};

const SUPPORTED_LANGS: Lang[] = ["uk", "en", "de", "fr", "ru"];

export let lang: Lang = ((): Lang => {
  const saved = localStorage.getItem("lang") as Lang | null;
  return saved && SUPPORTED_LANGS.includes(saved) ? saved : "uk";
})();

// Підстановка змінних: t("bt_note", { h: "30", b: "ETH" }) → "…за 30 днів … над ETH".
// Потрібна там, де в текст їде число з методики (горизонт, бенчмарк) — щоб підпис не розходився
// з тим, що сервер реально порахував.
export const t = (key: string, vars?: Record<string, string>): string => {
  let s = TR[lang][key] ?? TR.uk[key] ?? key;
  if (vars) for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(v);
  return s;
};

export function setLang(l: Lang): void {
  lang = l;
  localStorage.setItem("lang", l);
}
