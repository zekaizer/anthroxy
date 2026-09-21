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

/// After a failed run, how long a value still in use is served before the
/// command is run again (ADR-0015).
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30);

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
    /// The value the last `invalidate` cleared, until a run replaces it.
    invalidated: Option<Credential>,
    /// A value a run handed back although it had just been rejected: the
    /// helper has nothing newer, so further rejections of it within
    /// [`RETRY_AFTER_FAILURE`] do not run the command again per request.
    reproduced: Option<(Credential, Instant)>,
}

impl Slot {
    /// The cached value, when a failed refresh may still hand it out at `now`.
    fn fallback(&self, now: Instant) -> Option<Credential> {
        let cached = self.cached.as_ref()?;
        (now < cached.usable_until?).then(|| cached.credential.clone())
    }
}

#[derive(Debug)]
struct Cached {
    credential: Credential,
    /// Served from cache until then; the command is re-run after.
    valid_until: Instant,
    /// Reported by the command, for the refresh log line.
    expires_at: Option<SystemTime>,
    /// Served past `valid_until` while refreshes fail, until then: the
    /// reported expiry less the margin. `None` without a reported expiry.
    usable_until: Option<Instant>,
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

    /// Records one run that started at `at` and took `elapsed`. A failed run
    /// leaves the current value to the caller, which knows whether it is
    /// still served.
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
            refresh_at: timestamp(cached.valid_until),
        });
        let record = RefreshRecord {
            at,
            duration_ms: elapsed.as_millis() as u64,
            masked: current.as_ref().map(|c| c.masked.clone()),
            error: run.as_ref().err().map(ToString::to_string),
        };
        let mut observed = self.observed();
        if current.is_some() {
            observed.current = current;
        }
        if observed.runs.len() == REFRESH_HISTORY {
            observed.runs.pop_front();
        }
        observed.runs.push_back(record);
    }

    /// Runs the command once.
    async fn fetch(&self) -> Result<Cached, CredentialError> {
        // `${NAME}` is resolved here, each run, so the loaded configuration,
        // the console and `check` show the reference, never the secret.
        let command = crate::config::env::expand(&self.command, &|name| std::env::var(name).ok())
            .map_err(|error| match error {
            crate::config::ConfigError::MissingEnv(name) => CredentialError::MissingEnv(name),
            other => CredentialError::MissingEnv(other.to_string()),
        })?;
        let run = exec::run(&command, self.timeout).await?;
        let output = exec::interpret(&run, self.output)?;
        let valid_for = exec::valid_for(self.refresh, output.expires_at)?;
        let now = Instant::now();
        Ok(Cached {
            credential: Credential::new(self.header.clone(), output.secret)?,
            valid_until: now + valid_for,
            expires_at: output.expires_at,
            usable_until: output.expires_at.map(|expires_at| {
                let remaining = expires_at
                    .duration_since(SystemTime::now())
                    .unwrap_or_default();
                now + remaining.saturating_sub(exec::EXPIRY_MARGIN)
            }),
        })
    }
}

#[async_trait]
impl CredentialSource for CommandCredential {
    /// A call that waited for a run that failed gets that run's outcome
    /// instead of running the command again. A failed run hands out the
    /// previous value while its reported expiry is not near, and for
    /// [`RETRY_AFTER_FAILURE`] no run follows (ADR-0015).
    async fn credential(&self) -> Result<Option<Credential>, CredentialError> {
        let arrived = Instant::now();
        let mut slot = self.cache.lock().await;
        let now = Instant::now();
        if let Some(cached) = &slot.cached
            && now < cached.valid_until
        {
            return Ok(Some(cached.credential.clone()));
        }
        if let Some((ended, error)) = &slot.failed {
            let waited = *ended >= arrived;
            if waited || now < *ended + RETRY_AFTER_FAILURE {
                if let Some(credential) = slot.fallback(now) {
                    return Ok(Some(credential));
                }
                if waited {
                    return Err(error.clone());
                }
            }
        }
        let (at, started) = (jiff::Timestamp::now(), Instant::now());
        let fetched = self.fetch().await;
        self.observe(at, started.elapsed(), &fetched);
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(error) => {
                tracing::warn!(%error, "credential command failed");
                let ended = Instant::now();
                slot.failed = Some((ended, error.clone()));
                if let Some(credential) = slot.fallback(ended) {
                    tracing::warn!(
                        credential = %credential.masked(),
                        "serving the previous credential until its reported expiry nears"
                    );
                    if let Some(current) = &mut self.observed().current {
                        current.refresh_at = timestamp(ended + RETRY_AFTER_FAILURE);
                    }
                    return Ok(Some(credential));
                }
                self.observed().current = None;
                return Err(error);
            }
        };
        slot.failed = None;
        if slot.invalidated.take().as_ref() == Some(&fetched.credential) {
            tracing::warn!(
                credential = %fetched.credential.masked(),
                "credential command handed back the rejected value"
            );
            slot.reproduced = Some((fetched.credential.clone(), Instant::now()));
        }
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
        let reproduced = slot
            .reproduced
            .as_ref()
            .is_some_and(|(value, at)| value == rejected && at.elapsed() < RETRY_AFTER_FAILURE);
        if !reproduced
            && slot
                .cached
                .as_ref()
                .is_some_and(|cached| cached.credential == *rejected)
        {
            slot.cached = None;
            slot.invalidated = Some(rejected.clone());
            self.observed().current = None;
        }
    }

    fn is_refreshable(&self) -> bool {
        true
    }

    fn header(&self) -> Option<CredentialHeader> {
        Some(self.header.clone())
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

/// `at` on the wall clock, for status output.
fn timestamp(at: Instant) -> Option<jiff::Timestamp> {
    let remaining =
        jiff::SignedDuration::try_from(at.saturating_duration_since(Instant::now())).ok()?;
    jiff::Timestamp::now().checked_add(remaining).ok()
}
