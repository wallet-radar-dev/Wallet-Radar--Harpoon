//! Тести синхронізації реєстру підключених із сервером Wallet Radar.
//!
//! Ключова гарантія: **порожній реєстр ніколи не пишеться на сервер**.
//! Сервер замінює файл реєстру цілком, тому тестовий інстанс бота з
//! порожньою БД стер би список реальних підписників, а серверні алерти
//! впали б у порожнечу.

use bot::registry::RegistrySync;
use storage::{Language, NewUser, Role, SqliteStore, Store};

/// Той самий файл, який бачить сервер: токен, chat_id, юзернейм.
fn snapshot_of(server: &str) -> String {
    std::fs::read_to_string(server).unwrap_or_default()
}

#[tokio::test]
async fn порожній_реєстр_не_стирає_підписників_на_сервері() {
    let dir = std::env::temp_dir().join(format!("wra-reg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let server_file = dir.join("telegram-users.json");
    let existing = r#"{"users":{"527223651":{"chat":"527223651","username":"mihunt3r"}}}"#;
    std::fs::write(&server_file, existing).unwrap();

    // Локальний «сервер»: приймає POST і перезаписує файл, як це робить Wallet Radar.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_path = server_file.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        // Тіло POST-запиту — після \r\n\r\n.
        let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        std::fs::write(&server_path, body).unwrap();
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        stream.write_all(resp.as_bytes()).await.unwrap();
    });

    let store = SqliteStore::open_in_memory().unwrap();
    // Реєстр порожній — жодного користувача не додавали.
    let sync = RegistrySync::new(store, &format!("http://{addr}"), "test-token");

    let result = sync.sync_once().await;
    assert!(
        result.is_err(),
        "порожній реєстр не має синхронізуватися: {:?}",
        result.err()
    );
    let msg = result.err().unwrap().to_string();
    assert!(
        msg.contains("порожній"),
        "очікували захист від порожнього реєстру, отримали: {msg}"
    );

    // Сервер не отримав жодного POST — файл не змінився.
    // Очікування з таймаутом: запиту не буде взагалі, тож `accept()` не завершиться.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server).await;
    assert_eq!(
        snapshot_of(server_file.to_str().unwrap()),
        existing,
        "файл реєстру на сервері не мав змінитися"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn непустий_реєстр_синхронізується_на_сервер() {
    let dir = std::env::temp_dir().join(format!("wra-reg2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let server_file = dir.join("telegram-users.json");
    std::fs::write(&server_file, "{}").unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_path = server_file.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        std::fs::write(&server_path, body).unwrap();
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        stream.write_all(resp.as_bytes()).await.unwrap();
    });

    let store = SqliteStore::open_in_memory().unwrap();
    store
        .upsert_user(NewUser {
            chat_id: 42,
            user_id: Some(42),
            username: Some("tester".into()),
            language: Language::Uk,
            timezone: "UTC".into(),
            role: Role::Viewer,
        })
        .await
        .unwrap();

    let sync = RegistrySync::new(store, &format!("http://{addr}"), "test-token");
    let count = sync.sync_once().await.unwrap();
    assert_eq!(count, 1, "у реєстрі один користувач");

    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
    let written = snapshot_of(server_file.to_str().unwrap());
    assert!(
        written.contains("\"42\""),
        "сервер мав отримати chat_id користувача, отримали: {written}"
    );
    assert!(
        written.contains("tester"),
        "сервер мав отримати юзернейм, отримали: {written}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Мік сервера Wallet Radar: приймає один POST і перезаписує файл реєстру.
/// Повертає адресу сервера та задачу, що обслуговує запит.
async fn mock_server(server_file: std::path::PathBuf) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        std::fs::write(&server_file, body).unwrap();
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        stream.write_all(resp.as_bytes()).await.unwrap();
    });
    (format!("http://{addr}"), handle)
}

/// Реєстр має піти в **кожен** сервер: у мережі кілька незалежних екземплярів
/// Wallet Radar, і в кожному реєстр — це список, за яким сервер розсилає свої
/// власні алерти. Якщо один із них не отримає список, підписки розходять.
#[tokio::test]
async fn реєстр_пишеться_в_кожен_додатковий_сервер() {
    let dir = std::env::temp_dir().join(format!("wra-reg-multi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let primary_file = dir.join("primary.json");
    let extra_file = dir.join("extra.json");
    std::fs::write(&primary_file, "{}").unwrap();
    std::fs::write(&extra_file, "{}").unwrap();

    let (primary_url, primary_handle) = mock_server(primary_file.clone()).await;
    let (extra_url, extra_handle) = mock_server(extra_file.clone()).await;

    let store = SqliteStore::open_in_memory().unwrap();
    store
        .upsert_user(NewUser {
            chat_id: 77,
            user_id: Some(77),
            username: Some("subscriber".into()),
            language: Language::Uk,
            timezone: "UTC".into(),
            role: Role::Viewer,
        })
        .await
        .unwrap();

    // Основний сервер.
    let sync = RegistrySync::new(store.clone(), &primary_url, "test-token");
    assert_eq!(sync.sync_once().await.unwrap(), 1);

    // Додатковий сервер — тим самим токеном, тим самим списком.
    let sync_extra = RegistrySync::new(store, &extra_url, "test-token");
    assert_eq!(sync_extra.sync_once().await.unwrap(), 1);

    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), primary_handle).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), extra_handle).await;

    for (name, file) in [("основний", &primary_file), ("додатковий", &extra_file)]
    {
        let written = snapshot_of(file.to_str().unwrap());
        assert!(
            written.contains("\"77\""),
            "{name} сервер мав отримати chat_id, отримали: {written}"
        );
        assert!(
            written.contains("subscriber"),
            "{name} сервер мав отримати юзернейм, отримали: {written}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}
