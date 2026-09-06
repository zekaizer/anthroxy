//! systemd user unit: `~/.config/systemd/user/anthroxy.service`.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const UNIT_NAME: &str = "anthroxy.service";

/// Renders the unit. `exe` and `config` are embedded as absolute paths so the
/// unit does not depend on PATH or the working directory.
pub fn render_unit(exe: &Path, config: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=anthroxy: one endpoint for Claude Code in front of several backends\n\
         Documentation=https://github.com/zekaizer/anthroxy\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} --config {config} serve\n\
         ExecReload=/bin/kill -HUP $MAINPID\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         # Log lines already carry timestamps and levels.\n\
         Environment=RUST_LOG_STYLE=never\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe = shell_quote(exe),
        config = shell_quote(config),
    )
}

/// Quotes a path for a systemd `ExecStart` line when it contains spaces.
fn shell_quote(path: &Path) -> String {
    let text = path.display().to_string();
    if text.contains(' ') {
        format!("\"{text}\"")
    } else {
        text
    }
}

/// `~/.config/systemd/user/anthroxy.service`
pub fn unit_path() -> Result<PathBuf, ServiceError> {
    dirs::config_dir()
        .map(|d| d.join("systemd").join("user").join(UNIT_NAME))
        .ok_or(ServiceError::NoConfigDir)
}

/// The running binary, embedded in the unit as an absolute path.
pub fn exe() -> Result<PathBuf, ServiceError> {
    std::env::current_exe().map_err(ServiceError::Exe)
}

/// `program` and `args` as one shell-style line, for reports and errors.
pub fn command_line(program: &str, args: &[&str]) -> String {
    format!("{program} {}", args.join(" "))
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("systemd user services are only available on Linux")]
    Unsupported,
    #[error("cannot determine the user configuration directory")]
    NoConfigDir,
    #[error("cannot resolve the router executable: {0}")]
    Exe(#[source] std::io::Error),
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`{command}` failed: {detail}")]
    Command { command: String, detail: String },
}

/// Result of one external command, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// stderr when present, else stdout: what to show a human on failure.
    pub fn message(&self) -> &str {
        if self.stderr.is_empty() {
            &self.stdout
        } else {
            &self.stderr
        }
    }

    /// stdout when present, else stderr: `systemctl is-*` and `loginctl`
    /// print the state on stdout even when they exit non-zero.
    pub fn state(&self) -> &str {
        if self.stdout.is_empty() {
            &self.stderr
        } else {
            &self.stdout
        }
    }
}

/// Executes `systemctl`/`loginctl`. A trait so status checks can run against
/// scripted output in tests.
pub trait CommandRunner {
    /// `Err` only when the program could not be started at all.
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, ServiceError>;
}

/// Runs the real programs.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<CommandOutput, ServiceError> {
        let output =
            Command::new(program)
                .args(args)
                .output()
                .map_err(|e| ServiceError::Command {
                    command: command_line(program, args),
                    detail: e.to_string(),
                })?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// One `systemctl --user` (or `loginctl`) invocation that must succeed,
/// surfaced verbatim in the report.
pub fn run(program: &str, args: &[&str]) -> Result<String, ServiceError> {
    let output = SystemRunner.run(program, args)?;
    if output.success {
        Ok(output.stdout)
    } else {
        Err(ServiceError::Command {
            command: command_line(program, args),
            detail: output.message().to_owned(),
        })
    }
}

/// Paths embedded in a rendered unit's `ExecStart=` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecStart {
    pub exe: PathBuf,
    pub config: PathBuf,
}

/// Reads `ExecStart=<exe> --config <config> serve` back out of a unit file.
/// Accepts the quoting `render_unit` produces.
pub fn parse_exec_start(unit: &str) -> Option<ExecStart> {
    let line = unit
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("ExecStart="))?;
    let words = split_quoted(line);
    let exe = words.first()?;
    let config_index = words.iter().position(|w| w == "--config")?;
    let config = words.get(config_index + 1)?;
    if words.last().map(String::as_str) != Some("serve") {
        return None;
    }
    Some(ExecStart {
        exe: PathBuf::from(exe),
        config: PathBuf::from(config),
    })
}

/// Splits on spaces, keeping double-quoted segments together.
fn split_quoted(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Steps performed by `service install`, in order, for the report.
pub fn install_steps(unit: &Path) -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("write unit", vec![unit.display().to_string()]),
        ("systemctl", vec!["--user".into(), "daemon-reload".into()]),
        (
            "systemctl",
            vec![
                "--user".into(),
                "enable".into(),
                "--now".into(),
                UNIT_NAME.into(),
            ],
        ),
        ("loginctl", vec!["enable-linger".into()]),
    ]
}

pub fn write_unit(path: &Path, content: &str) -> Result<(), ServiceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ServiceError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, content).map_err(|source| ServiceError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_embeds_absolute_paths_and_restarts() {
        let unit = render_unit(
            Path::new("/usr/local/bin/anthroxy"),
            Path::new("/home/u/.config/anthroxy/config.toml"),
        );
        assert!(unit.contains("ExecStart=/usr/local/bin/anthroxy --config /home/u/.config/anthroxy/config.toml serve\n"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(
            unit.contains("ExecReload=/bin/kill -HUP $MAINPID\n"),
            "{unit}"
        );
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.starts_with("[Unit]\n"));
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        let unit = render_unit(Path::new("/opt/my tools/anthroxy"), Path::new("/c.toml"));
        assert!(unit.contains("ExecStart=\"/opt/my tools/anthroxy\" --config /c.toml serve"));
    }

    #[test]
    fn exec_start_round_trips_through_render() {
        let exe = Path::new("/opt/my tools/anthroxy");
        let config = Path::new("/home/u/.config/anthroxy/config.toml");
        let parsed = parse_exec_start(&render_unit(exe, config)).unwrap();
        assert_eq!(parsed.exe, exe);
        assert_eq!(parsed.config, config);

        let plain = parse_exec_start(&render_unit(
            Path::new("/usr/bin/anthroxy"),
            Path::new("/c.toml"),
        ))
        .unwrap();
        assert_eq!(plain.exe, Path::new("/usr/bin/anthroxy"));
        assert_eq!(plain.config, Path::new("/c.toml"));
    }

    #[test]
    fn exec_start_missing_or_foreign_is_none() {
        assert!(parse_exec_start("[Unit]\nDescription=x\n").is_none());
        assert!(parse_exec_start("[Service]\nExecStart=/usr/bin/other --flag\n").is_none());
    }

    #[test]
    fn install_steps_enable_linger_last() {
        let steps = install_steps(Path::new("/u.service"));
        assert_eq!(steps.last().unwrap().0, "loginctl");
        assert_eq!(steps[2].1, vec!["--user", "enable", "--now", UNIT_NAME]);
    }
}
