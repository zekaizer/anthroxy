//! When to try an upstream request again. Only failures that happened before
//! any response byte reached the client are candidates.

use std::time::Duration;

use http::StatusCode;

use crate::config::UpstreamConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts after the first one.
    pub max_retries: u32,
    /// Delay before the first retry; doubles per retry.
    pub backoff: Duration,
    pub retry_on_status: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Wait this long, then try again.
    Retry(Duration),
    GiveUp,
}

impl RetryPolicy {
    pub fn from_config(config: &UpstreamConfig) -> Self {
        Self {
            max_retries: config.retries,
            backoff: config.retry_backoff,
            retry_on_status: config.retry_on_status.clone(),
        }
    }

    pub const fn never() -> Self {
        Self {
            max_retries: 0,
            backoff: Duration::ZERO,
            retry_on_status: Vec::new(),
        }
    }

    /// `attempt` is 1-based: the attempt that just failed.
    pub fn on_transport_error(&self, attempt: u32, error: &reqwest::Error) -> Decision {
        if is_connection_failure(error) {
            self.next(attempt)
        } else {
            Decision::GiveUp
        }
    }

    /// `attempt` is 1-based: the attempt that produced `status`.
    pub fn on_status(&self, attempt: u32, status: StatusCode) -> Decision {
        if self.retry_on_status.contains(&status.as_u16()) {
            self.next(attempt)
        } else {
            Decision::GiveUp
        }
    }

    fn next(&self, attempt: u32) -> Decision {
        if attempt > self.max_retries {
            return Decision::GiveUp;
        }
        let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
        Decision::Retry(self.backoff.saturating_mul(factor))
    }
}

/// True when the request never reached a state where the backend could have
/// started answering: connect refused/reset, or the connection died before a
/// response line arrived. Timeouts are excluded because the backend may be
/// working on the request.
pub fn is_connection_failure(error: &reqwest::Error) -> bool {
    !error.is_timeout()
        && !error.is_body()
        && !error.is_decode()
        && (error.is_connect() || error.is_request())
}
