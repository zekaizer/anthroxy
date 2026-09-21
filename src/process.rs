//! Running a child with a time limit and an output cap. The child gets its
//! own process group so a timeout takes its pipeline with it, and each
//! stream is read up to [`MAX_OUTPUT_BYTES`] so a chatty helper cannot grow
//! the router's memory.

use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// Bytes read from each of stdout and stderr before the run is failed.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("cannot start: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("did not finish within {}", humantime::format_duration(*.0))]
    Timeout(Duration),
    #[error("printed more than {0} bytes")]
    OutputTooLarge(usize),
    #[error("cannot read its output: {0}")]
    Read(#[source] std::io::Error),
}

/// Runs `program` with `args`, no stdin, until it exits or `timeout` passes.
pub async fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Output, RunError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    let group = child.id();
    let result = tokio::time::timeout(timeout, collect(&mut child)).await;
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            kill_group(group);
            Err(error)
        }
        Err(_) => {
            kill_group(group);
            Err(RunError::Timeout(timeout))
        }
    }
}

async fn collect(child: &mut Child) -> Result<Output, RunError> {
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    // The first stream past the cap ends the run; the other read is dropped
    // with it, so a child blocked on a full pipe cannot hold this up.
    let (out, err) = tokio::try_join!(read_capped(&mut stdout), read_capped(&mut stderr))?;
    let status = child.wait().await.map_err(RunError::Read)?;
    Ok(Output {
        status,
        stdout: out,
        stderr: err,
    })
}

async fn read_capped(stream: &mut (impl AsyncReadExt + Unpin)) -> Result<Vec<u8>, RunError> {
    let mut buf = Vec::new();
    stream
        .take(MAX_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .await
        .map_err(RunError::Read)?;
    if buf.len() > MAX_OUTPUT_BYTES {
        return Err(RunError::OutputTooLarge(MAX_OUTPUT_BYTES));
    }
    Ok(buf)
}

/// Kills the child's whole process group; `kill_on_drop` reaches only the
/// child itself, not what it started.
#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) {
        // SAFETY: killpg has no memory-safety preconditions; the group is
        // ours, created by process_group(0) at spawn.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}
