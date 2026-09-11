//! `POST /v1/messages` (and siblings): route by `model`, forward, relay.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Extension;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use bytes::Bytes;
use http::HeaderValue;
use http::header::CONTENT_TYPE;
use http_body_util::LengthLimitError;

use crate::activity::hints::{self, UpstreamFailure};
use crate::activity::{Exchange, Source};
use crate::anthropic;
use crate::config::BackendKind;
use crate::observability::body_log::headers_for_record;
use crate::observability::{Recorder, RequestRecord};
use crate::server::annotate::annotate_upstream_error;
use crate::server::buffered::read_all;
use crate::server::handlers::openai;
use crate::server::relay::{Relay, RelayOutcome};
use crate::server::{AppState, RequestId, RouterError, Snapshot};
use crate::stats::StatsRecord;
use crate::text::short;
use crate::translate;
use crate::upstream::{
    UpstreamError, UpstreamRequest, X_ROUTER_BACKEND, X_ROUTER_MODEL, X_ROUTER_UPSTREAM_MODEL,
    header_value, response_headers, upstream_headers,
};

pub async fn proxy(
    State(app): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    request: Request,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.to_string());
    let exchange = app.activity.begin(
        request_id.as_str(),
        Source::Client,
        peer,
        request.method().as_str(),
        request.uri().path(),
    );
    serve(&snapshot, &request_id, request, exchange).await
}

/// Routes, forwards and relays `request` on `snapshot`, reporting to
/// `exchange`; a `/v1/messages` exchange also goes to the statistics.
pub async fn serve(
    snapshot: &Snapshot,
    request_id: &RequestId,
    request: Request,
    mut exchange: Exchange,
) -> Response {
    if request.uri().path() == "/v1/messages"
        && let Some(log) = snapshot.stats.clone()
    {
        exchange.on_finish(move |view| {
            if let Some(record) = StatsRecord::from_view(view) {
                log.append(record);
            }
        });
    }
    let mut exchange = Some(exchange);
    match handle(snapshot, request_id, request, &mut exchange).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, status = error.status().as_u16(), "request failed in router");
            if let Some(exchange) = exchange {
                exchange.fail(error.status().as_u16(), error.to_string());
            }
            error.into_response(request_id)
        }
    }
}

