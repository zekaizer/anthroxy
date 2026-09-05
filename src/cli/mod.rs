//! Command-line interface. Every subcommand is a small module; this file only
//! declares the grammar and dispatches.

mod check;
mod env;
mod init;
mod models;
mod serve;
mod service;
mod style;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::{self, Config, LogFormat};

pub use style::Style;

const AFTER_HELP: &str = "\
Getting started:
  1. claude-router init            write a commented config with a fresh token
  2. edit backends and models in the file it printed
  3. claude-router check           validate and probe every backend
  4. claude-router serve           start the router
  5. claude-router env             copy the variables into Claude Code's shell

Then pick any configured model with /model inside Claude Code.";

#[derive(Debug, Parser)]
#[command(
    name = "claude-router",
    version,
    about = "One endpoint for Claude Code in front of several Anthropic-compatible backends",
    after_help = AFTER_HELP,
    propagate_version = true
)]
pub struct Cli {
    /// Configuration file [default: ~/.config/claude-router/config.toml]
    #[arg(short, long, global = true, env = config::CONFIG_ENV, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Log filter: error|warn|info|debug|trace, or a directive such as
    /// "claude_router=debug,info". Overrides RUST_LOG and the file.
    #[arg(short = 'l', long, global = true, value_name = "FILTER")]
    pub log_level: Option<String>,

    /// Log line format; overrides the file
    #[arg(long, global = true, value_enum, value_name = "FORMAT")]
    pub log_format: Option<LogFormatArg>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogFormatArg {
    Text,
    Json,
}

impl From<LogFormatArg> for LogFormat {
    fn from(value: LogFormatArg) -> Self {
        match value {
            LogFormatArg::Text => LogFormat::Text,
            LogFormatArg::Json => LogFormat::Json,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the router
    Serve(serve::ServeArgs),
    /// Validate the configuration and probe every backend
    Check(check::CheckArgs),
    /// Show the models Claude Code will see and where each one goes
    Models,
    /// Write a commented example configuration with a fresh token
    Init(init::InitArgs),
    /// Print the environment variables that point Claude Code at the router
    Env(env::EnvArgs),
    /// Install, remove or inspect the systemd user service (Linux)
    Service(service::ServiceArgs),
}

impl Cli {
    /// Explicit `--config`, else `$CLAUDE_ROUTER_CONFIG`, else the default.
    pub fn config_path(&self) -> PathBuf {
        self.config.clone().unwrap_or_else(config::default_path)
    }

    fn load_config(&self) -> anyhow::Result<Config> {
        let path = self.config_path();
        Config::load(&path).map_err(|e| anyhow::anyhow!("{e}\n  (file: {})", path.display()))
    }

    /// Installs the tracing subscriber. `fallback` applies when neither the
    /// command line, `RUST_LOG` nor the file says otherwise.
    fn init_tracing(&self, config: Option<&Config>, fallback: &str) -> anyhow::Result<()> {
        let from_file = config.map(|c| c.logging.level.as_str()).unwrap_or(fallback);
        let directives = crate::observability::resolve_directives(
            self.log_level.as_deref(),
            std::env::var("RUST_LOG").ok().as_deref(),
            from_file,
        );
        let format = self
            .log_format
            .map(LogFormat::from)
            .or(config.map(|c| c.logging.format))
            .unwrap_or_default();
        crate::observability::init(&directives, format)
            .map_err(|e| anyhow::anyhow!("invalid log filter `{directives}`: {e}"))
    }
}

/// Runs the parsed command line. Errors are already user-facing text.
pub fn run(cli: Cli) -> anyhow::Result<()> {
    let style = Style::detect();
    match &cli.command {
        Command::Serve(args) => block_on(serve::run(&cli, args, &style)),
        Command::Check(args) => block_on(check::run(&cli, args, &style)),
        Command::Models => models::run(&cli, &style),
        Command::Init(args) => init::run(&cli, args, &style),
        Command::Env(args) => env::run(&cli, args),
        Command::Service(args) => service::run(&cli, args, &style),
    }
}

fn block_on<F: std::future::Future<Output = anyhow::Result<()>>>(future: F) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(future)
}

/// `~` shown instead of the home directory, for compact output.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}
