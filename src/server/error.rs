//! Everything the router itself answers with, in the Anthropic error shape.

use axum::response::{IntoResponse, Response};
use http::StatusCode;

use super::RequestId;
use crate::anthropic::{ErrorResponse, ErrorType, PeekError};
use crate::upstream::UpstreamError;

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("missing or invalid router token; send it as `x-api-key` or `Authorization: Bearer`")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(#[from] PeekError),
    #[error("request body exceeds the router limit of {limit} bytes")]
    BodyTooLarge { limit: usize },
    #[error("cannot read request body: {0}")]
    BodyRead(String),
    #[error("model `{model}` is not served by this router; configured models: {}", known.join(", "))]
    UnknownModel { model: String, known: Vec<String> },
    #[error("no route for {method} {path}")]
    NoRoute { method: String, path: String },
    #[error("model `{model}` refers to backend `{backend}`, which is not configured")]
    BackendMissing { model: String, backend: String },
    #[error(transparent)]
    Upstream(#[from] UpstreamError),
    #[error("cannot rewrite request body: {0}")]
    Rewrite(#[from] serde_json::Error),
}

impl RouterError {
    pub fn error_type(&self) -> ErrorType {
        match self {
            RouterError::Unauthorized => ErrorType::AuthenticationError,
            RouterError::BadRequest(_) | RouterError::BodyRead(_) => ErrorType::InvalidRequestError,
            RouterError::BodyTooLarge { .. } => ErrorType::RequestTooLarge,
            RouterError::UnknownModel { .. } | RouterError::NoRoute { .. } => {
                ErrorType::NotFoundError
            }
            RouterError::BackendMissing { .. }
            | RouterError::Upstream(_)
            | RouterError::Rewrite(_) => ErrorType::ApiError,
        }
    }

    /// Gateway-level failures answer 502 so the client can tell "the router
    /// could not reach the backend" from a backend's own 500.
    pub fn status(&self) -> StatusCode {
        match self {
            RouterError::Upstream(_) => StatusCode::BAD_GATEWAY,
            other => other.error_type().status(),
        }
    }

    /// Backend involved, for the `x-claude-router-backend` header.
    pub fn backend(&self) -> Option<&str> {
        match self {
            RouterError::Upstream(UpstreamError::Credential { backend, .. })
            | RouterError::Upstream(UpstreamError::Transport { backend, .. })
            | RouterError::BackendMissing { backend, .. } => Some(backend),
            _ => None,
        }
    }

    pub fn into_response(self, request_id: &RequestId) -> Response {
        let status = self.status();
        let body = ErrorResponse::new(self.error_type(), self.to_string())
            .with_request_id(request_id.as_str());
        let mut response = (status, axum::Json(body)).into_response();
        if let Some(backend) = self.backend() {
            response.headers_mut().insert(
                crate::upstream::X_ROUTER_BACKEND.clone(),
                http::HeaderValue::from_str(backend).expect("backend names are header-safe"),
            );
        }
        response
    }
}