/// `exchange` stays in place until a response body takes it; an error return
/// leaves it for the caller to fail.
async fn handle(
    state: &Snapshot,
    request_id: &RequestId,
    request: Request,
    exchange: &mut Option<Exchange>,
) -> Result<Response, RouterError> {
    let started = Instant::now();
    let (parts, body) = request.into_parts();
    let body = read_body(body, state.max_body_bytes).await?;

    let peek = anthropic::peek(&body)?;
    let requested_model = peek.model.expect("peek guarantees a model");
    note(exchange, |e| e.requested(&requested_model, peek.stream));
    let resolution = state.registry.resolve(&requested_model).ok_or_else(|| {
        note(exchange, |e| e.unrouted(&requested_model));
        RouterError::unknown_model(&requested_model, &state.registry)
    })?;
    let route = resolution.route;
    let backend = &route.backend;
    note(exchange, |e| {
        e.routed(
            &requested_model,
            resolution.matched,
            &route.id,
            &backend.name,
            backend.kind,
            &route.upstream_model,
        )
    });
    let span = tracing::Span::current();
    span.record("model", route.id.as_str());
    span.record("backend", backend.name.as_str());
    tracing::info!(
        // Whatever the client sent: `routing.default_model` routes a name the
        // model table never saw, so the log takes it escaped and cut.
        requested_model = %short(&requested_model),
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
    let body = match anthropic::rewrite(&body, rename, &backend.drop_fields, anthropic_kind)? {
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
    if let Some(recorder) = &recorder {
        note(exchange, |e| e.recording(recorder.entry()));
    }
    let upstream = match state
        .upstream
        .send(UpstreamRequest {
            backend,
            method: parts.method.clone(),
            path_and_query,
            headers,
            body,
        })
        .await
    {
        Ok(upstream) => upstream,
        Err(error) => {
            record_failure(recorder, &error);
            return Err(error.into());
        }
    };
    let status = upstream.response.status();
    tracing::info!(
        status = status.as_u16(),
        attempts = upstream.attempts,
        latency_ms = upstream.latency.as_millis() as u64,
        "upstream responded"
    );
    note(exchange, |e| {
        e.responded(
            status.as_u16(),
            upstream.attempts,
            upstream.credential_refreshed,
            upstream.latency,
        )
    });

    let recorder = recorder.map(|mut recorder| {
        recorder.response_started(
            status,
            upstream.response.headers(),
            upstream.attempts,
            upstream.latency.as_millis() as u64,
        );
        recorder
    });

    if let Some(location) = redirect_target(status, upstream.response.headers()) {
        let error = UpstreamError::Redirected {
            backend: backend.name.clone(),
            status: status.as_u16(),
            location: short(location),
        };
        record_failure(recorder, &error);
        return Err(error.into());
    }

    let mut headers = response_headers(upstream.response.headers());
    headers.insert(X_ROUTER_BACKEND.clone(), header_value(&backend.name));
    headers.insert(X_ROUTER_MODEL.clone(), header_value(&route.id));
    headers.insert(
        X_ROUTER_UPSTREAM_MODEL.clone(),
        header_value(&route.upstream_model),
    );
    let content_type = upstream
        .response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let body = if status.is_client_error() || status.is_server_error() {
        // ADR-0005: error bodies are buffered so they can be annotated.
        let raw = read_all(upstream, &backend.name, recorder).await?;
        tracing::warn!(
            status = status.as_u16(),
            bytes = raw.len(),
            "upstream returned an error"
        );
        let text = String::from_utf8_lossy(&raw);
        note(exchange, |e| {
            e.upstream_error(
                &raw,
                hints::hints(&UpstreamFailure {
                    backend: &backend.name,
                    kind: backend.kind,
                    upstream_model: &route.upstream_model,
                    drop_fields: &backend.drop_fields,
                    status: status.as_u16(),
                    body: &text,
                }),
            )
        });
        let (bytes, content_type) = match backend.kind {
            BackendKind::Anthropic => {
                match annotate_upstream_error(&raw, &backend.name, status, request_id.as_str()) {
                    Some(annotated) => (Bytes::from(annotated), content_type),
                    None => (raw, content_type),
                }
            }
            BackendKind::OpenAi => {
                let document =
                    translate::upstream_error(status, &raw, &backend.name, request_id.as_str());
                headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                (
                    Bytes::from(
                        serde_json::to_vec(&document).expect("an error document serializes"),
                    ),
                    Some("application/json".to_owned()),
                )
            }
        };
        if let Some(exchange) = exchange.take() {
            exchange.finish_body(status.as_u16(), &bytes, content_type.as_deref());
        }
        Body::from(bytes)
    } else {
        match backend.kind {
            // ADR-0003: relayed as it arrives.
            BackendKind::Anthropic => {
                let relay = Relay::new(upstream.response.bytes_stream(), span, started, recorder);
                match exchange.take() {
                    Some(exchange) => {
                        Body::from_stream(exchange.track(relay, content_type.as_deref()))
                    }
                    None => Body::from_stream(relay),
                }
            }
            BackendKind::OpenAi => {
                let (overrides, body) = openai::body(
                    upstream,
                    &backend.name,
                    &route.upstream_model,
                    peek.stream,
                    span,
                    started,
                    recorder,
                    exchange,
                )
                .await?;
                headers.extend(overrides);
                body
            }
        }
    };

    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

/// Reports to the exchange while it is still in place.
fn note(exchange: &Option<Exchange>, report: impl FnOnce(&Exchange)) {
    if let Some(exchange) = exchange {
        report(exchange);
    }
}

/// Where a redirecting response points. A 3xx without a `Location` is nothing
/// a client can follow, so it is relayed like any other status.
fn redirect_target(status: http::StatusCode, headers: &http::HeaderMap) -> Option<&str> {
    status
        .is_redirection()
        .then(|| headers.get(http::header::LOCATION)?.to_str().ok())
        .flatten()
}

/// Ends the record of an exchange that failed before a body was relayed;
/// without it the entry keeps the shape it had when the request went out.
fn record_failure(recorder: Option<Recorder>, error: &impl std::fmt::Display) {
    if let Some(recorder) = recorder {
        recorder.finish(&RelayOutcome::UpstreamError(error.to_string()));
    }
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
