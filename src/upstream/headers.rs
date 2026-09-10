//! Which headers cross the router, and how.

use http::header::{
    ACCEPT_ENCODING, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap,
    HeaderName, HeaderValue, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};

use super::Backend;
use crate::config::{BackendKind, X_API_KEY};

pub static ANTHROPIC_BETA: HeaderName = HeaderName::from_static("anthropic-beta");
static ANTHROPIC_VERSION: HeaderName = HeaderName::from_static("anthropic-version");
pub static X_ROUTER_BACKEND: HeaderName = HeaderName::from_static("x-anthroxy-backend");
pub static X_ROUTER_MODEL: HeaderName = HeaderName::from_static("x-anthroxy-model");
pub static X_ROUTER_UPSTREAM_MODEL: HeaderName =
    HeaderName::from_static("x-anthroxy-upstream-model");
static KEEP_ALIVE: HeaderName = HeaderName::from_static("keep-alive");
static PROXY_CONNECTION: HeaderName = HeaderName::from_static("proxy-connection");

/// A backend name or model id as a header value. Validation guarantees they
/// are header-safe.
pub fn header_value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).expect("validated header-safe")
}

/// Hop-by-hop headers belong to one connection and are never relayed.
fn is_hop_by_hop(name: &HeaderName) -> bool {
    *name == CONNECTION
        || *name == KEEP_ALIVE
        || *name == PROXY_CONNECTION
        || *name == TE
        || *name == TRAILER
        || *name == TRANSFER_ENCODING
        || *name == UPGRADE
}

/// Builds the header set for the upstream request from the client's headers.
/// The backend credential is not part of it; [`super::UpstreamClient`] adds
/// it per attempt.
///
/// Dropped: hop-by-hop headers, `host` and `content-length` (owned by the
/// client library), the client's own `authorization`/`x-api-key` (replaced by
/// the backend credential) and `accept-encoding` (bodies are relayed and
/// logged uncompressed). Backend `headers` override, `anthropic_beta` flags
/// are merged into the client's list. An `openai` backend gets no
/// `anthropic-version` or `anthropic-beta` at all (ADR-0010).
pub fn upstream_headers(client: &HeaderMap, backend: &Backend) -> HeaderMap {
    let anthropic = backend.kind == BackendKind::Anthropic;
    let mut out = HeaderMap::with_capacity(client.len() + backend.headers.len() + 1);
    for (name, value) in client {
        if is_hop_by_hop(name)
            || *name == HOST
            || *name == CONTENT_LENGTH
            || *name == AUTHORIZATION
            || *name == X_API_KEY
            || *name == ACCEPT_ENCODING
            || (!anthropic && (*name == ANTHROPIC_VERSION || *name == ANTHROPIC_BETA))
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    for (name, value) in &backend.headers {
        out.insert(name.clone(), value.clone());
    }
    if !backend.anthropic_beta.is_empty() {
        let merged = merge_beta(out.get_all(&ANTHROPIC_BETA), &backend.anthropic_beta);
        out.insert(
            ANTHROPIC_BETA.clone(),
            HeaderValue::from_str(&merged).expect("validated flags"),
        );
    }
    out
}

/// Every flag of every `existing` header (each comma-separated), then those
/// of `extra`, each listed once. The client may repeat the header, so all of
/// them are folded into the single value that replaces them.
fn merge_beta<'a>(existing: impl IntoIterator<Item = &'a HeaderValue>, extra: &[String]) -> String {
    let mut flags: Vec<&str> = Vec::new();
    let values: Vec<&str> = existing
        .into_iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    let listed = values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|flag| !flag.is_empty());
    for flag in listed.chain(extra.iter().map(String::as_str)) {
        if !flags.contains(&flag) {
            flags.push(flag);
        }
    }
    flags.join(",")
}

/// Whether a response body is server-sent events, by its content type.
pub fn is_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

/// Headers relayed from the upstream response to the client. Framing headers
/// are dropped because the router re-frames the body it forwards.
pub fn response_headers(upstream: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::with_capacity(upstream.len());
    for (name, value) in upstream {
        if is_hop_by_hop(name) || *name == CONTENT_LENGTH {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}
