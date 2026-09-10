//! The `kind = "openai"` side of the proxy (ADR-0010): what goes upstream
//! and how the answer comes back as a Messages response.

use std::time::Instant;

use axum::body::Body;
use bytes::Bytes;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue};
use tracing::Span;

use crate::observability::Recorder;
use crate::server::relay::Relay;
use crate::server::{RequestId, RouterError};
use crate::translate;
use crate::upstream::{Backend, UpstreamError, UpstreamResponse};

/// The upstream path and body for a client request to `client_path`.
/// `count_tokens` has no counterpart and is refused here.
pub fn prepare(
    body: &[u8],
    client_path: &str,
    backend: &str,
) -> Result<(&'static str, Bytes), RouterError> {
    let path = client_path.split('?').next().unwrap_or(client_path);
    if path != "/v1/messages" {
        return Err(RouterError::NotOnOpenAi {
            backend: backend.to_owned(),
            path: path.to_owned(),
        });
    }
    let translated = translate::request(body).map_err(|source| RouterError::Translate {
        backend: backend.to_owned(),
        source,
    })?;
    Ok((translate::CHAT_COMPLETIONS_PATH, Bytes::from(translated)))
}

/// Headers the router sets on the client response, and the body: an error
/// document, a message document, or a translated event stream. The recorder
/// sees the backend's bytes in every case.
#[allow(clippy::too_many_arguments)]
pub async fn body(
    upstream: UpstreamResponse,
    backend: &Backend,
    upstream_model: &str,
    stream: bool,
    request_id: &RequestId,
    span: Span,
    started: Instant,
    recorder: Option<Recorder>,
) -> Result<(HeaderMap, Body), RouterError> {
    let status = upstream.response.status();
    let mut headers = HeaderMap::new();
    if status.is_client_error() || status.is_server_error() {
        let raw = read(upstream, backend).await?;
        tracing::warn!(
            status = status.as_u16(),
            bytes = raw.len(),
            "upstream returned an error"
        );
        if let Some(recorder) = recorder {
            recorder.finish_with_body(&raw);
        }
        let document = translate::upstream_error(status, &raw, &backend.name, request_id.as_str());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let json = serde_json::to_vec(&document).expect("an error document serializes");
        return Ok((headers, Body::from(json)));
    }
    let event_stream = upstream
        .response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if !stream || !event_stream {
        if stream {
            tracing::warn!("backend answered a streaming request with a document");
        }
        let raw = read(upstream, backend).await?;
        tracing::info!(
            bytes = raw.len(),
            duration_ms = started.elapsed().as_millis() as u64,
            "response body complete"
        );
        if let Some(recorder) = recorder {
            recorder.finish_with_body(&raw);
        }
        let document = translate::response(&raw, upstream_model).map_err(|error| {
            RouterError::BadUpstreamResponse {
                backend: backend.name.clone(),
                detail: error.to_string(),
            }
        })?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        return Ok((headers, Body::from(document)));
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    let relay = Relay::new(upstream.response.bytes_stream(), span, started, recorder);
    let translator = translate::Translator::new(relay, upstream_model, &backend.name);
    Ok((headers, Body::from_stream(translator)))
}

async fn read(upstream: UpstreamResponse, backend: &Backend) -> Result<Bytes, RouterError> {
    upstream
        .response
        .bytes()
        .await
        .map_err(|source| UpstreamError::Body {
            backend: backend.name.clone(),
            source,
        })
        .map_err(RouterError::from)
}
