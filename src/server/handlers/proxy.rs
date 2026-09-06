//! `POST /v1/messages` (and siblings): route by `model`, forward, relay.

use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use bytes::Bytes;
use http::HeaderValue;
use http_body_util::LengthLimitError;

use crate::anthropic;
use crate::observability::RequestRecord;
use crate::observability::body_log::headers_for_record;
use crate::server::annotate::annotate_upstream_error;
use crate::server::relay::{Relay, TracingObserver};
use crate::server::{AppState, RequestId, RouterError};
use crate::upstream::{
    UpstreamRequest, X_ROUTER_BACKEND, X_ROUTER_MODEL, X_ROUTER_UPSTREAM_MODEL, response_headers,
    upstream_headers,
};

pub async fn proxy(
    State(state): State<AppState>,
    request_id: RequestId,
    request: Request,
) -> Response {
    match handle(&state, &request_id, request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, status = error.status().as_u16(), "request failed in router");
            error.into_response(&request_id)
        }
    }
}

async fn handle(
    state: &AppState,
    request_id: &RequestId,
    request: Request,
) -> Result<Response, RouterError> {
    // One snapshot per request: a reload mid-flight does not mix configurations.
    let state = state.snapshot();
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
    let body = if requested_model == route.upstream_model {
        body
    } else {
        Bytes::from(anthropic::rewrite_model(&body, &route.upstream_model)?)
    };

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let headers = upstream_headers(&parts.headers, backend);
    let mut recorder = state.body_log.as_ref().map(|log| {
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
    tracing::info!(
        status = upstream.status.as_u16(),
        attempts = upstream.attempts,
        latency_ms = upstream.latency.as_millis() as u64,
        "upstream responded"
    );

    if let Some(recorder) = recorder.as_mut() {
        recorder.response_started(
            upstream.status,
            &upstream.headers,
            upstream.attempts,
            upstream.latency.as_millis() as u64,
        );
    }

    let mut headers = response_headers(&upstream.headers);
    headers.insert(X_ROUTER_BACKEND.clone(), header_value(&backend.name));
    headers.insert(X_ROUTER_MODEL.clone(), header_value(&route.id));
    headers.insert(
        X_ROUTER_UPSTREAM_MODEL.clone(),
        header_value(&route.upstream_model),
    );

    let body = if upstream.status.is_client_error() || upstream.status.is_server_error() {
        let raw = upstream
            .body
            .bytes()
            .await
            .map_err(|e| RouterError::BodyRead(e.to_string()))?;
        tracing::warn!(
            status = upstream.status.as_u16(),
            bytes = raw.len(),
            "upstream returned an error"
        );
        if let Some(recorder) = recorder.take() {
            recorder.finish_with_body(&raw);
        }
        match annotate_upstream_error(&raw, &backend.name, upstream.status, request_id.as_str()) {
            Some(annotated) => Body::from(annotated),
            None => Body::from(raw),
        }
    } else {
        let mut relay =
            Relay::new(upstream.body.bytes_stream(), span).observe(TracingObserver::new(started));
        if let Some(recorder) = recorder.take() {
            relay = relay.observe(recorder);
        }
        Body::from_stream(relay)
    };

    let mut response = Response::new(body);
    *response.status_mut() = upstream.status;
    *response.headers_mut() = headers;
    Ok(response)
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

fn header_value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).unwrap_or_else(|_| HeaderValue::from_static("invalid"))
}
