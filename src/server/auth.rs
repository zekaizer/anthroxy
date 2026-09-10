//! Client authentication: one static token, accepted as `x-api-key` or
//! `Authorization: Bearer`.

use std::sync::Arc;

use axum::Extension;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use http::HeaderValue;
use http::header::AUTHORIZATION;
use subtle::ConstantTimeEq;

use super::{RequestId, RouterError, Snapshot};
use crate::config::X_API_KEY;

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
/// 401. Both headers are read: Claude Code sends `x-api-key` for
/// ANTHROPIC_API_KEY and the bearer for ANTHROPIC_AUTH_TOKEN, so a shell with
/// both variables set presents both, and either one carrying the token is a
/// request the router asked for.
pub async fn require_client_token(
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    request: Request,
    next: Next,
) -> Response {
    let (authorized, presented) = {
        let headers = request.headers();
        let candidates = [
            headers.get(&X_API_KEY).map(HeaderValue::as_bytes),
            headers
                .get(AUTHORIZATION)
                .and_then(|v| v.as_bytes().strip_prefix(b"Bearer ")),
        ];
        (
            candidates
                .iter()
                .flatten()
                .any(|token| snapshot.client_token.matches(token)),
            candidates.iter().any(Option::is_some),
        )
    };
    if authorized {
        return next.run(request).await;
    }
    if presented {
        tracing::warn!("rejected request: client token mismatch");
    } else {
        tracing::warn!("rejected request: no client token");
    }
    RouterError::Unauthorized.into_response(&request_id)
}
