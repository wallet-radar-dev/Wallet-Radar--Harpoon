//! WebSocket-стрім подій Wallet Radar.
//!
//! Сервер надсилає:
//! * `{"type":"init","events":[…]}` — останні події після (пере)підключення;
//! * `{"type":"event","event":{…}}` — одна нова подія;
//! * інші JSON-об'єкти — поведінкові сповіщення й події груп. Їхній склад не
//!   гарантований, тож ми їх **не розбираємо** (іначе невідомі поля могли б
//!   проникнути в наші типи) — лише рахуємо.

use crate::parse::parse_server_msg;
use crate::types::ServerMsg;
use futures_util::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::warn;

use crate::client::WrError;

/// Живе з'єднання зі стрічкою.
pub struct WsStream {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// Скільки «чужих» кадрів (не init і не event) бачили — для метрик.
    pub other_frames: u64,
}

impl WsStream {
    pub async fn connect(
        base_url: &str,
        ws_path: &str,
        auth: Option<&str>,
    ) -> Result<WsStream, WrError> {
        let http_base = base_url
            .trim_end_matches('/')
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let scheme = if base_url.starts_with("https://") {
            "wss://"
        } else {
            "ws://"
        };
        let url = format!("{scheme}{http_base}{ws_path}");

        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| WrError::Connect(format!("некоректний WS-URL {url}: {e}")))?;
        if let Some(value) = auth {
            if let Ok(v) = value.parse() {
                request.headers_mut().insert("Authorization", v);
            }
        }

        let (ws, _response): (WebSocketStream<MaybeTlsStream<TcpStream>>, Response) =
            tokio_tungstenite::connect_async(request)
                .await
                .map_err(|e| WrError::Connect(format!("WS {url}: {e}")))?;

        Ok(WsStream {
            ws,
            other_frames: 0,
        })
    }

    /// Наступний кадр. `None` — з'єднання закрите.
    pub async fn next_message(&mut self) -> Option<Result<ServerMsg, WrError>> {
        loop {
            let msg = self.ws.next().await?;
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(v) => {
                            let parsed = parse_server_msg(&v);
                            if matches!(parsed, ServerMsg::Other) {
                                self.other_frames += 1;
                            }
                            return Some(Ok(parsed));
                        }
                        Err(e) => {
                            warn!(error = %e, "не-JSON кадр від Wallet Radar — ігнорую");
                            continue;
                        }
                    }
                }
                Ok(tokio_tungstenite::tungstenite::Message::Ping(_)) => {
                    // PING обробляє сам tungstenite; продовжуємо чекати.
                    continue;
                }
                Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => return None,
                Ok(_) => continue,
                Err(e) => return Some(Err(WrError::Connect(format!("WS-помилка: {e}")))),
            }
        }
    }
}
