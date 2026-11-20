//! Sends one client request to a backend, with credential refresh and retry.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::Stream;
use http::{HeaderMap, Method, StatusCode};

use super::header_command::{Attempt, HeaderCommandError};
use super::{
    Backend, BodyClock, BodyError, ClientBuildError, Decision, HttpClients, RetryPolicy, Target,
    TimedBody, TimeoutClock, Timeouts, Transport,
};
use crate::config::{BackendConfig, UpstreamConfig};
use crate::credential::CredentialError;

#[derive(Debug)]
pub struct UpstreamClient {
    clients: HttpClients,
    retry: RetryPolicy,
    timeouts: Timeouts,
}

pub struct UpstreamRequest<'a> {
    /// The backend and the header rules of the route this request travels:
    /// [`crate::routing::Route::target`] for a chat request,
    /// [`Backend::target`] for a request no route makes.
    pub target: Target<'a>,
    pub method: Method,
    /// Path and query exactly as the client sent them, e.g. `/v1/messages`.
    pub path_and_query: &'a str,
    /// Built by [`super::upstream_headers`] from the same `target`; the
    /// credential is added per attempt.
    pub headers: HeaderMap,
    pub body: Bytes,
    /// Whether the client asked for an event stream. Chooses the stream clocks.
    pub stream: bool,
    /// What a backend's `header_command` is told about the request.
    pub origin: Origin<'a>,
}

/// The request a send is made for, as a backend's `header_command` is told
/// it (ADR-0021).
#[derive(Debug, Clone, Copy, Default)]
pub struct Origin<'a> {
    pub request_id: Option<&'a str>,
    /// The upstream model the request names.
    pub model: Option<&'a str>,
    /// The client's headers as they arrived; `None` for a request the router
    /// makes on its own.
    pub client: Option<&'a HeaderMap>,
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
    /// The backend's `header_command` produced no headers; nothing was sent.
    #[error("backend `{backend}`: header command failed: {source}")]
    HeaderCommand {
        backend: String,
        #[source]
        source: HeaderCommandError,
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

impl UpstreamClient {
    pub fn from_config(
        config: &UpstreamConfig,
        backends: &BTreeMap<String, BackendConfig>,
    ) -> Result<Self, ClientBuildError> {
        Ok(Self {
            clients: HttpClients::build(config, backends, None, Transport::ALL)?,
            retry: RetryPolicy::from_config(config),
            timeouts: Timeouts::from_config(config),
        })
    }

    pub fn new(clients: HttpClients, retry: RetryPolicy) -> Self {
        Self {
            clients,
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
        let backend = request.target.backend();
        let http = self.clients.get(request.target.headers().transport);
        let url = format!("{}{}", backend.url, request.path_and_query);
        // Logged instead of `url`, whose userinfo reqwest sends as basic auth.
        let shown = shown_target(backend, request.path_and_query);
        let started = Instant::now();
        let mut attempt: u32 = 0;
        // Of the retry budget; the credential re-send below is not one of them.
        let mut failures: u32 = 0;
        let mut credential_refreshed = false;
        let (target, body_sha256) = match backend.header_command {
            Some(_) => (without_userinfo(&url), sha256_hex(&request.body)),
            None => Default::default(),
        };
        loop {
            attempt += 1;
            let credential = backend.credential.credential().await.map_err(|source| {
                UpstreamError::Credential {
                    backend: backend.name.clone(),
                    source,
                }
            })?;
            let mut headers = request.headers.clone();
            if let Some(command) = &backend.header_command {
                let attempt = Attempt {
                    backend: &backend.name,
                    method: &request.method,
                    url: &target,
                    path: request.path_and_query,
                    model: request.origin.model,
                    request_id: request.origin.request_id,
                    number: attempt,
                    body_sha256: &body_sha256,
                    client: request.origin.client,
                };
                let computed = command.headers(&attempt).await.map_err(|source| {
                    UpstreamError::HeaderCommand {
                        backend: backend.name.clone(),
                        source,
                    }
                })?;
                for (name, value) in computed {
                    headers.insert(name, value);
                }
            }
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
            let send = http
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

/// `url` as the request target: userinfo travels as basic auth, not in it.
fn without_userinfo(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.into()
        }
        // The send fails on the same URL.
        Err(_) => url.to_owned(),
    }
}

fn sha256_hex(body: &[u8]) -> String {
    use std::fmt::Write;
    ring::digest::digest(&ring::digest::SHA256, body)
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
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
