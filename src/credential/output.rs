//! Reading a credential command's stdout.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::CredentialError;
use crate::config::CommandOutput;

/// What one run of the command produced.
pub struct Output {
    pub secret: String,
    /// Reported expiry, when the output carries one.
    pub expires_at: Option<SystemTime>,
}

/// Interprets `stdout` as `mode` says. An empty secret is [`CredentialError::Empty`].
pub fn parse(mode: CommandOutput, stdout: &str) -> Result<Output, CredentialError> {
    let output = match mode {
        CommandOutput::Text => Output {
            secret: stdout.trim().to_owned(),
            expires_at: None,
        },
        CommandOutput::Json => {
            let json: JsonOutput =
                serde_json::from_str(stdout).map_err(|e| CredentialError::Json(e.to_string()))?;
            Output {
                secret: json.token,
                expires_at: json.expires_at.map(ExpiresAt::resolve).transpose()?,
            }
        }
    };
    if output.secret.is_empty() {
        return Err(CredentialError::Empty);
    }
    Ok(output)
}

#[derive(Deserialize)]
struct JsonOutput {
    token: String,
    #[serde(default)]
    expires_at: Option<ExpiresAt>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ExpiresAt {
    Unix(u64),
    Rfc3339(String),
}

impl ExpiresAt {
    fn resolve(self) -> Result<SystemTime, CredentialError> {
        match self {
            // 10^11 seconds is year 5138; anything larger can only be milliseconds.
            ExpiresAt::Unix(n) if n >= 100_000_000_000 => Ok(UNIX_EPOCH + Duration::from_millis(n)),
            ExpiresAt::Unix(n) => Ok(UNIX_EPOCH + Duration::from_secs(n)),
            ExpiresAt::Rfc3339(text) => humantime::parse_rfc3339(&text)
                .map_err(|e| CredentialError::Json(format!("expires_at: {e}"))),
        }
    }
}
