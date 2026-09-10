use http::StatusCode;
use serde::Serialize;

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
