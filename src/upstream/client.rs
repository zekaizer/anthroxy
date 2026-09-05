//! Sends one client request to a backend, with credential refresh and retry.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, Method, StatusCode};

use super::{Backend, Decision, RetryPolicy, upstream_headers};
use crate::config::UpstreamConfig;
use crate::credential::CredentialError;

#[derive(Debug)]
pub struct UpstreamClient {
    http: reqwest::Client,
    retry: RetryPolicy,
}

pub struct UpstreamRequest<'a> {
    pub backend: &'a Arc<Backend>,
    pub method: Method,
    /// Path and query exactly as the client sent them, e.g. `/v1/messages`.
    pub path_and_query: &'a str,
    pub headers: &'a HeaderMap,
    pub body: Bytes,
}

pub struct UpstreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    /// Body still to be read; nothing has been consumed.
    pub body: reqwest::Response,
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
}

fn describe(error: &reqwest::Error) -> String {
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
    pub fn from_config(config: &UpstreamConfig) -> reqwest::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            retry: RetryPolicy::from_config(config),
        })
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
        let mut credential_refreshed = false;
        loop {
            attempt += 1;
            let credential = backend.credential.credential().await.map_err(|source| {
                UpstreamError::Credential {
                    backend: backend.name.clone(),
                    source,
                }
            })?;
            let headers = upstream_headers(request.headers, backend, credential.as_ref());
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
                    match self.retry.on_status(attempt, status) {
                        Decision::Retry(delay) => {
                            tracing::warn!(attempt, %status, delay_ms = delay.as_millis(), "retrying on upstream status");
                            tokio::time::sleep(delay).await;
                        }
                        Decision::GiveUp => {
                            return Ok(UpstreamResponse {
                                status,
                                headers: response.headers().clone(),
                                body: response,
                                attempts: attempt,
                                latency: started.elapsed(),
                            });
                        }
                    }
                }
                Err(error) => match self.retry.on_transport_error(attempt, &error) {
                    Decision::Retry(delay) => {
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
