//! Credential produced by a shell command, cached and re-run periodically.

use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::{Credential, CredentialError, CredentialSource};
use crate::config::CredentialHeader;

#[derive(Debug)]
pub struct CommandCredential {
    command: String,
    header: CredentialHeader,
    refresh: Duration,
    timeout: Duration,
    /// Held across the whole fetch so concurrent requests share one run.
    cache: Mutex<Option<Cached>>,
}

#[derive(Debug)]
struct Cached {
    credential: Credential,
    fetched_at: Instant,
}

impl CommandCredential {
    pub fn new(
        command: String,
        header: CredentialHeader,
        refresh: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            command,
            header,
            refresh,
            timeout,
            cache: Mutex::new(None),
        }
    }

    async fn run(&self) -> Result<Credential, CredentialError> {
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
        let secret = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if secret.is_empty() {
            return Err(CredentialError::Empty);
        }
        Credential::new(self.header, secret)
    }
}

#[async_trait]
impl CredentialSource for CommandCredential {
    async fn credential(&self) -> Result<Option<Credential>, CredentialError> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.fetched_at.elapsed() < self.refresh
        {
            return Ok(Some(cached.credential.clone()));
        }
        let credential = self.run().await.inspect_err(|e| {
            tracing::warn!(error = %e, "credential command failed");
        })?;
        tracing::info!(credential = %credential.masked(), "credential refreshed");
        *cache = Some(Cached {
            credential: credential.clone(),
            fetched_at: Instant::now(),
        });
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
            "command `{}` (refresh {}, timeout {})",
            self.command,
            humantime::format_duration(self.refresh),
            humantime::format_duration(self.timeout)
        )
    }
}
