//! Installation verdicts for `service status`: is systemd reachable, is the
//! unit there and pointing at this binary and config, is it enabled, active,
//! allowed to run without a login, and is the router answering.

use std::path::Path;
use std::time::Duration;

use super::systemd::{CommandRunner, UNIT_NAME, parse_exec_start};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Fail,
    /// Could not be evaluated because an earlier check failed or the input
    /// was unavailable.
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub verdict: Verdict,
    pub detail: String,
}

/// What a correct installation looks like from where `status` runs.
pub struct Expected<'a> {
    pub exe: &'a Path,
    pub config: &'a Path,
    pub unit_path: &'a Path,
    pub user: &'a str,
    /// `http://127.0.0.1:<port>/healthz`; `None` when the config could not be
    /// loaded, with the reason.
    pub health_url: Result<String, String>,
}

/// Runs every check. Never fails: problems are verdicts.
pub async fn collect(expected: &Expected<'_>, runner: &dyn CommandRunner) -> Vec<Check> {
    let mut checks = Vec::with_capacity(7);

    let manager_up = match runner.run("systemctl", &["--user", "is-system-running"]) {
        Ok(output) if manager_state_is_up(&output.stdout) => {
            checks.push(check(
                "systemd",
                Verdict::Ok,
                format!("user manager {}", output.stdout.trim()),
            ));
            true
        }
        Ok(output) => {
            checks.push(check(
                "systemd",
                Verdict::Fail,
                format!("{}{}", output.message(), SYSTEMD_HINT),
            ));
            false
        }
        Err(error) => {
            checks.push(check(
                "systemd",
                Verdict::Fail,
                format!("{error}{SYSTEMD_HINT}"),
            ));
            false
        }
    };

    let unit_text = std::fs::read_to_string(expected.unit_path).ok();
    match &unit_text {
        Some(_) => checks.push(check(
            "unit file",
            Verdict::Ok,
            expected.unit_path.display().to_string(),
        )),
        None => checks.push(check(
            "unit file",
            Verdict::Fail,
            format!(
                "{} not found; run `anthroxy service install`",
                expected.unit_path.display()
            ),
        )),
    }

    match unit_text.as_deref().map(parse_exec_start) {
        None => checks.push(check("unit paths", Verdict::Skip, "no unit file")),
        Some(None) => checks.push(check(
            "unit paths",
            Verdict::Fail,
            "ExecStart line not recognised; run `anthroxy service install` to rewrite the unit",
        )),
        Some(Some(exec)) => {
            let mut mismatches = Vec::new();
            if !same_path(&exec.exe, expected.exe) {
                mismatches.push(format!(
                    "binary: unit runs {}, this is {}",
                    exec.exe.display(),
                    expected.exe.display()
                ));
            }
            if !same_path(&exec.config, expected.config) {
                mismatches.push(format!(
                    "config: unit uses {}, this is {}",
                    exec.config.display(),
                    expected.config.display()
                ));
            }
            if mismatches.is_empty() {
                checks.push(check(
                    "unit paths",
                    Verdict::Ok,
                    format!("{} --config {}", exec.exe.display(), exec.config.display()),
                ));
            } else {
                mismatches.push("run `anthroxy service install` to update the unit".to_owned());
                checks.push(check("unit paths", Verdict::Fail, mismatches.join("; ")));
            }
        }
    }

    if manager_up {
        checks.push(systemctl_state(
            runner,
            "enabled",
            "is-enabled",
            "enabled",
            "run `systemctl --user enable anthroxy.service`",
        ));
        checks.push(systemctl_state(
            runner,
            "active",
            "is-active",
            "active",
            "see `journalctl --user -u anthroxy.service -n 20`",
        ));
    } else {
        checks.push(check("enabled", Verdict::Skip, "systemd not reachable"));
        checks.push(check("active", Verdict::Skip, "systemd not reachable"));
    }

    match runner.run("loginctl", &["show-user", expected.user, "--property=Linger", "--value"]) {
        Ok(output) if output.success && output.stdout.trim() == "yes" => {
            checks.push(check("linger", Verdict::Ok, "user services start without a login session"));
        }
        Ok(output) => checks.push(check(
            "linger",
            Verdict::Fail,
            format!(
                "Linger={}; run `loginctl enable-linger` so the router starts without a login session",
                if output.stdout.is_empty() { output.message().to_owned() } else { output.stdout.clone() }
            ),
        )),
        Err(error) => checks.push(check("linger", Verdict::Skip, error.to_string())),
    }

    match &expected.health_url {
        Err(reason) => checks.push(check(
            "health",
            Verdict::Skip,
            format!("cannot derive the listen port: {reason}"),
        )),
        Ok(url) => match health(url).await {
            Ok(200) => checks.push(check("health", Verdict::Ok, format!("HTTP 200 from {url}"))),
            Ok(status) => checks.push(check(
                "health",
                Verdict::Fail,
                format!("HTTP {status} from {url}"),
            )),
            Err(error) => checks.push(check("health", Verdict::Fail, format!("{url}: {error}"))),
        },
    }

    checks
}

const SYSTEMD_HINT: &str = "; enable systemd with `[boot] systemd=true` in /etc/wsl.conf, then `wsl --shutdown` from Windows";

/// `systemctl --user <query> anthroxy.service`, Ok when stdout equals `want`.
fn systemctl_state(
    runner: &dyn CommandRunner,
    name: &'static str,
    query: &str,
    want: &str,
    hint: &str,
) -> Check {
    match runner.run("systemctl", &["--user", query, UNIT_NAME]) {
        Ok(output) if output.stdout.trim() == want => check(name, Verdict::Ok, want),
        Ok(output) => {
            let state = if output.stdout.is_empty() {
                output.message().to_owned()
            } else {
                output.stdout.clone()
            };
            check(name, Verdict::Fail, format!("{state}; {hint}"))
        }
        Err(error) => check(name, Verdict::Skip, error.to_string()),
    }
}

fn check(name: &'static str, verdict: Verdict, detail: impl Into<String>) -> Check {
    Check {
        name,
        verdict,
        detail: detail.into(),
    }
}

/// True when the systemd user manager answered at all; `degraded` still
/// means it is running.
fn manager_state_is_up(output: &str) -> bool {
    matches!(
        output.trim(),
        "running" | "degraded" | "starting" | "initializing" | "maintenance" | "stopping"
    )
}

/// Same file even when one side is a symlink or relative.
fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

async fn health(url: &str) -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    client
        .get(url)
        .send()
        .await
        .map(|r| r.status().as_u16())
        .map_err(|e| crate::upstream::describe(&e))
}
