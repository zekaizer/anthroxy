//! The `kind = "openai"` side of the proxy (ADR-0010): what goes upstream
//! and how the answer comes back as a Messages response.

use std::time::Instant;

use axum::body::Body;
use bytes::Bytes;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue};
use tracing::Span;

use crate::activity::Exchange;
use crate::observability::Recorder;
use crate::openai::ResponseError;
use crate::server::RouterError;
use crate::server::buffered::read_all;
use crate::server::ping::{PING_INTERVAL, Pings};
use crate::server::relay::Relay;
use crate::translate;
use crate::upstream::{UpstreamResponse, is_event_stream};

/// The upstream path and body for a client request to `client_path`.
/// `count_tokens` has no counterpart and is refused here.
pub fn prepare(
    body: &[u8],
    client_path: &str,
    backend: &str,
    upstream_model: &str,
) -> Result<(&'static str, Bytes), RouterError> {
    let path = client_path.split('?').next().unwrap_or(client_path);
    if path != "/v1/messages" {
        return Err(RouterError::NotOnOpenAi {
            backend: backend.to_owned(),
            path: path.to_owned(),
        });
    }
    let translated =
        translate::request(body, upstream_model).map_err(|source| RouterError::Translate {
            backend: backend.to_owned(),
            source,
        })?;
    Ok((translate::CHAT_COMPLETIONS_PATH, Bytes::from(translated)))
}

/// Headers the router sets on the client response and the body, for a
/// successful upstream response: a translated event stream, or a message
/// document from a body the backend answered whole. The recorder sees the
/// backend's bytes in every case; the exchange sees what the client gets and
/// is taken only once a body exists.
#[allow(clippy::too_many_arguments)]
pub async fn body(
    upstream: UpstreamResponse,
    backend: &str,
    upstream_model: &str,
    stream: bool,
    span: Span,
    started: Instant,
    recorder: Option<Recorder>,
    exchange: &mut Option<Exchange>,
) -> Result<(HeaderMap, Body), RouterError> {
    let mut headers = HeaderMap::new();
    let status = upstream.response.status().as_u16();
    if stream && is_event_stream(upstream.response.headers()) {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        let relay = Relay::new(upstream.response.bytes_stream(), span, started, recorder);
        let translator = translate::Translator::new(relay, upstream_model, backend);
        let body = match exchange.take() {
            Some(exchange) => Body::from_stream(Pings::new(
                exchange.track(translator, Some("text/event-stream")),
                PING_INTERVAL,
            )),
            None => Body::from_stream(Pings::new(translator, PING_INTERVAL)),
        };
        return Ok((headers, body));
    }
    let raw = read_all(upstream, backend, recorder).await?;
    tracing::info!(
        bytes = raw.len(),
        duration_ms = started.elapsed().as_millis() as u64,
        "response body complete"
    );
    let bad = |error: ResponseError| RouterError::BadUpstreamResponse {
        backend: backend.to_owned(),
        detail: error.to_string(),
    };
    let (content_type, body) = if stream {
        // The client reads events and nothing else, so a document is
        // replayed as the stream it stands for.
        tracing::warn!("backend answered a streaming request with a document");
        let events = translate::document_events(&raw, upstream_model).map_err(bad)?;
        ("text/event-stream", Bytes::from(events))
    } else {
        let document = translate::response(&raw, upstream_model).map_err(bad)?;
        ("application/json", Bytes::from(document))
    };
    if let Some(exchange) = exchange.take() {
        exchange.finish_body(status, &body, Some(content_type));
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    Ok((headers, Body::from(body)))
}
