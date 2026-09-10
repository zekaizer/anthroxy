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
use crate::activity::{Exchange, Source};
use crate::anthropic::{ErrorType, UsageScanner};
use crate::server::handlers::proxy;
use crate::server::{AppState, RequestId, Snapshot};
use crate::sse::Parser;
use crate::stats::{Generation, generation};

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
    let id = RequestId::generate();
    let exchange = app.activity.begin(
        id.as_str(),
        Source::Console,
        Some(viewer.to_string()),
        "POST",
        "/v1/messages",
    );
    let Answer {
        status,
        content_type,
        body: received,
        ttfb_ms,
        duration_ms,
        failure,
    } = answer(&snapshot, &id, request(&payload), exchange, DEADLINE).await;

    let events = content_type
        .as_deref()
        .is_some_and(|t| t.starts_with("text/event-stream"));
    let mut scanner = UsageScanner::for_content_type(content_type.as_deref());
    scanner.feed(&received);
    let scan = scanner.finish();
    let complete =
        status.is_some_and(|status| status < 400) && failure.is_none() && scan.error.is_none();
    let speed = generation(
        ask.stream,
        complete,
        ttfb_ms,
        Some(duration_ms),
        scan.usage.map(|usage| usage.output),
    )
    .map(Generation::tokens_per_second);
    let view = app.activity.find(id.as_str());
    live(&json!({
        "request_id": id.as_str(),
        "model": ask.model,
        "stream": ask.stream,
        "status": status,
        "ttfb_ms": ttfb_ms,
        "duration_ms": duration_ms,
        "output_tokens_per_second": speed,
        "text": answer_text(&received, events),
        "usage": scan.usage,
        "error": failure.or(scan.error),
        "backend": view.as_ref().and_then(|v| v.backend.clone()),
        "upstream_model": view.as_ref().and_then(|v| v.upstream_model.clone()),
        "hints": view.map(|v| v.hints).unwrap_or_default(),
    }))
}

fn request(payload: &Value) -> http::Request<Body> {
    http::Request::builder()
        .method(http::Method::POST)
        .uri("/v1/messages")
        .header(CONTENT_TYPE, "application/json")
        .header("anthropic-version", "2023-06-01")
        .body(Body::from(payload.to_string()))
        .expect("a static request builds")
}

/// What came back for a test request.
struct Answer {
    /// `None` when no response headers arrived.
    status: Option<u16>,
    content_type: Option<String>,
    body: Vec<u8>,
    ttfb_ms: Option<u64>,
    duration_ms: u64,
    failure: Option<String>,
}

/// Routes `request` through the proxy and reads the whole answer, within
/// `deadline`.
async fn answer(
    snapshot: &Snapshot,
    id: &RequestId,
    request: http::Request<Body>,
    exchange: Exchange,
    deadline: Duration,
) -> Answer {
    let started = Instant::now();
    let mut status = None;
    let mut content_type = None;
    let mut received = Vec::new();
    let mut ttfb_ms = None;
    let mut failure = None;
    let whole = async {
        let response = proxy::serve(snapshot, id, request, exchange).await;
        status = Some(response.status().as_u16());
        content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut body = response.into_body();
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
    if tokio::time::timeout(deadline, whole).await.is_err() {
        failure = Some(format!(
            "no complete answer within {}",
            humantime::format_duration(deadline)
        ));
    }
    Answer {
        status,
        content_type,
        body: received,
        ttfb_ms,
        duration_ms: started.elapsed().as_millis() as u64,
        failure,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::config::Config;

    #[tokio::test]
    async fn the_deadline_covers_a_backend_that_never_answers() {
        let stuck = axum::Router::new().fallback(std::future::pending::<()>);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, stuck).await.unwrap() });
        let config = Config::parse(
            &format!(
                "[server]\ntoken = \"t\"\n[backends.stuck]\nurl = \"http://{addr}\"\n[[models]]\nid = \"m\"\nbackend = \"stuck\"\n[stats]\nenabled = false\n"
            ),
            |_| None,
        )
        .unwrap();
        let snapshot = Snapshot::from_config(&config).unwrap();
        let activity = Activity::new();
        let id = RequestId::generate();
        let exchange = activity.begin(id.as_str(), Source::Console, None, "POST", "/v1/messages");
        let payload = json!({"model": "m", "max_tokens": 8, "messages": []});

        let started = Instant::now();
        let answer = tokio::time::timeout(
            Duration::from_secs(10),
            answer(
                &snapshot,
                &id,
                request(&payload),
                exchange,
                Duration::from_millis(300),
            ),
        )
        .await
        .expect("the deadline bounds waiting for response headers too");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(answer.status, None);
        assert!(
            answer
                .failure
                .as_deref()
                .is_some_and(|f| f.contains("within 300ms")),
            "{:?}",
            answer.failure
        );
    }
}
