use http::StatusCode;

use crate::anthropic::{self, ErrorResponse};
use crate::config::BackendKind;
use crate::ir::Failure;

/// An error body from any backend as an IR failure, by the codec its kind
/// calls for. The only place an error decoder is chosen; `status` is absent
/// inside an event stream, where the document's own names are all there is.
pub fn failure(kind: BackendKind, status: Option<u16>, raw: &[u8]) -> Failure {
    match kind {
        BackendKind::OpenAi => crate::openai::decode_error(status, raw),
        BackendKind::Anthropic | BackendKind::Passthrough => anthropic::decode_error(status, raw),
    }
}

/// An OpenAI backend's error response as the Anthropic error document the
/// client sees: decoded into an IR failure, encoded back out with the
/// backend named.
pub fn upstream_error(
    status: StatusCode,
    raw: &[u8],
    backend: &str,
    request_id: &str,
) -> ErrorResponse {
    let failure = failure(BackendKind::OpenAi, Some(status.as_u16()), raw);
    anthropic::encode_error(&failure, &crate::server::backend_prefix(backend, status))
        .with_request_id(request_id)
}
