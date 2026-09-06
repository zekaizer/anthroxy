use clap::{Args, Subcommand};

use super::style::table;
use super::{Cli, Style, display_path, is_wsl};
use crate::service::status::{Check, Expected, Verdict, collect};
use crate::service::systemd::{self, ServiceError, SystemRunner, UNIT_NAME};

#[derive(Debug, Clone, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ServiceAction {
    /// Write the unit, enable it now and allow it to run without a login
    Install {
        /// Print the unit file instead of installing it
        #[arg(long)]
        print: bool,
    },
    /// Ask the running service to re-read its configuration (SIGHUP)
    Reload,
    /// Stop and disable the unit, then delete it
    Uninstall,
    /// Check the installation: systemd, unit file and paths, enabled, active,
    /// linger, health endpoint. Exit 1 when anything fails.
    Status,
}

pub async fn run(cli: &Cli, args: &ServiceArgs, style: &Style) -> anyhow::Result<()> {
    let config = cli.config_path();
    let config = std::path::absolute(&config)?;
    match &args.action {
        ServiceAction::Install { print: true } => {
            print!("{}", systemd::render_unit(&systemd::exe()?, &config));
            Ok(())
        }
        ServiceAction::Install { print: false } => {
            require_linux()?;
            let unit_path = systemd::unit_path()?;
            systemd::write_unit(&unit_path, &systemd::render_unit(&systemd::exe()?, &config))?;
            println!("{} wrote {}", style.ok_mark(), display_path(&unit_path));
            for (program, args) in systemd::install_steps(&unit_path).into_iter().skip(1) {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                systemd::run(program, &args)?;
                println!(
                    "{} {}",
                    style.ok_mark(),
                    systemd::command_line(program, &args)
                );
            }
            println!();
            println!("The router now starts with the system. Useful commands:");
            println!("  systemctl --user status {UNIT_NAME}");
            println!("  journalctl --user -u {UNIT_NAME} -f");
            println!("  anthroxy service uninstall");
            Ok(())
        }
        ServiceAction::Reload => {
            require_linux()?;
            systemd::run("systemctl", &["--user", "reload", UNIT_NAME])?;
            println!(
                "{} reload signalled; check `journalctl --user -u {UNIT_NAME} -n 5`",
                style.ok_mark()
            );
            Ok(())
        }
        ServiceAction::Uninstall => {
            require_linux()?;
            let unit_path = systemd::unit_path()?;
            if let Err(error) =
                systemd::run("systemctl", &["--user", "disable", "--now", UNIT_NAME])
            {
                println!("{} {error}", style.warn_mark());
            }
            match std::fs::remove_file(&unit_path) {
                Ok(()) => println!("{} removed {}", style.ok_mark(), display_path(&unit_path)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!(
                        "{} {} was not installed",
                        style.warn_mark(),
                        display_path(&unit_path)
                    )
                }
                Err(e) => return Err(e.into()),
            }
            systemd::run("systemctl", &["--user", "daemon-reload"])?;
            Ok(())
        }
        ServiceAction::Status => {
            require_linux()?;
            let exe = systemd::exe()?;
            let unit_path = systemd::unit_path()?;
            let user = std::env::var("USER").unwrap_or_else(|_| "-".to_owned());
            let health_url = cli
                .load_config()
                .map(|c| format!("http://127.0.0.1:{}/healthz", c.server.listen.port()))
                .map_err(|e| e.to_string());
            let expected = Expected {
                exe: &exe,
                config: &config,
                unit_path: &unit_path,
                user: &user,
                health_url,
            };
            let checks = collect(&expected, &SystemRunner).await;
            print!("{}", render(&checks, style));
            if is_wsl() {
                println!(
                    "{}",
                    style.dim("WSL: the VM stops when idle unless %USERPROFILE%\\.wslconfig sets [wsl2] vmIdleTimeout=-1 (not checkable from here).")
                );
            }
            let failures = checks.iter().filter(|c| c.verdict == Verdict::Fail).count();
            if failures == 0 {
                Ok(())
            } else {
                anyhow::bail!("{failures} check(s) failed")
            }
        }
    }
}

/// One line per check: mark, name, detail.
pub fn render(checks: &[Check], style: &Style) -> String {
    let rows: Vec<Vec<String>> = checks
        .iter()
        .map(|c| {
            let mark = match c.verdict {
                Verdict::Ok => style.ok_mark(),
                Verdict::Fail => style.err_mark(),
                Verdict::Skip => style.dim("-"),
            };
            vec![format!("  {mark}"), style.bold(c.name), c.detail.clone()]
        })
        .collect();
    let mut out = table(&rows);
    let failures = checks.iter().filter(|c| c.verdict == Verdict::Fail).count();
    out.push('\n');
    if failures == 0 {
        out.push_str(&format!(
            "{} service installed and running\n",
            style.ok_mark()
        ));
    } else {
        out.push_str(&format!(
            "{} {failures} check(s) failed\n",
            style.err_mark()
        ));
    }
    out
}

fn require_linux() -> Result<(), ServiceError> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        Err(ServiceError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_marks_each_verdict_and_counts_failures() {
        let checks = vec![
            Check {
                name: "systemd",
                verdict: Verdict::Ok,
                detail: "user manager running".into(),
            },
            Check {
                name: "unit file",
                verdict: Verdict::Fail,
                detail: "not found".into(),
            },
            Check {
                name: "unit paths",
                verdict: Verdict::Skip,
                detail: "no unit file".into(),
            },
        ];
        let out = render(&checks, &Style::plain());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "  ✓  systemd     user manager running");
        assert_eq!(lines[1], "  ✗  unit file   not found");
        assert_eq!(lines[2], "  -  unit paths  no unit file");
        assert_eq!(lines.last().unwrap(), &"✗ 1 check(s) failed");

        let ok = render(&checks[..1], &Style::plain());
        assert!(ok.ends_with("✓ service installed and running\n"), "{ok}");
    }
}
