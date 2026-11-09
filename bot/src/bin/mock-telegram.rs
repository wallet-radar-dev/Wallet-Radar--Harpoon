//! Мок Telegram Bot API — для локальних прогонів без справжнього токена.
//!
//! Реалізує рівно те, що потрібно боту:
//! * `GET /bot<token>/getUpdates` — повертає порожній список (або підкидає
//!   готові оновлення з файлу, якщо його задано);
//! * `POST /bot<token>/sendMessage` — приймає повідомлення, друкує їх у stdout
//!   і додає у файл (щоб потім перевірити, що в тексті немає адрес).
//!
//! Це інструмент розробки, а не частина продукту.

use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:8099".to_string());
    let out_file = args.next();
    let listener = TcpListener::bind(&addr).await?;
    println!("mock-telegram слухає http://{addr}");

    let out = out_file.map(|p| {
        Arc::new(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok(),
        ))
    });

    loop {
        let (stream, peer) = listener.accept().await?;
        let out = out.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, out).await {
                eprintln!("помилка обробки з'єднання від {peer}: {e}");
            }
        });
    }
}

async fn handle(
    mut stream: TcpStream,
    out: Option<Arc<Mutex<Option<std::fs::File>>>>,
) -> std::io::Result<()> {
    // Читаємо запит цілком: до кінця заголовків і потім тіло.
    let mut raw: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = vec![0u8; 4096];
    let head_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break raw.len();
        }
        raw.extend_from_slice(&chunk[..read]);
        if let Some(pos) = find_head_end(&raw) {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let request_line = head.split("\r\n").next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    for line in head.split("\r\n").skip(1) {
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = String::from_utf8_lossy(&raw[head_end.min(raw.len())..]).to_string();
    while body.len() < content_length {
        let mut more = vec![0u8; content_length - body.len()];
        let read = stream.read(&mut more).await?;
        if read == 0 {
            break;
        }
        body.push_str(&String::from_utf8_lossy(&more[..read]));
    }
    println!("→ запит: {request_line}");

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let lower_path = path.to_lowercase();
    let response = if lower_path.contains("getupdates") {
        "{\"ok\":true,\"result\":[]}"
    } else if lower_path.contains("sendmessage") {
        // Витягуємо текст повідомлення: значення рядкового поля "text".
        if let Some(start) = body.find("\"text\":") {
            let rest = body[start + 7..].trim_start();
            if let Some(stripped) = rest.strip_prefix('"') {
                let mut text = String::new();
                let mut escaped = false;
                for ch in stripped.chars() {
                    if escaped {
                        // Екранування в JSON: \n → перенос рядка, \" → лапка.
                        match ch {
                            'n' => text.push('\n'),
                            't' => text.push('\t'),
                            '"' => text.push('"'),
                            '\\' => text.push('\\'),
                            other => text.push(other),
                        }
                        escaped = false;
                        continue;
                    }
                    match ch {
                        '\\' => escaped = true,
                        '"' => break,
                        other => text.push(other),
                    }
                }
                println!("→ повідомлення: {text}");
                if let Some(file) = out.as_ref() {
                    let mut guard = file.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(f) = guard.as_mut() {
                        let _ = writeln!(f, "{text}");
                        let _ = f.flush();
                    }
                }
            }
        }
        "{\"ok\":true,\"result\":{\"message_id\":1,\"date\":0,\"chat\":{\"id\":0,\"type\":\"private\"}}}"
    } else {
        "{\"ok\":true,\"result\":true}"
    };

    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response.len(),
        response
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Позиція кінця заголовків (`\r\n\r\n`) у буфері.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
