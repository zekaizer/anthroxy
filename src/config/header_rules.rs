//! The rules a `headers` table and a `drop_headers` list obey, wherever the
//! configuration has one. Every problem starts with the key path the caller
//! gives.

use std::collections::{BTreeMap, HashMap};

use super::{HeaderPattern, Template, TemplateError};
use crate::text::short;

/// Framing and hop-by-hop headers: the HTTP client owns them, so a value set
/// here is either dropped or produces a request no backend can read. `host` is
/// not one of them; overriding it is how some gateways are addressed.
const CONNECTION_HEADERS: [&str; 8] = [
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
];

/// Headers the router itself never forwards, so naming one to drop says
/// nothing.
const ALREADY_DROPPED: [&str; 7] = [
    "host",
    "content-length",
    "authorization",
    "x-api-key",
    "accept-encoding",
    "connection",
    "proxy-authorization",
];

/// Headers the backend needs to read the request the router sends it.
const REQUIRED: [(&str, &str); 1] = [("content-type", "says what the body is")];

/// The problems of a `headers` table written at `field`.
pub(super) fn check_forced_headers(
    field: &str,
    headers: &BTreeMap<String, String>,
    problems: &mut Vec<String>,
) {
    // Header names are case-insensitive, table keys are not: two spellings
    // would be one header set twice, the later undoing the earlier.
    let mut spelled: HashMap<String, &str> = HashMap::new();
    for header in headers.keys() {
        if let Some(first) = spelled.insert(header.to_ascii_lowercase(), header) {
            problems.push(format!(
                "{field}: `{}` and `{}` are the same header",
                short(first),
                short(header)
            ));
        }
    }
    for (header, value) in headers {
        if http::HeaderName::from_bytes(header.as_bytes()).is_err() {
            problems.push(format!(
                "{field}: `{}` is not a valid header name",
                short(header)
            ));
        } else if CONNECTION_HEADERS.contains(&header.to_ascii_lowercase().as_str()) {
            problems.push(format!(
                "{field}: `{}` describes the connection the router makes and cannot be set here",
                short(header)
            ));
        } else if http::HeaderValue::from_str(value).is_err() {
            problems.push(format!(
                "{field}: `{}` has a value that cannot be sent in a header",
                short(header)
            ));
        } else {
            match Template::parse(value) {
                // The value may be a key, and what follows `{header:` is
                // part of it: the problem quotes none of it.
                Err(TemplateError::NotAHeaderName(_)) => problems.push(format!(
                    "{field}: `{}`: a `{{header:…}}` placeholder does not name a header",
                    short(header)
                )),
                Err(problem) => problems.push(format!("{field}: `{}`: {problem}", short(header))),
                Ok(template) => {
                    let required = REQUIRED
                        .iter()
                        .find(|(required, _)| header.eq_ignore_ascii_case(required));
                    if let Some((_, why)) = required
                        && template.reads_client()
                    {
                        problems.push(format!(
                            "{field}: `{}` {why}; a value read from the client's request may be left out",
                            short(header)
                        ));
                    }
                }
            }
        }
    }
}

/// The problems of a `drop_headers` list written at `field`, beside a
/// `headers` table that forces the names in `forced`. An entry must be a
/// pattern, and must not take back what the rest says: a header that is
/// forced, one the router drops anyway, or one the request cannot be read
/// without.
pub(super) fn check_drop_headers(
    field: &str,
    entries: &[String],
    forced: &[&str],
    problems: &mut Vec<String>,
) {
    for entry in entries {
        let patterns = match HeaderPattern::parse(entry) {
            Ok(patterns) => patterns,
            Err(problem) => {
                problems.push(format!("{field}: {problem}"));
                continue;
            }
        };
        let shown = entry.escape_debug();
        let hits = |header: &str| patterns.iter().any(|pattern| pattern.matches(header));
        // A pattern says which header it caught; a name is already the name.
        let caught = |header: &str| match entry.eq_ignore_ascii_case(header) {
            true => String::new(),
            false => format!(" (`{header}`)"),
        };
        if let Some(forced) = forced.iter().find(|k| hits(&k.to_ascii_lowercase())) {
            problems.push(format!(
                "{field}: `{shown}` is set in `headers` for this backend{}; a header is forced or dropped, not both",
                caught(&forced.to_ascii_lowercase())
            ));
        }
        if let Some(header) = ALREADY_DROPPED.iter().find(|header| hits(header)) {
            problems.push(format!(
                "{field}: `{shown}` never reaches a backend anyway{}",
                caught(header)
            ));
        }
        if let Some((header, why)) = REQUIRED.iter().find(|(header, _)| hits(header)) {
            problems.push(format!("{field}: `{shown}` {why}{}", caught(header)));
        }
    }
}
