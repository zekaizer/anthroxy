//! Per-request identifier, tracing span and configuration snapshot.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderValue;
use http::header::HeaderName;
use http::request::Parts;
use tracing::Instrument;

use super::AppState;

pub static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Router-assigned id, `rtr_` + 32 hex chars. Present as a request extension
/// on every request that passed [`assign`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn generate() -> Self {
        Self(format!("rtr_{}", uuid::Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn header_value(&self) -> HeaderValue {
        HeaderValue::from_str(&self.0).expect("hex id is header-safe")
    }
}

impl<S: Send + Sync> FromRequestParts<S> for RequestId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(parts
            .extensions
            .get::<RequestId>()
            .cloned()
            .expect("assign runs on every route, the fallback included"))
    }
}

/// Assigns an id, opens the `request` span every downstream log line lives
/// in, and echoes the id in `x-request-id`. Also pins the configuration:
/// the current [`super::Snapshot`] goes in as an extension so authentication
/// and routing of one request never see two different reloads.
pub async fn assign(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let id = RequestId::generate();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.to_string())
        .unwrap_or_else(|| "-".to_owned());
    let span = tracing::info_span!(
        "request",
        id = %id.as_str(),
        method = %request.method(),
        path = %request.uri().path(),
        %peer,
        model = tracing::field::Empty,
        backend = tracing::field::Empty,
    );
    request.extensions_mut().insert(id.clone());
    request.extensions_mut().insert(state.snapshot());
    async move {
        let started = std::time::Instant::now();
        let mut response = next.run(request).await;
        response
            .headers_mut()
            .insert(X_REQUEST_ID.clone(), id.header_value());
        tracing::info!(
            status = response.status().as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "response headers sent"
        );
        response
    }
    .instrument(span)
    .await
}
