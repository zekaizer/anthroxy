//! Client authentication: one static token, accepted as `x-api-key` or
//! `Authorization: Bearer`.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use http::header::AUTHORIZATION;
use subtle::ConstantTimeEq;

use super::{AppState, RequestId, RouterError};
use crate::credential::X_API_KEY;

#[derive(Clone)]
pub struct ClientToken(Vec<u8>);

impl ClientToken {
    pub fn new(token: &str) -> Self {
        Self(token.as_bytes().to_vec())
    }

    pub fn matches(&self, presented: &[u8]) -> bool {
        self.0.len() == presented.len() && bool::from(self.0.ct_eq(presented))
    }
}

impl std::fmt::Debug for ClientToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientToken(<redacted>)")
    }
}

/// Rejects requests whose token is missing or wrong with an Anthropic-shaped
/// 401.
pub async fn require_client_token(
    State(state): State<AppState>,
    request_id: RequestId,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(&X_API_KEY)
        .map(|v| v.as_bytes())
        .or_else(|| {
            request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|v| v.as_bytes().strip_prefix(b"Bearer "))
        });
    match presented {
        Some(token) if state.client_token.matches(token) => next.run(request).await,
        Some(_) => {
            tracing::warn!("rejected request: client token mismatch");
            RouterError::Unauthorized.into_response(&request_id)
        }
        None => {
            tracing::warn!("rejected request: no client token");
            RouterError::Unauthorized.into_response(&request_id)
        }
    }
}
