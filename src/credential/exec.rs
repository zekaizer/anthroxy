//! Running a credential command and reading what it produced. The router and
//! `anthroxy credential` share this so a report shows what the router sees.

use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

use tokio::process::Command;

use super::{CredentialError, Output, output};
use crate::config::CommandOutput;

/// A credential this close to its reported expiry is re-acquired instead of
/// served from cache.
pub const EXPIRY_MARGIN: Duration = Duration::from_secs(120);

/// One finished run of a credential command, before interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub success: bool,
    /// `exit <code>`, or how a signal ended the process.
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

impl Run {
    pub fn new(output: &std::process::Output, elapsed: Duration) -> Run {
        Run {
            success: output.status.success(),
            status: match output.status.code() {
                Some(code) => format!("exit {code}"),
                None => output.status.to_string(),
            },
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            elapsed,
        }
    }
}

/// Runs `command` through `sh -c` with no stdin. `Err` only when no exit
/// status was reached: the process could not start, or `timeout` elapsed.
pub async fn run(command: &str, timeout: Duration) -> Result<Run, CredentialError> {
    tracing::debug!(command = %command, "running credential command");
    let started = Instant::now();
    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(CredentialError::Spawn)?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| CredentialError::Timeout(timeout))?
        .map_err(CredentialError::Spawn)?;
    Ok(Run::new(&output, started.elapsed()))
}

/// The credential in a finished run. A non-zero exit is an error before the
/// output is looked at, so a command that prints on both streams and fails
/// does not yield a credential.
pub fn interpret(run: &Run, mode: CommandOutput) -> Result<Output, CredentialError> {
    if !run.success {
        return Err(CredentialError::Failed {
            status: run.status.clone(),
            stderr: run.stderr.clone(),
        });
    }
    output::parse(mode, &run.stdout)
}

/// How long the credential may be served from cache: `refresh`, cut short when
/// the reported expiry is nearer than [`EXPIRY_MARGIN`]. A credential that has
/// already expired is [`CredentialError::Expired`] rather than one the backend
/// will reject.
pub fn valid_for(
    refresh: Duration,
    expires_at: Option<SystemTime>,
) -> Result<Duration, CredentialError> {
    let Some(expires_at) = expires_at else {
        return Ok(refresh);
    };
    let remaining = expires_at
        .duration_since(SystemTime::now())
        .map_err(|_| CredentialError::Expired(expires_at))?;
    Ok(refresh.min(remaining.saturating_sub(EXPIRY_MARGIN)))
}
