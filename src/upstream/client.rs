//! Sends one client request to a backend, with credential refresh and retry.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::Stream;
use http::{HeaderMap, Method, StatusCode};

use super::{
    Backend, BodyClock, BodyError, Decision, RetryPolicy, TimedBody, TimeoutClock, Timeouts,
};
use crate::config::{BackendConfig, UpstreamConfig, origin};
use crate::credential::CredentialError;

#[derive(Debug)]
pub struct UpstreamClient {
    http: reqwest::Client,
    retry: RetryPolicy,
    timeouts: Timeouts,
}

pub struct UpstreamRequest<'a> {
    pub backend: &'a Backend,
    pub method: Method,
    /// Path and query exactly as the client sent them, e.g. `/v1/messages`.
    pub path_and_query: &'a str,
    /// Already translated by [`super::upstream_headers`]; the credential is
    /// added per attempt.
    pub headers: HeaderMap,
    pub body: Bytes,
    /// Whether the client asked for an event stream. Chooses the stream clocks.
    pub stream: bool,
}

/// Largest body [`UpstreamResponse::body_bytes`] reads whole.
pub const MAX_WHOLE_BODY_BYTES: usize = 16 * 1024 * 1024;

pub struct UpstreamResponse {
    /// Headers arrived; nothing of the body has been consumed.
    pub response: reqwest::Response,
    /// Attempts made, including the successful one.
    pub attempts: u32,
    /// The backend rejected the credential, which was re-acquired and the
    /// request sent again; that send is one of `attempts`.
    pub credential_refreshed: bool,
    /// From first attempt to response headers.
    pub latency: Duration,
    body: BodyClock,
}

#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("backend `{backend}`: could not obtain credential: {source}")]
    Credential {
        backend: String,
        #[source]
        source: CredentialError,
    },
    #[error("backend `{backend}` unreachable after {attempts} attempt(s): {}", describe(.source))]
    Transport {
        backend: String,
        attempts: u32,
        #[source]
        source: reqwest::Error,
    },
    /// The backend redirected. Following it is refused for the router itself,
    /// and relaying it would only move the decision to a client that follows
    /// redirects with the router's own token.
    #[error(
        "backend `{backend}` answered HTTP {status} redirecting to `{location}`; point its `url` there instead"
    )]
    Redirected {
        backend: String,
        status: u16,
        location: String,
    },
    /// The backend answered but its error body broke off before the end.
    #[error("backend `{backend}` failed while sending its error body: {}", describe(.source))]
    Body {
        backend: String,
        #[source]
        source: reqwest::Error,
    },
    /// A body the router reads whole went past its limit.
    #[error(
        "backend `{backend}` sent a response body over {limit} bytes, more than the router reads whole"
    )]
    BodyTooLarge { backend: String, limit: usize },
    /// One of the configured upstream clocks ran out.
    #[error("backend `{backend}`: upstream.{clock} elapsed")]
    TimedOut {
        backend: String,
        clock: TimeoutClock,
    },
}

/// The error with its full source chain, e.g. `error sending request: ... : Connection refused`.
pub fn describe(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("cannot read upstream.ca_certificate {}: {source}", path.display())]
    ReadCaCertificate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("upstream.ca_certificate {}: {source}", path.display())]
    ParseCaCertificate {
        path: PathBuf,
        #[source]
        source: reqwest::Error,
    },
    #[error("upstream.ca_certificate {} holds no certificate", path.display())]
    NoCaCertificate { path: PathBuf },
    #[error("cannot build HTTP client: {0}")]
    Http(#[from] reqwest::Error),
}

