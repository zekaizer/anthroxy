use clap::{Args, Subcommand};

use super::{Cli, Style, display_path};
use crate::service::systemd::{self, ServiceError, UNIT_NAME};

#[derive(Debug, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Debug, Subcommand)]
pub enum ServiceAction {
    /// Write the unit, enable it now and allow it to run without a login
    Install {
        /// Print the unit file instead of installing it
        #[arg(long)]
        print: bool,
    },
    /// Stop and disable the unit, then delete it
    Uninstall,
    /// Show `systemctl --user status`
    Status,
}

pub fn run(cli: &Cli, args: &ServiceArgs, style: &Style) -> anyhow::Result<()> {
    let config = cli.config_path();
    let config = std::path::absolute(&config)?;
    match &args.action {
        ServiceAction::Install { print: true } => {
            let exe = std::env::current_exe().map_err(ServiceError::Exe)?;
            print!("{}", systemd::render_unit(&exe, &config));
            Ok(())
        }
        ServiceAction::Install { print: false } => {
            require_linux()?;
            let exe = std::env::current_exe().map_err(ServiceError::Exe)?;
            let unit_path = systemd::unit_path().ok_or(ServiceError::NoConfigDir)?;
            systemd::write_unit(&unit_path, &systemd::render_unit(&exe, &config))?;
            println!("{} wrote {}", style.ok_mark(), display_path(&unit_path));
            for (program, args) in systemd::install_steps(&unit_path).into_iter().skip(1) {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                systemd::run(program, &args)?;
                println!("{} {program} {}", style.ok_mark(), args.join(" "));
            }
            println!();
            println!("The router now starts with the system. Useful commands:");
            println!("  systemctl --user status {UNIT_NAME}");
            println!("  journalctl --user -u {UNIT_NAME} -f");
            println!("  claude-router service uninstall");
            Ok(())
        }
        ServiceAction::Uninstall => {
            require_linux()?;
            let unit_path = systemd::unit_path().ok_or(ServiceError::NoConfigDir)?;
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
            match systemd::run("systemctl", &["--user", "status", UNIT_NAME]) {
                Ok(out) => println!("{out}"),
                Err(ServiceError::Command { detail, .. }) => println!("{detail}"),
                Err(e) => return Err(e.into()),
            }
            Ok(())
        }
    }
}

fn require_linux() -> Result<(), ServiceError> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        Err(ServiceError::Unsupported)
    }
}
