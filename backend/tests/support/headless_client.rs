#![allow(dead_code)]

use std::collections::BTreeSet;
use std::process::Child;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct HttpResponse {
    pub status: u16,
    pub body: Value,
}

pub struct HeadlessClient {
    authority: String,
    bearer: String,
    conversation_id: String,
}

impl HeadlessClient {
    pub fn from_parts(authority: String, bearer: String, conversation_id: String) -> Self {
        Self {
            authority,
            bearer,
            conversation_id,
        }
    }

    pub async fn submit(&self, text: &str, client_message_id: &str) -> HttpResponse {
        let body = serde_json::to_vec(&json!({
            "protocol_version": 1,
            "client_message_id": client_message_id,
            "content": [{"type": "text", "text": text}],
        }))
        .unwrap();
        self.http(
            "POST",
            &format!("/v1/conversations/{}/messages", self.conversation_id),
            Some(&body),
            Some(client_message_id),
        )
        .await
    }

    pub async fn bootstrap(&self) -> HttpResponse {
        self.http("GET", "/v1/bootstrap", None, None).await
    }

    async fn http(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        idempotency_key: Option<&str>,
    ) -> HttpResponse {
        let mut stream = TcpStream::connect(&self.authority)
            .await
            .expect("connect headless HTTP client");
        let body = body.unwrap_or_default();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n",
            self.authority, self.bearer
        );
        if let Some(value) = idempotency_key {
            request.push_str(&format!("Idempotency-Key: {value}\r\n"));
        }
        if !body.is_empty() {
            request.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        parse_http_response(&bytes)
    }

    pub async fn websocket(&self, after: u64) -> Socket {
        let url = format!("ws://{}/v1/events?after={after}", self.authority);
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.bearer).parse().unwrap(),
        );
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }
}

fn parse_http_response(bytes: &[u8]) -> HttpResponse {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP header terminator");
    let headers = std::str::from_utf8(&bytes[..split]).unwrap();
    let status = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    HttpResponse {
        status,
        body: serde_json::from_slice(&bytes[split + 4..]).expect("JSON headless HTTP response"),
    }
}

pub fn signal_owned_process_group(child: &Child, signal: i32) -> std::io::Result<()> {
    let process_group = i32::try_from(child.id()).unwrap();
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: callers place the child's PID in a dedicated process group before spawning. A
    // negative target therefore reaches only the owned runtime and its descendants.
    let result = unsafe { kill(-process_group, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub async fn next_json_with_timeout(socket: &mut Socket, timeout: Duration) -> Value {
    loop {
        let message = tokio::time::timeout(timeout, socket.next())
            .await
            .expect("headless WebSocket frame timeout")
            .expect("headless WebSocket closed")
            .expect("headless WebSocket frame");
        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                return serde_json::from_str(&text).unwrap();
            }
            tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Pong(bytes))
                    .await
                    .unwrap();
            }
            tokio_tungstenite::tungstenite::Message::Pong(_) => {}
            other => panic!("unexpected headless WebSocket frame: {other:?}"),
        }
    }
}

pub async fn through_sync(socket: &mut Socket) -> Vec<Value> {
    let mut frames = Vec::new();
    loop {
        let frame = next_json_with_timeout(socket, Duration::from_secs(20)).await;
        let complete = frame["event_type"] == "sync.complete";
        frames.push(frame);
        if complete {
            return frames;
        }
    }
}

pub fn assert_durable_cursor_contract(frames: &[Value]) {
    let durable: Vec<&Value> = frames
        .iter()
        .filter(|frame| frame["delivery_kind"] == "durable")
        .collect();
    let cursors: Vec<u64> = durable
        .iter()
        .map(|frame| frame["cursor"].as_u64().unwrap())
        .collect();
    assert!(cursors.windows(2).all(|pair| pair[0] < pair[1]));
    let event_ids: BTreeSet<&str> = durable
        .iter()
        .map(|frame| frame["event_id"].as_str().unwrap())
        .collect();
    assert_eq!(event_ids.len(), durable.len());
}
