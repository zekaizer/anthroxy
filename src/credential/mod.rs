//! Backend credentials: where they come from and how they are presented
//! (ADR-0004).

mod command;
mod fixed;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use http::header::{AUTHORIZATION, HeaderName, HeaderValue};

use crate::config::{CredentialConfig, CredentialHeader};

pub use command::CommandCredential;
pub use fixed::FixedCredential;

pub static X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");

/// A secret plus the header it is sent in. `Debug` never prints the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    header: CredentialHeader,
    secret: String,
}

impl Credential {
    pub fn new(header: CredentialHeader, secret: impl Into<String>) -> Self {
        Self {
            header,
            secret: secret.into(),
        }
    }

    /// Header to set on the upstream request. The value is marked sensitive so
    /// HTTP-layer logging redacts it.
    pub fn header_pair(&self) -> (HeaderName, HeaderValue) {
        let mut value = match self.header {
            CredentialHeader::Bearer => HeaderValue::from_str(&format!("Bearer {}", self.secret)),
            CredentialHeader::XApiKey => HeaderValue::from_str(&self.secret),
        }
        .expect("credential validated as header-safe");
        value.set_sensitive(true);
        let name = match self.header {
            CredentialHeader::Bearer => AUTHORIZATION,
            CredentialHeader::XApiKey => X_API_KEY.clone(),
        };
        (name, value)
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
    #[error("credential command exited with {status}{}", stderr_suffix(.stderr))]
    Failed { status: String, stderr: String },
    #[error("credential command printed nothing on stdout")]
    Empty,
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

    /// One-line human description for status output.
    fn describe(&self) -> String;
}

/// Builds the source for a backend. Static inputs (environment) are resolved
/// here so misconfiguration surfaces at startup rather than on first request.
pub fn build(config: &CredentialConfig) -> Result<Arc<dyn CredentialSource>, CredentialError> {
    Ok(match config {
        CredentialConfig::None => Arc::new(FixedCredential::none()),
        CredentialConfig::Static { value, header } => {
            Arc::new(FixedCredential::secret(*header, value.clone())?)
        }
        CredentialConfig::Env { name, header } => {
            Arc::new(FixedCredential::from_env(*header, name)?)
        }
        CredentialConfig::Command {
            command,
            refresh,
            timeout,
            header,
        } => Arc::new(CommandCredential::new(
            command.clone(),
            *header,
            *refresh,
            *timeout,
        )),
    })
}

fn check_header_safe(secret: &str) -> Result<(), CredentialError> {
    HeaderValue::from_str(secret)
        .map(|_| ())
        .map_err(|_| CredentialError::NotHeaderSafe)
}
