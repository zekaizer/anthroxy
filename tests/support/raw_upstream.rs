//! A Chat Completions backend on a bare TCP socket. It keeps the head of
//! every request as the bytes that arrived: the order of the headers and the
//! case of their names, which an HTTP library parses away.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A credential the backend refuses: a request that carries it is answered
/// 401 and its head is not kept.
pub const REFUSED_KEY: &str = "refused-key";

/// The request line and the header lines of one request.
#[derive(Debug, Clone)]
pub struct Head(String);

impl Head {
    pub fn request_line(&self) -> &str {
        self.0.lines().next().unwrap_or_default()
    }

    /// The header names as written, in the order they arrived.
    pub fn names(&self) -> Vec<&str> {
        self.fields().map(|(name, _)| name).collect()
    }

    /// The value of the header written exactly as `name`.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.fields()
            .find(|(written, _)| *written == name)
            .map(|(_, value)| value)
    }

    fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(": "))
    }
}

/// Answers a chat request with a completion, as an event stream when the
/// request asks for one, and anything else with a model list, over HTTP/1.1.
pub struct RawUpstream {
    addr: SocketAddr,
    requests: Requests,
    _task: tokio::task::JoinHandle<()>,
}

/// The head and the body of every request, in the order they arrived.
type Requests = Arc<Mutex<Vec<(Head, String)>>>;

impl RawUpstream {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Requests::default();
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                tokio::spawn(serve(socket, recorded.clone()));
            }
        });
        Self {
            addr,
            requests,
            _task: task,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The head of the last request.
    pub fn last(&self) -> Head {
        self.last_request().0
    }

    /// The body of the last request, as the bytes that arrived.
    pub fn last_body(&self) -> String {
        self.last_request().1
    }

    fn last_request(&self) -> (Head, String) {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("the backend received at least one request")
    }
}

const STREAMED: &str = concat!(
    r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1,"model":"qwen","choices":[{"index":0,"delta":{"role":"assistant","content":"ok"},"finish_reason":null}]}"#,
    "\n\n",
    r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1,"model":"qwen","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
    "\n\n",
    r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","created":1,"model":"qwen","choices":[],"usage":{"prompt_tokens":5,"completion_tokens":7,"total_tokens":12}}"#,
    "\n\n",
    "data: [DONE]\n\n",
);

async fn serve(mut socket: TcpStream, requests: Requests) {
    let mut buffer = Vec::new();
    loop {
        let end = loop {
            if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
            let mut chunk = [0u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(read) if read > 0 => buffer.extend_from_slice(&chunk[..read]),
                _ => return,
            }
        };
        let head = Head(String::from_utf8_lossy(&buffer[..end - 4]).into_owned());
        let body_len: usize = head
            .fields()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, value)| value.parse().unwrap());
        while buffer.len() < end + body_len {
            let mut chunk = [0u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(read) if read > 0 => buffer.extend_from_slice(&chunk[..read]),
                _ => return,
            }
        }
        let body = String::from_utf8_lossy(&buffer[end..end + body_len]).into_owned();
        buffer.drain(..end + body_len);
        let refused = head.fields().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value.ends_with(REFUSED_KEY)
        });
        if refused {
            let refusal = "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n";
            if socket.write_all(refusal.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }

        let chat = head.request_line().contains("/v1/chat/completions");
        let (content_type, answer) = match (chat, body.contains(r#""stream":true"#)) {
            (true, true) => ("text/event-stream", STREAMED.to_owned()),
            (true, false) => (
                "application/json",
                json!({
                    "id": "chatcmpl-9", "object": "chat.completion", "created": 1, "model": "qwen",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "ok"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12}
                })
                .to_string(),
            ),
            (false, _) => (
                "application/json",
                json!({"object": "list", "data": [{"id": "live-1", "object": "model"}]})
                    .to_string(),
            ),
        };
        requests.lock().unwrap().push((head, body));
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\r\n{answer}",
            answer.len()
        );
        if socket.write_all(response.as_bytes()).await.is_err() {
            return;
        }
    }
}
