//! Everything the router itself answers with, in the Anthropic error shape.

use axum::response::{IntoResponse, Response};
use http::StatusCode;

use super::RequestId;
use crate::anthropic::{ErrorResponse, ErrorType, PeekError};
use crate::routing::Registry;
use crate::text::{cut, short};
use crate::upstream::UpstreamError;

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("missing or invalid router token; send it as `x-api-key` or `Authorization: Bearer`")]
    Unauthorized,
    #[error("{}", cut(&.0.to_string(), 200))]
    BadRequest(#[from] PeekError),
    #[error("request body exceeds the router limit of {limit} bytes")]
    BodyTooLarge { limit: usize },
    #[error("cannot read request body: {0}")]
    BodyRead(String),
    #[error("model `{}` is not served by this router; configured models: {}", short(model), known.join(", "))]
    UnknownModel { model: String, known: Vec<String> },
    #[error("no route for {} {}", short(method), short(path))]
    NoRoute { method: String, path: String },
    #[error(transparent)]
    Upstream(#[from] UpstreamError),
}

impl RouterError {
    pub fn unknown_model(model: impl Into<String>, registry: &Registry) -> Self {
        RouterError::UnknownModel {
            model: model.into(),
            known: registry
                .known_names()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    pub fn error_type(&self) -> ErrorType {
        match self {
            RouterError::Unauthorized => ErrorType::AuthenticationError,
            RouterError::BadRequest(_) | RouterError::BodyRead(_) => ErrorType::InvalidRequestError,
            RouterError::BodyTooLarge { .. } => ErrorType::RequestTooLarge,
            RouterError::UnknownModel { .. } | RouterError::NoRoute { .. } => {
                ErrorType::NotFoundError
            }
            RouterError::Upstream(_) => ErrorType::ApiError,
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

    /// Backend involved, for the `x-anthroxy-backend` header.
    pub fn backend(&self) -> Option<&str> {
        match self {
            RouterError::Upstream(
                UpstreamError::Credential { backend, .. }
                | UpstreamError::Transport { backend, .. }
                | UpstreamError::Redirected { backend, .. }
                | UpstreamError::Body { backend, .. },
            ) => Some(backend),
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
                crate::upstream::header_value(backend),
            );
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_model_name_does_not_reach_the_message_in_full() {
        let error = RouterError::UnknownModel {
            model: "m".repeat(10_000),
            known: vec!["m".to_owned()],
        };
        assert!(error.to_string().len() < 200, "{}", error.to_string().len());
    }

    #[test]
    fn a_body_that_will_not_parse_does_not_reach_the_message_in_full() {
        let quoted = format!("\"{}\"", "x".repeat(5_000));
        let error = RouterError::BadRequest(PeekError::NotJson(
            serde_json::from_str::<bool>(&quoted).unwrap_err(),
        ));
        let text = error.to_string();
        assert!(text.len() < 300, "{} chars", text.len());
        assert!(text.contains("invalid type: string"), "{text}");
    }

    #[test]
    fn a_long_path_and_method_do_not_reach_the_message_in_full() {
        let error = RouterError::NoRoute {
            method: "X".repeat(4096),
            path: format!("/v1/{}", "a".repeat(4096)),
        };
        assert!(error.to_string().len() < 200, "{}", error.to_string().len());
    }

    #[test]
    fn an_unknown_model_name_cannot_forge_a_log_line() {
        let error = RouterError::UnknownModel {
            model: "ghost\n2026-09-09T00:00:00Z  INFO forged".to_owned(),
            known: vec!["m".to_owned()],
        };
        let text = error.to_string();
        assert!(!text.contains('\n'), "{text}");
        assert!(text.contains("ghost\\n2026"), "{text}");
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
    }
}
