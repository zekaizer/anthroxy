//! Credential produced by a shell command, cached until its refresh interval
//! or reported expiry nears.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::{Credential, CredentialError, CredentialSource, CredentialStatus, RefreshRecord, exec};
use crate::config::{CommandOutput, CredentialHeader};

/// Runs kept for status output.
pub const REFRESH_HISTORY: usize = 20;

#[derive(Debug)]
pub struct CommandCredential {
    command: String,
    output: CommandOutput,
    header: CredentialHeader,
    refresh: Duration,
    timeout: Duration,
    /// Held across the whole fetch so concurrent requests share one run.
    cache: Mutex<Slot>,
    /// Kept apart from `cache` so status output never waits for a run.
    observed: std::sync::Mutex<Observed>,
}

#[derive(Debug, Default)]
struct Observed {
    /// Set while a value is cached.
    current: Option<Current>,
    /// Oldest first, at most [`REFRESH_HISTORY`].
    runs: VecDeque<RefreshRecord>,
}

#[derive(Debug, Clone)]
struct Current {
    masked: String,
    fetched_at: jiff::Timestamp,
    expires_at: Option<jiff::Timestamp>,
    refresh_at: Option<jiff::Timestamp>,
}

#[derive(Debug, Default)]
struct Slot {
    cached: Option<Cached>,
    /// When the last run ended and its error; cleared by a run that succeeds.
    failed: Option<(Instant, CredentialError)>,
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
            cache: Mutex::new(Slot::default()),
            observed: std::sync::Mutex::new(Observed::default()),
        }
    }

    fn observed(&self) -> std::sync::MutexGuard<'_, Observed> {
        self.observed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Records one run that started at `at` and took `elapsed`.
    fn observe(
        &self,
        at: jiff::Timestamp,
        elapsed: Duration,
        run: &Result<Cached, CredentialError>,
    ) {
        let current = run.as_ref().ok().map(|cached| Current {
            masked: cached.credential.masked(),
            fetched_at: at,
            expires_at: cached
                .expires_at
                .and_then(|t| jiff::Timestamp::try_from(t).ok()),
            refresh_at: jiff::SignedDuration::try_from(
                cached.valid_until.saturating_duration_since(Instant::now()),
            )
            .ok()
            .and_then(|remaining| jiff::Timestamp::now().checked_add(remaining).ok()),
        });
        let record = RefreshRecord {
            at,
            duration_ms: elapsed.as_millis() as u64,
            masked: current.as_ref().map(|c| c.masked.clone()),
            error: run.as_ref().err().map(ToString::to_string),
        };
        let mut observed = self.observed();
        observed.current = current;
        if observed.runs.len() == REFRESH_HISTORY {
            observed.runs.pop_front();
        }
        observed.runs.push_back(record);
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
    /// A call that waited for a run that failed returns that run's error
    /// instead of running the command again.
    async fn credential(&self) -> Result<Option<Credential>, CredentialError> {
        let arrived = Instant::now();
        let mut slot = self.cache.lock().await;
        if let Some(cached) = &slot.cached
            && Instant::now() < cached.valid_until
        {
            return Ok(Some(cached.credential.clone()));
        }
        if let Some((ended, error)) = &slot.failed
            && *ended >= arrived
        {
            return Err(error.clone());
        }
        let (at, started) = (jiff::Timestamp::now(), Instant::now());
        let fetched = self.fetch().await;
        self.observe(at, started.elapsed(), &fetched);
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(error) => {
                tracing::warn!(%error, "credential command failed");
                slot.failed = Some((Instant::now(), error.clone()));
                return Err(error);
            }
        };
        slot.failed = None;
        let expires_at = fetched
            .expires_at
            .map(|t| humantime::format_rfc3339_seconds(t).to_string());
        tracing::info!(
            credential = %fetched.credential.masked(),
            expires_at = expires_at.as_deref(),
            "credential refreshed"
        );
        let credential = fetched.credential.clone();
        slot.cached = Some(fetched);
        Ok(Some(credential))
    }

    async fn invalidate(&self, rejected: &Credential) {
        let mut slot = self.cache.lock().await;
        if slot
            .cached
            .as_ref()
            .is_some_and(|cached| cached.credential == *rejected)
        {
            slot.cached = None;
            self.observed().current = None;
        }
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

    fn status(&self) -> CredentialStatus {
        let observed = self.observed();
        let current = observed.current.clone();
        CredentialStatus {
            source: self.describe(),
            masked: current.as_ref().map(|c| c.masked.clone()),
            fetched_at: current.as_ref().map(|c| c.fetched_at),
            expires_at: current.as_ref().and_then(|c| c.expires_at),
            refresh_at: current.and_then(|c| c.refresh_at),
            refreshes: observed.runs.iter().cloned().collect(),
        }
    }
}
