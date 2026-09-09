//! Backend credentials: where they come from and how they are presented
//! (ADR-0004).

mod command;
pub mod exec;
mod fixed;
mod output;

#[cfg(test)]
mod tests;

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

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("environment variable `{0}` is not set")]
    MissingEnv(String),
    #[error("credential contains characters not allowed in an HTTP header")]
    NotHeaderSafe,
    #[error("cannot start credential command: {0}")]
    Spawn(#[source] std::io::Error),
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

fn stderr_suffix(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(": {trimmed}")
    }
}

/// Source of a backend's credential.
#[async_trait]
pub trait CredentialSource: Send + Sync + std::fmt::Debug {
    /// Current credential; `None` means the backend takes no auth header.
    async fn credential(&self) -> Result<Option<Credential>, CredentialError>;

    /// Forgets any cached value so the next call re-acquires it.
    async fn invalidate(&self);

    /// Whether `invalidate` can yield a different credential. Callers skip the
    /// 401-refresh cycle when it cannot.
    fn is_refreshable(&self) -> bool {
        false
    }

    /// Where the credential comes from, for status output. Never the value.
    fn describe(&self) -> String;
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
