//! Fixtures for a backend that speaks OpenAI Chat Completions.

use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::router::TOKEN;

/// Configuration with one `kind = "openai"` backend and two models.
pub fn config_with_openai_backend(backend_url: &str, extra: &str) -> String {
    format!(
        r#"
[server]
listen = "127.0.0.1:0"
token = "{TOKEN}"

[backends.mock]
kind = "openai"
url = "{backend_url}"
credential = {{ kind = "static", value = "backend-secret-key" }}

[[models]]
id = "qwen"
backend = "mock"
upstream_model = "qwen-32b"
display_name = "Qwen"
aliases = ["claude-haiku-4-5"]

[[models]]
id = "other"
backend = "mock"
{extra}
"#
    )
}

/// One `data:` frame carrying `payload`.
pub fn frame(payload: &Value) -> String {
    format!("data: {payload}\n\n")
}

/// A chunk with `delta` for choice 0 and an optional finish reason.
pub fn chunk(delta: Value, finish: Option<&str>) -> String {
    frame(&json!({
        "id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1, "model": "qwen-32b",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
    }))
}

/// The usage-only chunk a server sends when `stream_options.include_usage`
/// is set, followed by `[DONE]`.
pub fn usage_and_done(input: u64, output: u64) -> Vec<String> {
    vec![
        frame(&json!({
            "id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "qwen-32b", "choices": [],
            "usage": {"prompt_tokens": input, "completion_tokens": output, "total_tokens": input + output}
        })),
        "data: [DONE]\n\n".to_owned(),
    ]
}

/// A `text/event-stream` response sending `frames` with `gap` between them.
pub fn sse_response(frames: Vec<String>, gap: Duration) -> Response {
    let stream = futures_util::stream::iter(frames).then(move |f| async move {
        tokio::time::sleep(gap).await;
        Ok::<_, std::io::Error>(f)
    });
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// A completed `chat.completion` document.
pub fn completion(message: Value, finish: &str) -> Response {
    super::mock_upstream::json_response(
        200,
        json!({
            "id": "chatcmpl-9", "object": "chat.completion", "created": 1, "model": "qwen-32b",
            "choices": [{"index": 0, "message": message, "finish_reason": finish}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12}
        }),
    )
}

/// `(event name, data)` of every frame in an Anthropic SSE body.
pub fn events(sse: &str) -> Vec<(String, Value)> {
    sse.split("\n\n")
        .filter(|f| !f.trim().is_empty())
        .map(|f| {
            let mut lines = f.lines();
            let event = lines
                .next()
                .unwrap()
                .strip_prefix("event: ")
                .unwrap()
                .to_owned();
            let data = lines.next().unwrap().strip_prefix("data: ").unwrap();
            (event, serde_json::from_str(data).unwrap())
        })
        .collect()
}
