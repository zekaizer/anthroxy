//! Which headers cross the router, and how.

use http::header::{
    ACCEPT_ENCODING, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, HOST, HeaderMap, HeaderName,
    HeaderValue, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};

use super::Backend;
use crate::credential::X_API_KEY;

pub static ANTHROPIC_BETA: HeaderName = HeaderName::from_static("anthropic-beta");
pub static X_ROUTER_BACKEND: HeaderName = HeaderName::from_static("x-anthroxy-backend");
pub static X_ROUTER_MODEL: HeaderName = HeaderName::from_static("x-anthroxy-model");
pub static X_ROUTER_UPSTREAM_MODEL: HeaderName =
    HeaderName::from_static("x-anthroxy-upstream-model");
static KEEP_ALIVE: HeaderName = HeaderName::from_static("keep-alive");
static PROXY_CONNECTION: HeaderName = HeaderName::from_static("proxy-connection");

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
/// are merged into the client's list.
pub fn upstream_headers(client: &HeaderMap, backend: &Backend) -> HeaderMap {
    let mut out = HeaderMap::with_capacity(client.len() + backend.headers.len() + 1);
    for (name, value) in client {
        if is_hop_by_hop(name)
            || *name == HOST
            || *name == CONTENT_LENGTH
            || *name == AUTHORIZATION
            || *name == X_API_KEY
            || *name == ACCEPT_ENCODING
        {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    for (name, value) in &backend.headers {
        out.insert(name.clone(), value.clone());
    }
    if !backend.anthropic_beta.is_empty() {
        let merged = merge_beta(out.get(&ANTHROPIC_BETA), &backend.anthropic_beta);
        out.insert(
            ANTHROPIC_BETA.clone(),
            HeaderValue::from_str(&merged).expect("validated flags"),
        );
    }
    out
}

/// `existing` (comma-separated) plus every flag in `extra` not already listed.
fn merge_beta(existing: Option<&HeaderValue>, extra: &[String]) -> String {
    let mut flags: Vec<String> = existing
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for flag in extra {
        if !flags.iter().any(|f| f == flag) {
            flags.push(flag.clone());
        }
    }
    flags.join(",")
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
