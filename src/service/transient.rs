//! What the service sees: a command run in a transient unit under the systemd
//! user manager, which starts it with the environment `anthroxy.service` gets.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;

use super::systemd::{ServiceError, command_line};
use crate::credential::exec::Run;

const PROGRAM: &str = "systemd-run";

/// `--pipe` keeps the output out of the journal and off the terminal;
/// `--collect` drops the unit even when it fails.
const ARGS: [&str; 6] = ["--user", "--pipe", "--wait", "--collect", "--quiet", "--"];

/// Variables that differ per process or per invocation and never explain a
/// credential command's failure.
const NOISE: [&str; 9] = [
    "_",
    "OLDPWD",
    "PWD",
    "SHLVL",
    "INVOCATION_ID",
    "JOURNAL_STREAM",
    "SYSTEMD_EXEC_PID",
    "MEMORY_PRESSURE_WATCH",
    "MEMORY_PRESSURE_WRITE",
];

/// Runs `sh -c command` as a user unit does. `Err` only when the transient
/// unit could not be started or did not finish within `timeout`.
pub async fn shell(command: &str, timeout: Duration) -> Result<Run, ServiceError> {
    run(&["/bin/sh", "-c", command], timeout).await
}

/// The whole invocation. systemd-run needs an absolute program, so the command
/// travels as an argument of `/bin/sh`, the interpreter the router itself uses.
fn argv<'a>(program: &[&'a str]) -> Vec<&'a str> {
    ARGS.iter()
        .copied()
        .chain(program.iter().copied())
        .collect()
}

/// The environment a user unit starts with.
pub async fn environment(timeout: Duration) -> Result<BTreeMap<String, String>, ServiceError> {
    let argv = ["/usr/bin/env", "-0"];
    let run = run(&argv, timeout).await?;
    if !run.success {
        return Err(ServiceError::Command {
            command: line(&argv),
            detail: first_line(&run.stderr).to_owned(),
        });
    }
    Ok(parse_env(&run.stdout))
}

/// Starts one transient unit and waits for it. `Err` means no unit ran: the
/// manager is out of reach, or it did not finish within `timeout`.
async fn run(command: &[&str], timeout: Duration) -> Result<Run, ServiceError> {
    if !cfg!(target_os = "linux") {
        return Err(ServiceError::Unsupported);
    }
    let failed = |detail: String| ServiceError::Command {
        command: line(command),
        detail,
    };
    let started = Instant::now();
    let child = Command::new(PROGRAM)
        .args(argv(command))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| failed(e.to_string()))?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            failed(format!(
                "did not finish within {}",
                humantime::format_duration(timeout)
            ))
        })?
        .map_err(|e| failed(e.to_string()))?;
    Ok(Run::new(&output, started.elapsed()))
}

/// The invocation as run, for an error a user has to act on.
fn line(command: &[&str]) -> String {
    command_line(PROGRAM, &argv(command))
}

fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or("no output")
}

/// This process's environment, in the same shape.
pub fn process_environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

/// `NAME=value` pairs from `env -0`.
fn parse_env(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

/// How this process's environment differs from the service's, noise removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Delta {
    /// Set here, absent in the service.
    pub missing: Vec<String>,
    /// Set in the service, absent here.
    pub extra: Vec<String>,
    pub changed: Vec<Change>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub name: String,
    pub shell: String,
    pub service: String,
}

fn is_noise(name: &str) -> bool {
    NOISE.contains(&name)
}

impl Delta {
    pub fn between(shell: &BTreeMap<String, String>, service: &BTreeMap<String, String>) -> Delta {
        let mut delta = Delta::default();
        for (name, value) in shell.iter().filter(|(n, _)| !is_noise(n)) {
            match service.get(name) {
                None => delta.missing.push(name.clone()),
                Some(other) if other != value => delta.changed.push(Change {
                    name: name.clone(),
                    shell: value.clone(),
                    service: other.clone(),
                }),
                Some(_) => {}
            }
        }
        delta.extra = service
            .keys()
            .filter(|n| !is_noise(n) && !shell.contains_key(*n))
            .cloned()
            .collect();
        delta
    }

    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty() && self.changed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn the_unit_runs_the_command_the_way_the_router_would() {
        assert_eq!(
            line(&["/bin/sh", "-c", "cat ~/.token"]),
            "systemd-run --user --pipe --wait --collect --quiet -- /bin/sh -c cat ~/.token",
            "no flag may go missing: --pipe keeps the secret off the journal"
        );
        assert_eq!(argv(&["/usr/bin/env", "-0"]).last(), Some(&"-0"));
        assert_eq!(argv(&[])[ARGS.len() - 1], "--", "the command follows `--`");
    }

    #[test]
    fn parses_null_separated_pairs_with_values_of_their_own() {
        let stdout = "PATH=/usr/bin:/bin\0LS_COLORS=di=1;34:ln=36\0EMPTY=\0";
        assert_eq!(
            parse_env(stdout),
            env(&[
                ("PATH", "/usr/bin:/bin"),
                ("LS_COLORS", "di=1;34:ln=36"),
                ("EMPTY", ""),
            ])
        );
        assert!(parse_env("").is_empty());
    }

    #[test]
    fn delta_reports_missing_extra_and_changed_variables() {
        let shell = env(&[
            ("PATH", "/opt/homebrew/bin:/usr/bin"),
            ("HOME", "/home/u"),
            ("SSH_AUTH_SOCK", "/tmp/ssh"),
        ]);
        let service = env(&[
            ("PATH", "/usr/bin"),
            ("HOME", "/home/u"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]);
        let delta = Delta::between(&shell, &service);
        assert_eq!(delta.missing, ["SSH_AUTH_SOCK"]);
        assert_eq!(delta.extra, ["XDG_RUNTIME_DIR"]);
        assert_eq!(
            delta.changed,
            [Change {
                name: "PATH".to_owned(),
                shell: "/opt/homebrew/bin:/usr/bin".to_owned(),
                service: "/usr/bin".to_owned(),
            }]
        );
        assert!(!delta.is_empty());
    }

    #[test]
    fn delta_ignores_per_invocation_noise() {
        let shell = env(&[("PWD", "/home/u"), ("SHLVL", "1"), ("_", "/usr/bin/env")]);
        let service = env(&[("PWD", "/"), ("INVOCATION_ID", "abc"), ("SHLVL", "2")]);
        assert_eq!(Delta::between(&shell, &service), Delta::default());
        assert!(Delta::between(&shell, &service).is_empty());
    }
}
