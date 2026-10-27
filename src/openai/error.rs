//! What an error body from an OpenAI-compatible server says, as an IR
//! failure.

use crate::ir::{Failure, FailureKind, says_context_exceeded};

use super::common::{error_document, error_names, parse_object};

/// Characters of a non-JSON body reported to the client.
pub const MAX_RAW_MESSAGE: usize = 200;

/// The failure an error body stands for. `status` is the HTTP status it
/// arrived with, absent inside an event stream. A context overflow named
/// anywhere in the body is that kind whatever the status; otherwise the
/// status decides where it says something, and the names the document uses
/// do where it does not.
pub fn decode(status: Option<u16>, raw: &[u8]) -> Failure {
    let fields = parse_object(raw).ok();
    let (message, names) = match fields
        .as_ref()
        .and_then(|fields| Some((error_document(fields)?, fields)))
    {
        Some((message, fields)) => (message, error_names(fields).collect()),
        None => (
            String::from_utf8_lossy(raw)
                .trim()
                .chars()
                .take(MAX_RAW_MESSAGE)
                .collect(),
            Vec::new(),
        ),
    };
    let overflow = names
        .iter()
        .copied()
        .chain([message.as_str()])
        .any(says_context_exceeded);
    let kind = match status.map(FailureKind::from_status) {
        _ if overflow => FailureKind::ContextExceeded,
        Some(kind) if !kind.is_unspecific() => kind,
        _ => names
            .into_iter()
            .find_map(named_kind)
            .unwrap_or(FailureKind::Upstream),
    };
    Failure::new(kind, message)
}

/// The kind an OpenAI-compatible `error.type` or `error.code` stands for.
/// Servers spell these loosely — `invalid_request_error`, `BadRequestError`,
/// `rate_limit_exceeded` — so separators are dropped and a name is matched by
/// what it contains.
fn named_kind(name: &str) -> Option<FailureKind> {
    let name: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let has = |needle: &str| name.contains(needle);
    if has("ratelimit") || has("quota") {
        Some(FailureKind::RateLimit)
    } else if has("overload") || has("unavailable") || has("capacity") {
        Some(FailureKind::Overloaded)
    } else if has("authentication") || has("apikey") || has("unauthorized") {
        Some(FailureKind::Authentication)
    } else if has("permission") || has("forbidden") {
        Some(FailureKind::Permission)
    } else if has("notfound") {
        Some(FailureKind::NotFound)
    } else if has("toolarge") || has("toolong") {
        Some(FailureKind::TooLarge)
    } else if has("invalid") || has("badrequest") {
        Some(FailureKind::InvalidRequest)
    } else {
        None
    }
}
