//! Backend credentials: where they come from and how they are presented
//! (ADR-0004).

mod command;
pub mod exec;
mod fixed;
mod output;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use http::header::{HeaderName, HeaderValue};

use crate::config::{CredentialConfig, CredentialHeader};

pub use command::CommandCredential;
pub use fixed::FixedCredential;
pub use output::Output;

/// A secret plus the header it is sent in. `Debug` never prints the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    header: CredentialHeader,
    secret: String,
}

impl Credential {
    /// Fails when `secret` cannot travel in an HTTP header.
    pub fn new(
        header: CredentialHeader,
        secret: impl Into<String>,
    ) -> Result<Self, CredentialError> {
        let secret = secret.into();
        HeaderValue::from_str(&header.value(&secret))
            .map_err(|_| CredentialError::NotHeaderSafe)?;
        Ok(Self { header, secret })
    }

    /// Header to set on the upstream request. The value is marked sensitive so
    /// HTTP-layer logging redacts it.
    pub fn header_pair(&self) -> (HeaderName, HeaderValue) {
        let mut value = HeaderValue::from_str(&self.header.value(&self.secret))
            .expect("checked in Credential::new");
        value.set_sensitive(true);
        (self.header.name.clone(), value)
    }

    /// First and last four characters, enough to tell two tokens apart in logs.
    pub fn masked(&self) -> String {
        mask(&self.secret)
    }

    /// The header value with the secret masked, e.g. `Bearer abcd…wxyz`.
    pub fn masked_value(&self) -> String {
        self.header.value(&self.masked())
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("header", &self.header)
            .field("secret", &self.masked())
            .finish()
    }
}

pub fn mask(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CredentialError {
    #[error("environment variable `{0}` is not set")]
    MissingEnv(String),
    #[error("credential contains characters not allowed in an HTTP header")]
    NotHeaderSafe,
    #[error("cannot start credential command: {0}")]
    Spawn(#[source] Arc<std::io::Error>),
    #[error("credential command exceeded {}", humantime::format_duration(*.0))]
    Timeout(Duration),
    #[error("credential command failed with {status}{}", stderr_suffix(.stderr))]
    Failed { status: String, stderr: String },
    #[error("credential command printed nothing on stdout")]
    Empty,
    #[error("credential command output is not `{{\"token\": ..., \"expires_at\": ...}}` JSON: {0}")]
    Json(String),
    #[error("credential command returned a credential that expired at {}", humantime::format_rfc3339_seconds(*.0))]
    Expired(std::time::SystemTime),
}

/// The command's stderr as an error message may carry it. It is whatever the
/// command wrote — a token server's answer, a shell trace — and this message
/// reaches both a log line and the client, so it travels escaped and cut.
/// `anthroxy credential` prints the run's own stderr in full instead.
fn stderr_suffix(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(": {}", crate::text::cut(trimmed, 200))
    }
}

/// What a source holds right now and how its recent acquisitions went, for
/// status output. Never the value itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CredentialStatus {
    /// [`CredentialSource::describe`].
    pub source: String,
    /// The value a request would carry now, masked; `None` when there is none
    /// or nothing is cached.
    pub masked: Option<String>,
    pub fetched_at: Option<jiff::Timestamp>,
    /// Reported by the command.
    pub expires_at: Option<jiff::Timestamp>,
    /// When the cached value stops being served and the command runs again.
    pub refresh_at: Option<jiff::Timestamp>,
    /// Recent runs, oldest first.
    pub refreshes: Vec<RefreshRecord>,
}

/// One run of a credential command.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RefreshRecord {
    pub at: jiff::Timestamp,
    pub duration_ms: u64,
    /// The value it produced, masked.
    pub masked: Option<String>,
    pub error: Option<String>,
}

/// Source of a backend's credential.
#[async_trait]
pub trait CredentialSource: Send + Sync + std::fmt::Debug {
    /// Current credential; `None` means the backend takes no auth header.
    async fn credential(&self) -> Result<Option<Credential>, CredentialError>;

    /// Forgets the cached value if it is still `rejected`, so the next call
    /// re-acquires it. A value re-acquired since `rejected` was handed out
    /// stays: requests that were rejected together refresh once.
    async fn invalidate(&self, rejected: &Credential);

    /// Whether `invalidate` can yield a different credential. Callers skip the
    /// 401-refresh cycle when it cannot.
    fn is_refreshable(&self) -> bool {
        false
    }

    /// Where the credential comes from, for status output. Never the value.
    fn describe(&self) -> String;

    fn status(&self) -> CredentialStatus;
}

/// Builds the source for a backend. Static inputs (environment) are resolved
/// here so misconfiguration surfaces at startup rather than on first request.
pub fn build(config: &CredentialConfig) -> Result<Box<dyn CredentialSource>, CredentialError> {
    Ok(match config {
        CredentialConfig::None => Box::new(FixedCredential::none()),
        CredentialConfig::Static { value, header } => {
            Box::new(FixedCredential::secret(header.clone(), value.clone())?)
        }
        CredentialConfig::Env { name, header } => {
            Box::new(FixedCredential::from_env(header.clone(), name)?)
        }
        CredentialConfig::Command {
            command,
            output,
            refresh,
            timeout,
            header,
        } => Box::new(CommandCredential::new(
            command.clone(),
            *output,
            header.clone(),
            *refresh,
            *timeout,
        )),
    })
}
