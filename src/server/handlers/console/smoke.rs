//! `POST /api/smoke`: one short `/v1/messages` request through the full route
//! (rename, dropped fields, translation, credential), answered with what came
//! back.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Extension;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use http::StatusCode;
use http::header::CONTENT_TYPE;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{error, live};
use crate::activity::Source;
use crate::anthropic::{ErrorType, UsageScanner};
use crate::server::handlers::proxy;
use crate::server::{AppState, RequestId, Snapshot};
use crate::sse::Parser;

/// The whole answer must arrive within this.
const DEADLINE: Duration = Duration::from_secs(120);
const DEFAULT_PROMPT: &str = "Reply with one short sentence.";

#[derive(Deserialize)]
struct Ask {
    model: String,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    prompt: Option<String>,
}

pub async fn smoke(
    State(app): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
    ConnectInfo(viewer): ConnectInfo<SocketAddr>,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    let Ok(ask) = serde_json::from_slice::<Ask>(&body) else {
        return error(
            StatusCode::BAD_REQUEST,
            ErrorType::InvalidRequestError,
            r#"expected {"model": "<id>", "stream": true|false, "prompt": "<optional>"}"#,
            &request_id,
        );
    };
    let payload = json!({
        "model": ask.model,
        "max_tokens": 128,
        "stream": ask.stream,
        "messages": [{"role": "user", "content": ask.prompt.as_deref().unwrap_or(DEFAULT_PROMPT)}],
    });
    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header("anthropic-version", "2023-06-01")
        .body(Body::from(payload.to_string()))
        .expect("a static request builds");
    let id = RequestId::generate();
    let exchange = app.activity.begin(
        id.as_str(),
        Source::Console,
        Some(viewer.to_string()),
        "POST",
        "/v1/messages",
    );
    let started = Instant::now();
    let response = proxy::serve(&snapshot, &id, request, exchange).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut body = response.into_body();
    let mut received = Vec::new();
    let mut ttfb_ms = None;
    let mut failure = None;
    let read = async {
        while let Some(frame) = body.frame().await {
            match frame {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref() {
                        ttfb_ms.get_or_insert(started.elapsed().as_millis() as u64);
                        received.extend_from_slice(data);
                    }
                }
                Err(problem) => {
                    failure = Some(problem.to_string());
                    break;
                }
            }
        }
    };
    if tokio::time::timeout(DEADLINE, read).await.is_err() {
        failure = Some(format!(
            "no complete answer within {}",
            humantime::format_duration(DEADLINE)
        ));
    }
    drop(body);
    let duration_ms = started.elapsed().as_millis() as u64;

    let events = content_type
        .as_deref()
        .is_some_and(|t| t.starts_with("text/event-stream"));
    let mut scanner = UsageScanner::for_content_type(content_type.as_deref());
    scanner.feed(&received);
    let scan = scanner.finish();
    let view = app.activity.find(id.as_str());
    live(&json!({
        "request_id": id.as_str(),
        "model": ask.model,
        "stream": ask.stream,
        "status": status,
        "ttfb_ms": ttfb_ms,
        "duration_ms": duration_ms,
        "text": answer_text(&received, events),
        "usage": scan.usage,
        "error": failure.or(scan.error),
        "backend": view.as_ref().and_then(|v| v.backend.clone()),
        "upstream_model": view.as_ref().and_then(|v| v.upstream_model.clone()),
        "hints": view.map(|v| v.hints).unwrap_or_default(),
    }))
}

/// The text blocks of a Messages event stream or document.
fn answer_text(body: &[u8], events: bool) -> String {
    let mut text = String::new();
    if events {
        let mut parser = Parser::new();
        let mut frames = parser.feed(body).unwrap_or_default();
        frames.extend(parser.finish().unwrap_or_default());
        for frame in frames {
            let Ok(event) = serde_json::from_str::<Value>(&frame.data) else {
                continue;
            };
            if event.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                && let Some(delta) = event.pointer("/delta/text").and_then(Value::as_str)
            {
                text.push_str(delta);
            }
        }
    } else if let Ok(document) = serde_json::from_slice::<Value>(body) {
        for block in document["content"].as_array().into_iter().flatten() {
            if block["type"] == "text"
                && let Some(part) = block["text"].as_str()
            {
                text.push_str(part);
            }
        }
    }
    text
}
