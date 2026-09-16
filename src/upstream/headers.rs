//! Which headers cross the router, and how.

use std::collections::BTreeMap;

use http::header::{
    ACCEPT_ENCODING, AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap,
    HeaderName, HeaderValue, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};

use super::Backend;
use crate::config::view::REDACTED;
use crate::config::{BackendKind, CredentialHeader, X_API_KEY};

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

/// One request header as a report shows it, with where its value came from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SentHeader {
    pub name: String,
    pub value: String,
    pub source: HeaderSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderSource {
    /// Came with the client's request.
    Default,
    /// The backend's `headers` or `anthropic_beta`.
    Backend,
    Credential,
    /// Added by the HTTP client as it frames the request. Derived from the
    /// backend URL and the body, not read back off the wire.
    Transport,
}

/// How a value that may be a secret is shown: masked keeps its first and last
/// four characters, which is enough to tell two values apart in a report that
/// stays in memory. Anything written to disk redacts instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretView {
    Masked,
    Redacted,
}

impl SecretView {
    fn show(self, value: &str) -> String {
        match self {
            SecretView::Masked => crate::credential::mask(value),
            SecretView::Redacted => REDACTED.to_owned(),
        }
    }
}

/// Every header the backend receives: what [`upstream_headers`] built, the
/// credential the send adds on top as `(header, value as shown)`, and the two
/// the HTTP client frames the request with. A value the backend forces may be
/// a secret and is shown as `view` says; `body_len` is 0 for a request that
/// carries no body, which is then sent without `content-length`.
pub fn sent_headers(
    backend: &Backend,
    headers: &HeaderMap,
    credential: Option<(&CredentialHeader, String)>,
    body_len: usize,
    view: SecretView,
) -> Vec<SentHeader> {
    let mut sent: BTreeMap<String, (String, HeaderSource)> = BTreeMap::new();
    for name in headers.keys() {
        let value = headers
            .get_all(name)
            .iter()
            .map(header_text)
            .collect::<Vec<_>>()
            .join(", ");
        let (value, source) = if backend.headers.contains_key(name) {
            (view.show(&value), HeaderSource::Backend)
        } else if *name == ANTHROPIC_BETA && !backend.anthropic_beta.is_empty() {
            (value, HeaderSource::Backend)
        } else {
            (value, HeaderSource::Default)
        };
        sent.insert(name.to_string(), (value, source));
    }
    if let Some((header, value)) = credential {
        sent.insert(header.name.to_string(), (value, HeaderSource::Credential));
    }
    if let Some(host) = host(&backend.url) {
        sent.insert(HOST.to_string(), (host, HeaderSource::Transport));
    }
    if body_len > 0 {
        sent.insert(
            CONTENT_LENGTH.to_string(),
            (body_len.to_string(), HeaderSource::Transport),
        );
    }
    sent.into_iter()
        .map(|(name, (value, source))| SentHeader {
            name,
            value,
            source,
        })
        .collect()
}

/// Authority of `url`, as the HTTP client sends it: the default port for the
/// scheme is left off.
fn host(url: &str) -> Option<String> {
    let uri: http::Uri = url.parse().ok()?;
    let authority = uri.authority()?;
    let default = match uri.scheme_str() {
        Some("https") => Some(443),
        _ => Some(80),
    };
    match authority.port_u16() {
        Some(port) if Some(port) != default => Some(format!("{}:{port}", authority.host())),
        _ => Some(authority.host().to_owned()),
    }
}

pub fn header_text(value: &HeaderValue) -> &str {
    value.to_str().unwrap_or("<binary>")
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
