//! Route table and middleware stack.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{Method, Uri};
use axum::middleware;
use axum::response::Response;
use axum::routing::{get, post};

use super::handlers::{health, models, proxy};
use super::{AppState, RequestId, RouterError, auth, request_id};

pub fn build(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/models", get(models::list))
        .route("/v1/models/{id}", get(models::get_one))
        .route("/v1/messages", post(proxy::proxy))
        .route("/v1/messages/count_tokens", post(proxy::proxy))
        .route_layer(middleware::from_fn(auth::require_client_token));

    Router::new()
        .route("/healthz", get(health::healthz))
        .merge(protected)
        .fallback(not_found)
        // The proxy enforces `server.max_body_bytes` itself with a shaped 413.
        .layer(DefaultBodyLimit::disable())
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
