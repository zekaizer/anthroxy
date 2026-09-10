//! `POST /v1/messages` (and siblings): route by `model`, forward, relay.

use std::sync::Arc;
use std::time::Instant;

use axum::Extension;
use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use bytes::Bytes;
use http_body_util::LengthLimitError;

use crate::anthropic;
use crate::config::BackendKind;
use crate::observability::body_log::headers_for_record;
use crate::observability::{Recorder, RequestRecord};
use crate::server::annotate::annotate_upstream_error;
use crate::server::handlers::openai;
use crate::server::relay::Relay;
use crate::server::{RequestId, RouterError, Snapshot};
use crate::upstream::{
    Backend, UpstreamError, UpstreamRequest, UpstreamResponse, X_ROUTER_BACKEND, X_ROUTER_MODEL,
    X_ROUTER_UPSTREAM_MODEL, header_value, response_headers, upstream_headers,
};

pub async fn proxy(
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    request: Request,
) -> Response {
    match handle(&snapshot, &request_id, request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, status = error.status().as_u16(), "request failed in router");
            error.into_response(&request_id)
        }
    }
}

async fn handle(
    state: &Snapshot,
    request_id: &RequestId,
    request: Request,
) -> Result<Response, RouterError> {
    let started = Instant::now();
    let (parts, body) = request.into_parts();
    let body = read_body(body, state.max_body_bytes).await?;

    let peek = anthropic::peek(&body)?;
    let requested_model = peek.model.expect("peek guarantees a model");
    let resolution = state
        .registry
        .resolve(&requested_model)
        .ok_or_else(|| RouterError::unknown_model(&requested_model, &state.registry))?;
    let route = resolution.route;
    let backend = &route.backend;
    let span = tracing::Span::current();
    span.record("model", route.id.as_str());
    span.record("backend", backend.name.as_str());
    tracing::info!(
        requested_model = %requested_model,
        matched = ?resolution.matched,
        upstream_model = %route.upstream_model,
        stream = peek.stream,
        body_bytes = body.len(),
        "routed"
    );
    // The translated body names the upstream model itself; only a relayed
    // body needs the rename here.
    let anthropic_kind = backend.kind == BackendKind::Anthropic;
    let rename = (anthropic_kind && requested_model != route.upstream_model)
        .then_some(route.upstream_model.as_str());
    let body = match anthropic::rewrite(&body, rename, &backend.drop_fields, anthropic_kind) {
        Some(rewritten) => Bytes::from(rewritten),
        None => body,
    };

    let client_path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let (path_and_query, body) = match backend.kind {
        BackendKind::Anthropic => (client_path, body),
        BackendKind::OpenAi => {
            openai::prepare(&body, client_path, &backend.name, &route.upstream_model)?
        }
    };
    let headers = upstream_headers(&parts.headers, backend);
    let recorder = state.body_log.as_ref().map(|log| {
        let record = request_record(
            request_id,
            &parts.method,
            path_and_query,
            &headers,
            &requested_model,
            route,
            peek.stream,
        );
        log.begin(record, &body, started)
    });
    let upstream = state
        .upstream
        .send(UpstreamRequest {
            backend,
            method: parts.method.clone(),
            path_and_query,
            headers,
            body,
        })
        .await?;
    let status = upstream.response.status();
    tracing::info!(
        status = status.as_u16(),
        attempts = upstream.attempts,
        latency_ms = upstream.latency.as_millis() as u64,
        "upstream responded"
    );

    let recorder = recorder.map(|mut recorder| {
        recorder.response_started(
            status,
            upstream.response.headers(),
            upstream.attempts,
            upstream.latency.as_millis() as u64,
        );
        recorder
    });

    let mut headers = response_headers(upstream.response.headers());
    headers.insert(X_ROUTER_BACKEND.clone(), header_value(&backend.name));
    headers.insert(X_ROUTER_MODEL.clone(), header_value(&route.id));
    headers.insert(
        X_ROUTER_UPSTREAM_MODEL.clone(),
        header_value(&route.upstream_model),
    );

    let body = match backend.kind {
        BackendKind::Anthropic => {
            relay_body(upstream, backend, request_id, span, started, recorder).await?
        }
        BackendKind::OpenAi => {
            let (overrides, body) = openai::body(
                upstream,
                backend,
                &route.upstream_model,
                peek.stream,
                request_id,
                span,
                started,
                recorder,
            )
            .await?;
            headers.extend(overrides);
            body
        }
    };

    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

/// ADR-0003: error bodies are buffered and annotated, everything else is
/// relayed as it arrives.
async fn relay_body(
    upstream: UpstreamResponse,
    backend: &Backend,
    request_id: &RequestId,
    span: tracing::Span,
    started: Instant,
    recorder: Option<Recorder>,
) -> Result<Body, RouterError> {
    let status = upstream.response.status();
    if status.is_client_error() || status.is_server_error() {
        let raw = upstream
            .response
            .bytes()
            .await
            .map_err(|source| UpstreamError::Body {
                backend: backend.name.clone(),
                source,
            })?;
        tracing::warn!(
            status = status.as_u16(),
            bytes = raw.len(),
            "upstream returned an error"
        );
        if let Some(recorder) = recorder {
            recorder.finish_with_body(&raw);
        }
        return Ok(
            match annotate_upstream_error(&raw, &backend.name, status, request_id.as_str()) {
                Some(annotated) => Body::from(annotated),
                None => Body::from(raw),
            },
        );
    }
    Ok(Body::from_stream(Relay::new(
        upstream.response.bytes_stream(),
        span,
        started,
        recorder,
    )))
}

/// Snapshot for the body log. `headers` are the ones going upstream; the
/// credential is added at send time and never written.
fn request_record(
    request_id: &RequestId,
    method: &http::Method,
    path_and_query: &str,
    headers: &http::HeaderMap,
    requested_model: &str,
    route: &crate::routing::Route,
    stream: bool,
) -> RequestRecord {
    RequestRecord {
        request_id: request_id.as_str().to_owned(),
        received_at: jiff::Timestamp::now().to_string(),
        method: method.to_string(),
        path: path_and_query.to_owned(),
        requested_model: requested_model.to_owned(),
        model: route.id.clone(),
        upstream_model: route.upstream_model.clone(),
        backend: route.backend.name.clone(),
        stream,
        request_headers: headers_for_record(headers),
    }
}

async fn read_body(body: Body, limit: usize) -> Result<Bytes, RouterError> {
    axum::body::to_bytes(body, limit).await.map_err(|error| {
        let boxed = error.into_inner();
        if boxed.downcast_ref::<LengthLimitError>().is_some() {
            RouterError::BodyTooLarge { limit }
        } else {
            RouterError::BodyRead(boxed.to_string())
        }
    })
}
