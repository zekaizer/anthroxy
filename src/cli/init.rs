use clap::Args;

use super::{Cli, Style, display_path};
use crate::config::example;

#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Overwrite an existing file
    #[arg(long)]
    pub force: bool,
    /// Print the configuration instead of writing it
    #[arg(long)]
    pub stdout: bool,
}

pub fn run(cli: &Cli, args: &InitArgs, style: &Style) -> anyhow::Result<()> {
    let text = example::render(&example::generate_token());
    if args.stdout {
        print!("{text}");
        return Ok(());
    }
    let path = cli.config_path();
    if path.exists() && !args.force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite it or --stdout to print an example",
            display_path(&path)
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_private(&path, &text)?;
    println!(
        "{} wrote {}",
        style.ok_mark(),
        style.bold(&display_path(&path))
    );
    println!();
    println!("Next:");
    println!("  1. edit the [backends.*] and [[models]] entries in that file");
    println!("  2. anthroxy check      validate and probe the backends");
    println!("  3. anthroxy serve      start the router");
    println!("  4. anthroxy env        variables for Claude Code");
    Ok(())
}

/// The file holds the client token: owner read/write only where supported.
/// The mode is in place before the token is written, and is tightened on a
/// file `--force` overwrites.
#[cfg(unix)]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}
