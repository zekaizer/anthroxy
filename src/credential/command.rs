//! Credential produced by a shell command, cached until its refresh interval
//! or reported expiry nears.

use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::{Credential, CredentialError, CredentialSource, exec};
use crate::config::{CommandOutput, CredentialHeader};

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

    /// Runs the command once.
    async fn fetch(&self) -> Result<Cached, CredentialError> {
        let run = exec::run(&self.command, self.timeout).await?;
        let output = exec::interpret(&run, self.output)?;
        let valid_for = exec::valid_for(self.refresh, output.expires_at)?;
        Ok(Cached {
            credential: Credential::new(self.header.clone(), output.secret)?,
            valid_until: Instant::now() + valid_for,
            expires_at: output.expires_at,
        })
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
