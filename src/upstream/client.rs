//! Sends one client request to a backend, with credential refresh and retry.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};

use super::{Backend, Decision, RetryPolicy};
use crate::config::UpstreamConfig;
use crate::credential::CredentialError;

#[derive(Debug)]
pub struct UpstreamClient {
    http: reqwest::Client,
    retry: RetryPolicy,
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
}

pub struct UpstreamResponse {
    /// Headers arrived; nothing of the body has been consumed.
    pub response: reqwest::Response,
    /// Attempts made, including the successful one.
    pub attempts: u32,
    /// From first attempt to response headers.
    pub latency: Duration,
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
/// the body and the credential elsewhere, and no proxy, since `http_proxy` in
/// the environment would do the same to every backend at once — a backend the
/// operator named by URL is reached at that URL.
pub fn http_client(config: &UpstreamConfig) -> Result<reqwest::ClientBuilder, ClientBuildError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(config.connect_timeout)
        .read_timeout(config.read_timeout)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    if let Some(path) = &config.ca_certificate {
        for certificate in ca_certificates(path)? {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
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
    pub fn from_config(config: &UpstreamConfig) -> Result<Self, ClientBuildError> {
        Ok(Self::new(
            http_client(config)?.build()?,
            RetryPolicy::from_config(config),
        ))
    }

    pub fn new(http: reqwest::Client, retry: RetryPolicy) -> Self {
        Self { http, retry }
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
            tracing::debug!(attempt, %url, "sending upstream request");
            let outcome = self
                .http
                .request(request.method.clone(), &url)
                .headers(headers)
                .body(request.body.clone())
                .send()
                .await;
            match outcome {
                Ok(response) => {
                    let status = response.status();
                    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                        && !credential_refreshed
                        && backend.credential.is_refreshable()
                    {
                        tracing::warn!(%status, "backend rejected credential; re-acquiring and retrying once");
                        backend.credential.invalidate().await;
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
                                latency: started.elapsed(),
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