/// How every backend is reached: the configured timeouts, the extra trust
/// anchors from `ca_certificate`, no redirects, since a redirect would resend
/// the body and the credential elsewhere, and no proxy but the one a backend
/// names (ADR-0012), since `http_proxy` in the environment would do the same
/// to every backend at once. Assumes `backends` passed validation.
pub fn http_client(
    config: &UpstreamConfig,
    backends: &BTreeMap<String, BackendConfig>,
) -> Result<reqwest::ClientBuilder, ClientBuildError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(config.connect_timeout)
        .tcp_keepalive(Duration::from_secs(60))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    let proxies = proxies(backends);
    if !proxies.is_empty() {
        builder = builder.proxy(reqwest::Proxy::custom(move |url| {
            origin(url).and_then(|origin| proxies.get(&origin).cloned())
        }));
    }
    if let Some(path) = &config.ca_certificate {
        for certificate in ca_certificates(path)? {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}

/// Proxy by backend origin. A backend URL reqwest cannot parse is never
/// requested, so it needs no entry.
fn proxies(backends: &BTreeMap<String, BackendConfig>) -> HashMap<String, reqwest::Url> {
    backends
        .values()
        .filter_map(|backend| {
            let proxy = backend.proxy.as_deref()?;
            let origin = origin(&reqwest::Url::parse(&backend.url).ok()?)?;
            let proxy = reqwest::Url::parse(proxy).expect("validated proxy url");
            Some((origin, proxy))
        })
        .collect()
}

/// Every certificate in the PEM bundle at `path`; an empty bundle is an
/// error because it silently trusts nothing.
fn ca_certificates(path: &Path) -> Result<Vec<reqwest::Certificate>, ClientBuildError> {
    let pem = std::fs::read(path).map_err(|source| ClientBuildError::ReadCaCertificate {
        path: path.to_path_buf(),
        source,
    })?;
    let certificates = reqwest::Certificate::from_pem_bundle(&pem).map_err(|source| {
        ClientBuildError::ParseCaCertificate {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if certificates.is_empty() {
        return Err(ClientBuildError::NoCaCertificate {
            path: path.to_path_buf(),
        });
    }
    Ok(certificates)
}

impl UpstreamClient {
    pub fn from_config(
        config: &UpstreamConfig,
        backends: &BTreeMap<String, BackendConfig>,
    ) -> Result<Self, ClientBuildError> {
        Ok(Self {
            http: http_client(config, backends)?.build()?,
            retry: RetryPolicy::from_config(config),
            timeouts: Timeouts::from_config(config),
        })
    }

    pub fn new(http: reqwest::Client, retry: RetryPolicy) -> Self {
        Self {
            http,
            retry,
            timeouts: Timeouts::default(),
        }
    }

    /// Forwards `request`, returning as soon as response headers arrive.
    ///
    /// Retries connection failures and configured statuses with backoff. A
    /// 401/403 from a refreshable credential triggers one re-acquire and
    /// re-send that is not counted as a retry.
    pub async fn send(
        &self,
        request: UpstreamRequest<'_>,
    ) -> Result<UpstreamResponse, UpstreamError> {
        let backend = request.backend;
        let url = format!("{}{}", backend.url, request.path_and_query);
        // Logged instead of `url`, whose userinfo reqwest sends as basic auth.
        let shown = shown_target(backend, request.path_and_query);
        let started = Instant::now();
        let mut attempt: u32 = 0;
        // Of the retry budget; the credential re-send below is not one of them.
        let mut failures: u32 = 0;
        let mut credential_refreshed = false;
        loop {
            attempt += 1;
            let credential = backend.credential.credential().await.map_err(|source| {
                UpstreamError::Credential {
                    backend: backend.name.clone(),
                    source,
                }
            })?;
            let mut headers = request.headers.clone();
            if let Some(credential) = &credential {
                let (name, value) = credential.header_pair();
                headers.insert(name, value);
            }
            tracing::debug!(attempt, url = %shown, "sending upstream request");
            let attempt_start = tokio::time::Instant::now();
            let header_deadline = attempt_start
                + if request.stream {
                    self.timeouts.stream_first_byte
                } else {
                    self.timeouts.non_stream
                };
            let clock = if request.stream {
                TimeoutClock::StreamFirstByte
            } else {
                TimeoutClock::NonStream
            };
            let send = self
                .http
                .request(request.method.clone(), &url)
                .headers(headers)
                .body(request.body.clone())
                .send();
            let outcome = match tokio::time::timeout_at(header_deadline, send).await {
                Ok(outcome) => outcome,
                Err(_) => {
                    return Err(UpstreamError::TimedOut {
                        backend: backend.name.clone(),
                        clock,
                    });
                }
            };
            match outcome {
                Ok(response) => {
                    let status = response.status();
                    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                        && !credential_refreshed
                        && backend.credential.is_refreshable()
                        && let Some(rejected) = &credential
                    {
                        tracing::warn!(%status, "backend rejected credential; re-acquiring and retrying once");
                        backend.credential.invalidate(rejected).await;
                        credential_refreshed = true;
                        continue;
                    }
                    match self.retry.on_status(failures + 1, status) {
                        Decision::Retry(delay) => {
                            failures += 1;
                            tracing::warn!(attempt, %status, delay_ms = delay.as_millis(), "retrying on upstream status");
                            tokio::time::sleep(delay).await;
                        }
                        Decision::GiveUp => {
                            return Ok(UpstreamResponse {
                                response,
                                attempts: attempt,
                                credential_refreshed,
                                latency: started.elapsed(),
                                body: BodyClock {
                                    stream: request.stream,
                                    deadline: header_deadline,
                                    idle: self.timeouts.stream_idle,
                                },
                            });
                        }
                    }
                }
                Err(error) => match self.retry.on_transport_error(failures + 1, &error) {
                    Decision::Retry(delay) => {
                        failures += 1;
                        tracing::warn!(attempt, error = %describe(&error), delay_ms = delay.as_millis(), "retrying after connection failure");
                        tokio::time::sleep(delay).await;
                    }
                    Decision::GiveUp => {
                        return Err(UpstreamError::Transport {
                            backend: backend.name.clone(),
                            attempts: attempt,
                            source: error,
                        });
                    }
                },
            }
        }
    }
}

/// The request target as a log line may carry it.
pub(super) fn shown_target(backend: &Backend, path_and_query: &str) -> String {
    format!("{}{path_and_query}", backend.shown_url())
}

impl UpstreamResponse {
    pub fn bytes_stream(self) -> TimedBody<impl Stream<Item = Result<Bytes, reqwest::Error>>> {
        TimedBody::new(self.response.bytes_stream(), self.body)
    }

    /// The whole body, up to [`MAX_WHOLE_BODY_BYTES`]; a model list or a
    /// probe answer past that is a failure, not a buffer.
    pub async fn body_bytes(self) -> Result<Bytes, BodyError> {
        use futures_util::StreamExt;
        let mut stream = self.bytes_stream();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if out.len() + chunk.len() > MAX_WHOLE_BODY_BYTES {
                return Err(BodyError::TooLarge {
                    limit: MAX_WHOLE_BODY_BYTES,
                });
            }
            out.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(out))
    }
}
