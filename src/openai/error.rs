//! What an error body from an OpenAI-compatible server says, as an IR
//! failure.

use crate::ir::{Failure, FailureKind};

use super::common::{error_document, error_names, parse_object};

/// Characters of a non-JSON body reported to the client.
pub const MAX_RAW_MESSAGE: usize = 200;

/// The failure an error body stands for. `status` is the HTTP status it
/// arrived with, absent inside an event stream. The status decides the kind
/// where it says something; where it does not, the names the document uses
/// do.
pub fn decode(status: Option<u16>, raw: &[u8]) -> Failure {
    let kind = status.map(FailureKind::from_status);
    if let Ok(fields) = parse_object(raw)
        && let Some(message) = error_document(&fields)
    {
        let named = error_names(&fields).find_map(named_kind);
        let kind = match kind {
            Some(kind) if !kind.is_unspecific() => kind,
            _ => named.unwrap_or(FailureKind::Upstream),
        };
        return Failure::new(kind, message);
    }
    let message: String = String::from_utf8_lossy(raw)
        .trim()
        .chars()
        .take(MAX_RAW_MESSAGE)
        .collect();
    Failure::new(kind.unwrap_or(FailureKind::Upstream), message)
}

/// The kind an OpenAI-compatible `error.type` or `error.code` stands for.
/// Servers spell these loosely — `invalid_request_error`, `BadRequestError`,
/// `context_length_exceeded` — so separators are dropped and a name is
/// matched by what it contains.
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
    } else if has("invalid") || has("badrequest") || has("contextlength") {
        Some(FailureKind::InvalidRequest)
    } else {
        None
    }
}
