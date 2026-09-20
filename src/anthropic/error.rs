use http::StatusCode;
use serde::Serialize;

use crate::ir::{Failure, FailureKind};

/// `error.type` values defined by the Anthropic API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    InvalidRequestError,
    AuthenticationError,
    PermissionError,
    NotFoundError,
    RequestTooLarge,
    RateLimitError,
    ApiError,
    OverloadedError,
}

impl ErrorType {
    /// The type a backend's status maps to when its body is not an Anthropic
    /// error document (ADR-0010): the six statuses the API defines, `ApiError`
    /// for everything else.
    pub fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::BAD_REQUEST => ErrorType::InvalidRequestError,
            StatusCode::UNAUTHORIZED => ErrorType::AuthenticationError,
            StatusCode::FORBIDDEN => ErrorType::PermissionError,
            StatusCode::NOT_FOUND => ErrorType::NotFoundError,
            StatusCode::PAYLOAD_TOO_LARGE => ErrorType::RequestTooLarge,
            StatusCode::TOO_MANY_REQUESTS => ErrorType::RateLimitError,
            _ => ErrorType::ApiError,
        }
    }

    /// The type an `error.type` names, when the API defines it.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "invalid_request_error" => ErrorType::InvalidRequestError,
            "authentication_error" => ErrorType::AuthenticationError,
            "permission_error" => ErrorType::PermissionError,
            "not_found_error" => ErrorType::NotFoundError,
            "request_too_large" => ErrorType::RequestTooLarge,
            "rate_limit_error" => ErrorType::RateLimitError,
            "api_error" => ErrorType::ApiError,
            "overloaded_error" => ErrorType::OverloadedError,
            _ => return None,
        })
    }

    /// The IR kind this type stands for.
    pub fn kind(self) -> FailureKind {
        match self {
            ErrorType::InvalidRequestError => FailureKind::InvalidRequest,
            ErrorType::AuthenticationError => FailureKind::Authentication,
            ErrorType::PermissionError => FailureKind::Permission,
            ErrorType::NotFoundError => FailureKind::NotFound,
            ErrorType::RequestTooLarge => FailureKind::TooLarge,
            ErrorType::RateLimitError => FailureKind::RateLimit,
            ErrorType::ApiError => FailureKind::Upstream,
            ErrorType::OverloadedError => FailureKind::Overloaded,
        }
    }

    /// The type an IR failure is reported as.
    pub fn from_kind(kind: FailureKind) -> Self {
        match kind {
            FailureKind::InvalidRequest => ErrorType::InvalidRequestError,
            FailureKind::Authentication => ErrorType::AuthenticationError,
            FailureKind::Permission => ErrorType::PermissionError,
            FailureKind::NotFound => ErrorType::NotFoundError,
            FailureKind::TooLarge => ErrorType::RequestTooLarge,
            FailureKind::RateLimit => ErrorType::RateLimitError,
            FailureKind::Overloaded => ErrorType::OverloadedError,
            FailureKind::Upstream => ErrorType::ApiError,
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            ErrorType::InvalidRequestError => StatusCode::BAD_REQUEST,
            ErrorType::AuthenticationError => StatusCode::UNAUTHORIZED,
            ErrorType::PermissionError => StatusCode::FORBIDDEN,
            ErrorType::NotFoundError => StatusCode::NOT_FOUND,
            ErrorType::RequestTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorType::RateLimitError => StatusCode::TOO_MANY_REQUESTS,
            ErrorType::ApiError => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorType::OverloadedError => StatusCode::from_u16(529).expect("valid"),
        }
    }
}

/// `{"type":"error","error":{"type":..,"message":..},"request_id":..}`
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorResponse {
    #[serde(rename = "type")]
    pub kind: String,
    pub error: ErrorDetail,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorDetail {
    #[serde(rename = "type")]
    pub kind: ErrorType,
    pub message: String,
}

impl ErrorResponse {
    pub fn new(kind: ErrorType, message: impl Into<String>) -> Self {
        Self {
            kind: "error".to_owned(),
            error: ErrorDetail {
                kind,
                message: message.into(),
            },
            request_id: None,
        }
    }

    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    pub fn status(&self) -> StatusCode {
        self.error.kind.status()
    }
}

/// An IR failure as the error document the client reads. `prefix` names
/// where it came from; the API has no field for that.
pub fn encode_error(failure: &Failure, prefix: &str) -> ErrorResponse {
    ErrorResponse::new(
        ErrorType::from_kind(failure.kind),
        format!("{prefix}{}", failure.message),
    )
}

/// An Anthropic error body as an IR failure: the document's own type when it
/// names one the API defines, the status otherwise. `status` is absent when
/// the body did not arrive with one.
pub fn decode_error(status: Option<u16>, raw: &[u8]) -> Failure {
    let document: Option<serde_json::Value> = serde_json::from_slice(raw).ok();
    let error = document.as_ref().and_then(|d| d.get("error"));
    let named = error
        .and_then(|e| e.get("type"))
        .and_then(|t| t.as_str())
        .and_then(ErrorType::from_name)
        .map(ErrorType::kind);
    let kind = named
        .or_else(|| status.map(FailureKind::from_status))
        .unwrap_or(FailureKind::Upstream);
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            String::from_utf8_lossy(raw)
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(MAX_DETAIL)
                .collect()
        });
    Failure::new(kind, message)
}

/// Characters of a body that is not an error document reported as the detail.
const MAX_DETAIL: usize = 160;
