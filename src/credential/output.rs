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

/// 9999-12-31T23:59:59Z. A timestamp past it cannot be printed as RFC 3339,
/// so it is refused rather than carried into a log line.
const MAX_EPOCH_SECONDS: f64 = 253_402_300_799.0;

#[derive(Deserialize)]
#[serde(untagged)]
enum ExpiresAt {
    /// A number, since a token store may write the epoch with a fraction.
    Unix(f64),
    Rfc3339(String),
}

impl ExpiresAt {
    fn resolve(self) -> Result<SystemTime, CredentialError> {
        match self {
            ExpiresAt::Unix(n) => {
                // 10^11 seconds is year 5138; anything larger can only be milliseconds.
                let seconds = if n >= 100_000_000_000.0 {
                    n / 1000.0
                } else {
                    n
                };
                if !(0.0..=MAX_EPOCH_SECONDS).contains(&seconds) {
                    return Err(CredentialError::Json(format!(
                        "expires_at: {n} is not a usable unix timestamp"
                    )));
                }
                Ok(UNIX_EPOCH + Duration::from_secs_f64(seconds))
            }
            ExpiresAt::Rfc3339(text) => humantime::parse_rfc3339(&text)
                .map_err(|e| CredentialError::Json(format!("expires_at: {e}"))),
        }
    }
}
