//! `credential`: acquire every backend's credential the way the router does,
//! and report what the command printed. With `--as-service` the same command
//! also runs in the systemd user service's environment, which is where a
//! command that works in a shell usually stops working.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use clap::Args;

use super::{Cli, Style};
use crate::config::{BackendConfig, CommandOutput, Config, CredentialConfig};
use crate::credential::exec::{self, Run};
use crate::credential::{CommandCredential, CredentialSource, mask};
use crate::service::transient::{self, Delta};

/// Reading the environment of a transient unit must not hang the report.
const ENVIRONMENT_TIMEOUT: Duration = Duration::from_secs(10);

/// `service`, the longest environment label.
const LABEL_WIDTH: usize = 7;

/// `credential`, the longest detail key.
const KEY_WIDTH: usize = 10;

/// `changed`, the longest key of the environment delta.
const CHANGE_WIDTH: usize = 7;

#[derive(Debug, Clone, Args)]
pub struct CredentialArgs {
    /// Backends to try; every configured backend when omitted
    #[arg(value_name = "BACKEND")]
    pub backends: Vec<String>,
    /// Run each command in the systemd user service's environment as well, and
    /// report how that environment differs from this one (Linux)
    #[arg(long)]
    pub as_service: bool,
    /// Print credentials and environment values in full instead of masked
    #[arg(long)]
    pub reveal: bool,
}

pub async fn run(cli: &Cli, args: &CredentialArgs, style: &Style) -> anyhow::Result<()> {
    cli.init_tracing(None, "warn")?;
    let config = cli.load_config()?;
    let selected = select(&config, &args.backends)?;
    let service = if args.as_service {
        Some(transient::environment(ENVIRONMENT_TIMEOUT).await?)
    } else {
        None
    };
    if service.is_some() {
        println!(
            "{}",
            style.dim("service: systemd-run --user, the environment anthroxy.service starts with")
        );
    }

    let mut problems = 0usize;
    for (name, backend) in selected {
        let attempts = attempts(&backend.credential, service.as_ref(), args.reveal).await;
        problems += attempts.iter().filter(|a| !a.ok).count();
        println!();
        print!(
            "{}",
            render(name, &describe(&backend.credential), &attempts, style)
        );
    }

    if let Some(service) = &service {
        println!();
        let delta = Delta::between(&transient::process_environment(), service);
        print!("{}", render_delta(&delta, style, args.reveal));
    }

    println!();
    if problems == 0 {
        println!("{} every credential acquired", style.ok_mark());
        Ok(())
    } else {
        println!(
            "{} {problems} credential problem(s) above.",
            style.err_mark()
        );
        if !args.reveal {
            println!(
                "{}",
                style.dim("  `--reveal` prints credentials and command output unmasked.")
            );
        }
        anyhow::bail!("{problems} credential problem(s)")
    }
}

