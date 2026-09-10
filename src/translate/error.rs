use http::StatusCode;

use crate::anthropic::{ErrorResponse, ErrorType};

/// An OpenAI backend's error response as the Anthropic error document the
/// client sees: type from the status, message naming the backend.
pub fn upstream_error(
    status: StatusCode,
    raw: &[u8],
    backend: &str,
    request_id: &str,
) -> ErrorResponse {
    let message = format!(
        "{}{}",
        crate::server::backend_prefix(backend, status),
        crate::openai::error_message(raw)
    );
    ErrorResponse::new(ErrorType::from_status(status), message).with_request_id(request_id)
}
