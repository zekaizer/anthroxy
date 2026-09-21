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
static PROXY_AUTHENTICATE: HeaderName = HeaderName::from_static("proxy-authenticate");
static PROXY_AUTHORIZATION: HeaderName = HeaderName::from_static("proxy-authorization");

/// A backend name or model id as a header value. Validation guarantees they
/// are header-safe.
pub fn header_value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).expect("validated header-safe")
}

/// Hop-by-hop headers belong to one connection and are never relayed. The
/// proxy pair is the client's business with its own proxy, not a backend's,
/// and the backend's is not the client's.
fn is_hop_by_hop(name: &HeaderName) -> bool {
    *name == CONNECTION
        || *name == KEEP_ALIVE
        || *name == PROXY_CONNECTION
        || *name == PROXY_AUTHENTICATE
        || *name == PROXY_AUTHORIZATION
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
/// the backend credential), `accept-encoding` (bodies are relayed and logged
/// uncompressed) and whatever the backend's `drop_headers` names. Backend
/// `headers` override, `anthropic_beta` flags are merged into the client's
/// list. An `openai` backend gets no `anthropic-version` or `anthropic-beta`
/// at all (ADR-0010).
pub fn upstream_headers(client: &HeaderMap, backend: &Backend) -> HeaderMap {
    let mut out = HeaderMap::with_capacity(client.len() + backend.headers.len() + 1);
    for (name, value) in client {
        if dropped(name, backend).is_some() {
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

/// Why a header the client sent does not reach the backend. A backend's
/// configuration decides [`DropReason::DropHeaders`], [`DropReason::Overridden`]
/// and, through its `kind`, [`DropReason::BackendKind`]; the rest follow from
/// what the router is and happen to every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// Hop-by-hop, `host` or `content-length`: the HTTP client owns them.
    Framing,
    /// The client's own `authorization`/`x-api-key`, replaced by the backend
    /// credential.
    ClientCredential,
    /// `accept-encoding`: bodies are relayed and logged uncompressed.
    Uncompressed,
    /// `anthropic-version`/`anthropic-beta` on an `openai` backend (ADR-0010).
    BackendKind,
    /// The backend's `drop_headers`.
    DropHeaders,
    /// The backend's `headers` set this name, which replaces every value the
    /// client sent under it.
    Overridden,
}

/// Whether the client's `name` reaches `backend`, and why not.
fn dropped(name: &HeaderName, backend: &Backend) -> Option<DropReason> {
    if is_hop_by_hop(name) || *name == HOST || *name == CONTENT_LENGTH {
        return Some(DropReason::Framing);
    }
    if (*name == AUTHORIZATION || *name == X_API_KEY) && !backend.forwards_client_auth {
        return Some(DropReason::ClientCredential);
    }
    if *name == ACCEPT_ENCODING {
        return Some(DropReason::Uncompressed);
    }
    if backend.kind == BackendKind::OpenAi
        && (*name == ANTHROPIC_VERSION || *name == ANTHROPIC_BETA)
    {
        return Some(DropReason::BackendKind);
    }
    backend
        .drop_headers
        .matches(name)
        .then_some(DropReason::DropHeaders)
}

/// One header the client sent that the backend never sees.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DroppedHeader {
    pub name: String,
    pub value: String,
    pub reason: DropReason,
}

/// Every value the client sent that the backend does not see: what
/// [`upstream_headers`] filters out, and what its `headers` overwrite. Values
/// are the client's own and are shown as sent, except its credential, which
/// is the router's token and is of no use to any report.
pub fn dropped_headers(client: &HeaderMap, backend: &Backend) -> Vec<DroppedHeader> {
    let mut out: Vec<DroppedHeader> = Vec::new();
    for (name, value) in client {
        let reason = match dropped(name, backend) {
            Some(reason) => reason,
            // `anthropic_beta` merges the client's flags in rather than
            // replacing them, so a beta header the backend adds to is not one
            // it overrode.
            None if backend.headers.contains_key(name) => DropReason::Overridden,
            None => continue,
        };
        let value = match reason {
            DropReason::ClientCredential => REDACTED.to_owned(),
            _ => header_text(value).to_owned(),
        };
        out.push(DroppedHeader {
            name: name.to_string(),
            value,
            reason,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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
        } else if *name == AUTHORIZATION || *name == X_API_KEY {
            // The client's own credential, forwarded by a passthrough
            // backend: a secret like any other.
            (view.show(&value), HeaderSource::Default)
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
    // A `host` the backend forces is what the HTTP client sends; the URL's
    // authority is only what it frames when none is set.
    if let Some(host) = host(&backend.url) {
        sent.entry(HOST.to_string())
            .or_insert((host, HeaderSource::Transport));
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
