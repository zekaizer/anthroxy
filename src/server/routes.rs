//! Route table and middleware stack.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{Method, Uri};
use axum::middleware;
use axum::response::Response;
use axum::routing::{delete, get, post};

use super::handlers::{console, health, models, proxy};
use super::{AppState, RequestId, RouterError, auth, request_id, shutdown};

/// The console sends small JSON documents only.
const CONSOLE_BODY_LIMIT: usize = 64 * 1024;

pub fn build(state: AppState) -> Router {
    // ADR-0011: the console's data and actions sit behind the router token.
    let console_api = Router::new()
        .route("/api/status", get(console::status))
        .route("/api/requests", get(console::requests))
        .route("/api/requests/{id}", get(console::request))
        .route("/api/stats", get(console::stats))
        .route("/api/env", get(console::env))
        .route("/api/reload", post(console::reload))
        .route("/api/probe", post(console::probe))
        .route("/api/smoke", post(console::smoke))
        .route(
            "/api/recordings",
            get(console::recordings).delete(console::remove_recordings),
        )
        .route("/api/recordings/{name}", delete(console::remove_recording))
        .route(
            "/api/recordings/{name}/{file}",
            get(console::recording_file),
        )
        .layer(DefaultBodyLimit::max(CONSOLE_BODY_LIMIT));

    let v1 = Router::new()
        .route("/v1/models", get(models::list))
        .route("/v1/models/{id}", get(models::get_one))
        .route("/v1/messages", post(proxy::proxy))
        .route("/v1/messages/count_tokens", post(proxy::proxy))
        .route_layer(middleware::from_fn(auth::require_v1));
    let console_api = console_api.route_layer(middleware::from_fn(auth::require_client_token));

    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/", get(console::root))
        .route("/ui", get(console::root))
        .route("/ui/", get(console::index))
        .route("/ui/app.js", get(console::script))
        .route("/ui/recordings.js", get(console::recordings_script))
        .route("/ui/app.css", get(console::style))
        .merge(v1)
        .merge(console_api)
        .fallback(not_found)
        // Without this a method no route takes answers 405 with an empty body,
        // the one reply that would not be an Anthropic error document.
        .method_not_allowed_fallback(not_found)
        // The proxy enforces `server.max_body_bytes` itself with a shaped 413.
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            shutdown::cut_on_stop,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_id::assign,
        ))
        .with_state(state)
}

async fn not_found(request_id: RequestId, method: Method, uri: Uri) -> Response {
    RouterError::NoRoute {
        method: method.to_string(),
        path: uri.path().to_owned(),
    }
    .into_response(&request_id)
}
