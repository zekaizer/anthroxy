//! Credential produced by a shell command, cached until its refresh interval
//! or reported expiry nears.

use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::{Credential, CredentialError, CredentialSource, output};
use crate::config::{CommandOutput, CredentialHeader};

/// A credential this close to its reported expiry is re-acquired instead of
/// served from cache.
pub const EXPIRY_MARGIN: Duration = Duration::from_secs(120);

#[derive(Debug)]
pub struct CommandCredential {
    command: String,
    output: CommandOutput,
    header: CredentialHeader,
    refresh: Duration,
    timeout: Duration,
    /// Held across the whole fetch so concurrent requests share one run.
    cache: Mutex<Option<Cached>>,
}

#[derive(Debug)]
struct Cached {
    credential: Credential,
    /// Served from cache until then; the command is re-run after.
    valid_until: Instant,
    /// Reported by the command, for the refresh log line.
    expires_at: Option<SystemTime>,
}

impl CommandCredential {
    pub fn new(
        command: String,
        output: CommandOutput,
        header: CredentialHeader,
        refresh: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            command,
            output,
            header,
            refresh,
            timeout,
            cache: Mutex::new(None),
        }
    }

    /// Runs the command once. A reported expiry that has already passed is
    /// [`CredentialError::Expired`] rather than a credential the backend will
    /// reject.
    async fn fetch(&self) -> Result<Cached, CredentialError> {
        let output = self.run().await?;
        let credential = Credential::new(self.header.clone(), output.secret)?;
        let now = Instant::now();
        let mut valid_until = now + self.refresh;
        if let Some(expires_at) = output.expires_at {
            let remaining = expires_at
                .duration_since(SystemTime::now())
                .map_err(|_| CredentialError::Expired(expires_at))?;
            valid_until = valid_until.min(now + remaining.saturating_sub(EXPIRY_MARGIN));
        }
        Ok(Cached {
            credential,
            valid_until,
            expires_at: output.expires_at,
        })
    }

    async fn run(&self) -> Result<output::Output, CredentialError> {
        tracing::debug!(command = %self.command, "running credential command");
        let child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(CredentialError::Spawn)?;
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| CredentialError::Timeout(self.timeout))?
            .map_err(CredentialError::Spawn)?;
        if !output.status.success() {
            return Err(CredentialError::Failed {
                status: output
                    .status
                    .code()
                    .map(|c| format!("status {c}"))
                    .unwrap_or_else(|| output.status.to_string()),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        output::parse(self.output, &String::from_utf8_lossy(&output.stdout))
    }
}

#[async_trait]
impl CredentialSource for CommandCredential {
    async fn credential(&self) -> Result<Option<Credential>, CredentialError> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && Instant::now() < cached.valid_until
        {
            return Ok(Some(cached.credential.clone()));
        }
        let fetched = self.fetch().await.inspect_err(|e| {
            tracing::warn!(error = %e, "credential command failed");
        })?;
        let expires_at = fetched
            .expires_at
            .map(|t| humantime::format_rfc3339_seconds(t).to_string());
        tracing::info!(
            credential = %fetched.credential.masked(),
            expires_at = expires_at.as_deref(),
            "credential refreshed"
        );
        let credential = fetched.credential.clone();
        *cache = Some(fetched);
        Ok(Some(credential))
    }

    async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }

    fn is_refreshable(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        format!(
            "command `{}` ({}refresh {}, timeout {})",
            self.command,
            match self.output {
                CommandOutput::Text => "",
                CommandOutput::Json => "json, ",
            },
            humantime::format_duration(self.refresh),
            humantime::format_duration(self.timeout)
        )
    }
}