/// The named backends in the order given, or every configured one.
fn select<'a>(
    config: &'a Config,
    names: &[String],
) -> anyhow::Result<Vec<(&'a String, &'a BackendConfig)>> {
    if names.is_empty() {
        return Ok(config.backends.iter().collect());
    }
    names
        .iter()
        .map(|name| {
            config.backends.get_key_value(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "no backend named `{name}`; configured: {}",
                    config
                        .backends
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .collect()
}

/// One attempt per environment, in the order they are reported.
async fn attempts(
    config: &CredentialConfig,
    service: Option<&BTreeMap<String, String>>,
    reveal: bool,
) -> Vec<Attempt> {
    match config {
        CredentialConfig::Command {
            command,
            output,
            refresh,
            timeout,
            ..
        } => {
            let reading = Reading {
                output: *output,
                refresh: *refresh,
                reveal,
            };
            let here = exec::run(command, *timeout)
                .await
                .map_err(|e| e.to_string());
            let mut attempts = vec![ran("shell", here, reading)];
            if service.is_some() {
                let there = transient::shell(command, *timeout)
                    .await
                    .map_err(|e| e.to_string());
                attempts.push(ran("service", there, reading));
            }
            attempts
        }
        CredentialConfig::Env { name, .. } => {
            let mut attempts = vec![variable("shell", std::env::var(name).ok(), reveal)];
            if let Some(service) = service {
                attempts.push(variable("service", service.get(name).cloned(), reveal));
            }
            attempts
        }
        CredentialConfig::Static { .. } | CredentialConfig::None => Vec::new(),
    }
}

/// Where the credential comes from, in the wording `check` uses.
fn describe(config: &CredentialConfig) -> String {
    match config {
        CredentialConfig::Command {
            command,
            output,
            refresh,
            timeout,
            header,
        } => CommandCredential::new(command.clone(), *output, header.clone(), *refresh, *timeout)
            .describe(),
        CredentialConfig::Env { name, .. } => format!("env ${name}"),
        CredentialConfig::Static { .. } => "static".to_owned(),
        CredentialConfig::None => "none".to_owned(),
    }
}

/// What a credential command's output means; fixed for one backend.
#[derive(Debug, Clone, Copy)]
struct Reading {
    output: CommandOutput,
    refresh: Duration,
    reveal: bool,
}

/// One credential lookup in one environment.
#[derive(Debug, PartialEq, Eq)]
struct Attempt {
    /// `shell` or `service`.
    label: &'static str,
    ok: bool,
    /// Follows the label: `exit 0 in 84 ms`, or why nothing ran.
    headline: String,
    /// Lines under the headline; values may span several lines.
    details: Vec<(&'static str, String)>,
}

/// One run of the command, read as the router would read it.
fn ran(label: &'static str, run: Result<Run, String>, reading: Reading) -> Attempt {
    let run = match run {
        Ok(run) => run,
        Err(error) => {
            return Attempt {
                label,
                ok: false,
                headline: error,
                details: Vec::new(),
            };
        }
    };
    let headline = format!("{} in {} ms", run.status, run.elapsed.as_millis());
    let read = exec::interpret(&run, reading.output).and_then(|output| {
        exec::valid_for(reading.refresh, output.expires_at).map(|valid| (output, valid))
    });
    let mut details = Vec::new();
    let ok = read.is_ok();
    match read {
        Ok((output, valid)) => {
            details.push(("credential", show(&output.secret, reading.reveal)));
            if let Some(expires_at) = output.expires_at {
                details.push(("expires", expiry(expires_at)));
            }
            details.push(("re-run in", duration(valid)));
        }
        Err(error) => {
            // A non-zero exit is already in the headline; its error text only
            // repeats the status and the stderr shown below.
            if run.success {
                details.push(("error", error.to_string()));
            } else if !run.stderr.trim().is_empty() {
                details.push(("stderr", run.stderr.trim().to_owned()));
            }
            if !run.stdout.trim().is_empty() {
                details.push(("stdout", show(run.stdout.trim(), reading.reveal)));
            }
        }
    }
    Attempt {
        label,
        ok,
        headline,
        details,
    }
}

/// An `env` credential: the variable as that environment has it.
fn variable(label: &'static str, value: Option<String>, reveal: bool) -> Attempt {
    match value {
        Some(value) => Attempt {
            label,
            ok: true,
            headline: "set".to_owned(),
            details: vec![("value", show(&value, reveal))],
        },
        None => Attempt {
            label,
            ok: false,
            headline: "not set".to_owned(),
            details: Vec::new(),
        },
    }
}

/// Masked with its length, which is enough to tell two credentials apart.
/// Masking collapses whitespace so a multi-line output stays one row.
fn show(secret: &str, reveal: bool) -> String {
    if reveal {
        return secret.to_owned();
    }
    let one_line = secret.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{} ({} chars)", mask(&one_line), secret.chars().count())
}

fn expiry(expires_at: SystemTime) -> String {
    let remaining = expires_at
        .duration_since(SystemTime::now())
        .map(|left| format!("in {}", duration(left)))
        .unwrap_or_else(|_| "expired".to_owned());
    format!(
        "{} ({remaining})",
        humantime::format_rfc3339_seconds(expires_at)
    )
}

/// Whole seconds: sub-second precision is noise in a report.
fn duration(value: Duration) -> String {
    humantime::format_duration(Duration::from_secs(value.as_secs())).to_string()
}

/// One backend: what its credential is, then one block per environment.
fn render(name: &str, description: &str, attempts: &[Attempt], style: &Style) -> String {
    let mut out = format!("{}  {description}\n", style.bold(name));
    for attempt in attempts {
        let mark = if attempt.ok {
            style.ok_mark()
        } else {
            style.err_mark()
        };
        out.push_str(&format!(
            "  {:<width$}  {mark} {}\n",
            attempt.label,
            attempt.headline,
            width = LABEL_WIDTH
        ));
        for (key, value) in &attempt.details {
            for (index, line) in value.lines().enumerate() {
                let key = if index == 0 { *key } else { "" };
                out.push_str(&format!("      {key:<KEY_WIDTH$}  {line}\n"));
            }
        }
    }
    out
}

/// The environment difference that explains a command failing only as a
/// service. Values are shown for `PATH`, and for everything with `--reveal`.
fn render_delta(delta: &Delta, style: &Style, reveal: bool) -> String {
    let header = style.bold("environment (shell → service)");
    if delta.is_empty() {
        return format!("{header}  identical\n");
    }
    let mut out = format!("{header}\n");
    for change in &delta.changed {
        if reveal || change.name == "PATH" {
            out.push_str(&row(&change.name, &change.shell));
            out.push_str(&format!("  {:>CHANGE_WIDTH$}  {}\n", "→", change.service));
        }
    }
    if !reveal {
        let names: Vec<&str> = delta
            .changed
            .iter()
            .map(|c| c.name.as_str())
            .filter(|name| *name != "PATH")
            .collect();
        if !names.is_empty() {
            out.push_str(&row("changed", &names.join(", ")));
        }
    }
    if !delta.missing.is_empty() {
        out.push_str(&row("missing", &delta.missing.join(", ")));
    }
    if !delta.extra.is_empty() {
        out.push_str(&row("extra", &delta.extra.join(", ")));
    }
    out
}

fn row(key: &str, value: &str) -> String {
    format!("  {key:<CHANGE_WIDTH$}  {value}\n")
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::service::transient::Change;

    fn text() -> Reading {
        Reading {
            output: CommandOutput::Text,
            refresh: Duration::from_secs(300),
            reveal: false,
        }
    }

    fn run(status: &str, stdout: &str, stderr: &str) -> Run {
        Run {
            success: status == "exit 0",
            status: status.to_owned(),
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
            elapsed: Duration::from_millis(12),
        }
    }

    #[test]
    fn a_failed_run_reports_its_status_and_stderr() {
        let attempt = ran(
            "service",
            Ok(run("exit 127", "", "sh: 1: jq: not found\n")),
            text(),
        );
        assert!(!attempt.ok);
        assert_eq!(attempt.headline, "exit 127 in 12 ms");
        assert_eq!(
            attempt.details,
            [("stderr", "sh: 1: jq: not found".to_owned())]
        );
    }

    #[test]
    fn a_command_that_never_ran_reports_why() {
        let attempt = ran(
            "shell",
            Err("credential command exceeded 10s".into()),
            text(),
        );
        assert!(!attempt.ok);
        assert_eq!(attempt.headline, "credential command exceeded 10s");
        assert!(attempt.details.is_empty());
    }

    #[test]
    fn a_successful_run_masks_the_credential_and_says_when_it_is_re_run() {
        let attempt = ran("shell", Ok(run("exit 0", "  tok-abcdefgh \n", "")), text());
        assert!(attempt.ok);
        assert_eq!(attempt.headline, "exit 0 in 12 ms");
        assert_eq!(
            attempt.details,
            [
                ("credential", "tok-…efgh (12 chars)".to_owned()),
                ("re-run in", "5m".to_owned()),
            ]
        );
    }

    #[test]
    fn reveal_prints_the_credential_in_full() {
        let reading = Reading {
            reveal: true,
            ..text()
        };
        let attempt = ran("shell", Ok(run("exit 0", "tok-abcdefgh\n", "")), reading);
        assert_eq!(
            attempt.details[0],
            ("credential", "tok-abcdefgh".to_owned())
        );
    }

    #[test]
    fn json_output_reports_the_expiry_it_carries() {
        let expires_at = SystemTime::now() + Duration::from_secs(3600);
        let stdout = format!(
            r#"{{"token": "tok-abcdefgh", "expires_at": "{}"}}"#,
            humantime::format_rfc3339_seconds(expires_at)
        );
        let reading = Reading {
            output: CommandOutput::Json,
            ..text()
        };
        let attempt = ran("shell", Ok(run("exit 0", &stdout, "")), reading);
        assert!(attempt.ok);
        let expires = attempt.details.iter().find(|(k, _)| *k == "expires");
        let (_, value) = expires.unwrap_or_else(|| panic!("{:?}", attempt.details));
        assert!(
            value.starts_with(&humantime::format_rfc3339_seconds(expires_at).to_string()),
            "{value}"
        );
        assert!(value.ends_with(')'), "{value} carries the remaining time");
    }

    #[test]
    fn output_the_router_cannot_read_is_a_problem_with_the_output_shown() {
        let reading = Reading {
            output: CommandOutput::Json,
            ..text()
        };
        let attempt = ran("shell", Ok(run("exit 0", "null\n", "")), reading);
        assert!(!attempt.ok);
        assert_eq!(attempt.details.len(), 2, "{:?}", attempt.details);
        assert_eq!(attempt.details[0].0, "error");
        assert_eq!(attempt.details[1], ("stdout", "**** (4 chars)".to_owned()));
    }

    #[test]
    fn an_env_credential_reports_whether_the_variable_is_set() {
        let set = variable("service", Some("key-1234567890".to_owned()), false);
        assert!(set.ok);
        assert_eq!(set.headline, "set");
        assert_eq!(set.details, [("value", "key-…7890 (14 chars)".to_owned())]);

        let unset = variable("service", None, false);
        assert!(!unset.ok);
        assert_eq!(unset.headline, "not set");
        assert!(unset.details.is_empty());
    }

    #[test]
    fn render_puts_every_environment_under_the_backend_with_its_details() {
        let attempts = [
            ran("shell", Ok(run("exit 0", "tok-abcdefgh", "")), text()),
            ran(
                "service",
                Ok(run("exit 127", "", "sh: 1: jq: not found\nsecond line")),
                text(),
            ),
        ];
        let out = render(
            "claude",
            "command `x | jq -r .t`",
            &attempts,
            &Style::plain(),
        );
        assert_eq!(
            out,
            "\
claude  command `x | jq -r .t`
  shell    ✓ exit 0 in 12 ms
      credential  tok-…efgh (12 chars)
      re-run in   5m
  service  ✗ exit 127 in 12 ms
      stderr      sh: 1: jq: not found
                  second line
"
        );
    }

    #[test]
    fn render_delta_shows_path_in_full_and_other_changes_by_name() {
        let delta = Delta {
            missing: vec!["SSH_AUTH_SOCK".to_owned(), "HOMEBREW_PREFIX".to_owned()],
            extra: vec!["XDG_RUNTIME_DIR".to_owned()],
            changed: vec![
                Change {
                    name: "PATH".to_owned(),
                    shell: "/opt/homebrew/bin:/usr/bin".to_owned(),
                    service: "/usr/bin".to_owned(),
                },
                Change {
                    name: "HOME".to_owned(),
                    shell: "/home/u".to_owned(),
                    service: "/root".to_owned(),
                },
            ],
        };
        let out = render_delta(&delta, &Style::plain(), false);
        assert_eq!(
            out,
            "\
environment (shell → service)
  PATH     /opt/homebrew/bin:/usr/bin
        →  /usr/bin
  changed  HOME
  missing  SSH_AUTH_SOCK, HOMEBREW_PREFIX
  extra    XDG_RUNTIME_DIR
"
        );

        let revealed = render_delta(&delta, &Style::plain(), true);
        assert!(
            revealed.contains("  HOME     /home/u\n        →  /root\n"),
            "{revealed}"
        );

        assert_eq!(
            render_delta(&Delta::default(), &Style::plain(), false),
            "environment (shell → service)  identical\n"
        );
    }
}
