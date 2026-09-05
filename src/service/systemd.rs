//! systemd user unit: `~/.config/systemd/user/claude-router.service`.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const UNIT_NAME: &str = "claude-router.service";

/// Renders the unit. `exe` and `config` are embedded as absolute paths so the
/// unit does not depend on PATH or the working directory.
pub fn render_unit(exe: &Path, config: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=claude-router: one endpoint for Claude Code in front of several backends\n\
         Documentation=https://github.com/zekaizer/claude-router\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} --config {config} serve\n\
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

/// `~/.config/systemd/user/claude-router.service`
pub fn unit_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("systemd").join("user").join(UNIT_NAME))
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

/// One `systemctl --user` (or `loginctl`) invocation, surfaced verbatim in
/// the report.
pub fn run(program: &str, args: &[&str]) -> Result<String, ServiceError> {
    let command = format!("{program} {}", args.join(" "));
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| ServiceError::Command {
            command: command.clone(),
            detail: e.to_string(),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(ServiceError::Command {
            command,
            detail: if stderr.is_empty() { stdout } else { stderr },
        })
    }
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
            Path::new("/usr/local/bin/claude-router"),
            Path::new("/home/u/.config/claude-router/config.toml"),
        );
        assert!(unit.contains("ExecStart=/usr/local/bin/claude-router --config /home/u/.config/claude-router/config.toml serve\n"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.starts_with("[Unit]\n"));
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        let unit = render_unit(
            Path::new("/opt/my tools/claude-router"),
            Path::new("/c.toml"),
        );
        assert!(unit.contains("ExecStart=\"/opt/my tools/claude-router\" --config /c.toml serve"));
    }

    #[test]
    fn install_steps_enable_linger_last() {
        let steps = install_steps(Path::new("/u.service"));
        assert_eq!(steps.last().unwrap().0, "loginctl");
        assert_eq!(steps[2].1, vec!["--user", "enable", "--now", UNIT_NAME]);
    }
}
